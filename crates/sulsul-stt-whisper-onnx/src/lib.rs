//! 로컬 Whisper (ONNX) 음성 인식 엔진.
//!
//! whisper.cpp 엔진(`sulsul-stt-whisper`)과 같은 OpenAI Whisper 모델을 ONNX Runtime으로 돌립니다.
//! Windows에서는 인코더를 DirectML로 GPU에서 돌리고(디코더는 CPU), GPU를 쓸 수 없으면 모두 CPU로 돌립니다.
//!
//! - `lib.rs`: 팩토리, 설정 스키마, 설정 키
//! - [`models`]: 내려받을 수 있는 모델 목록 (sherpa-onnx 내보내기)
//! - `features.rs`: Whisper 로그 멜 특징 (80·128밴드)
//! - `chunks.rs`: 30초보다 긴 오디오를 조용한 곳에서 나누기
//! - `engine.rs`: 받아쓰기 흐름과 greedy 디코딩
//! - `runner.rs`: ONNX Runtime 세션(인코더는 고른 장치, 디코더는 CPU), IoBinding으로 번갈아 쓰는 KV 캐시
//! - `decode.rs`: 다음 토큰 고르기 규칙과 되풀이 끊기
//! - `tokens.rs`: 토큰 표와 토큰 → 텍스트
//! - [`accel`]: 가속 장치 설정. 장치 감지와 ONNX Runtime 환경은 SenseVoice 엔진과 함께 씁니다
//!   (`ort`는 프로세스마다 환경을 하나만 만들 수 있습니다).

pub mod accel;
mod chunks;
mod decode;
mod engine;
mod features;
pub mod models;
mod runner;
mod tokens;

use std::path::PathBuf;
use std::sync::Arc;

use sulsul_core::AppPaths;
use sulsul_core::settings::{ChoiceOption, PathKind, ResolvedSettings, SettingField};
use sulsul_core::stt::{
    DEFAULT_LANGUAGE, EngineCapabilities, EngineContext, EngineDescriptor, EngineKind,
    LanguageSupport, ModelRequirement, ModelVariant, SttEngine, SttEngineFactory, SttError,
    WHISPER_SAMPLE_RATE, keep_loaded_field, language_field,
};
use sulsul_stt_sensevoice::accel::{DIRECTML_BUILT, detected_devices};

pub use accel::DevicePreference;
use engine::{EngineOptions, WhisperOnnxEngine};
pub use models::{DEFAULT_MODEL, MODELS, ModelPaths, WhisperOnnxModel, find_model};

pub const ENGINE_ID: &str = "whisper_onnx";

/// 설정 키
pub mod keys {
    pub const MODEL: &str = "model";
    /// 모델 파일이 있는 폴더. 비어 있으면 내려받은 모델 폴더를 씁니다.
    pub const MODEL_PATH: &str = "model_path";
    /// 가속 장치: `auto` / `gpu` / `cpu` ([`crate::DevicePreference`])
    pub const DEVICE: &str = "device";
}

pub fn factory() -> Arc<dyn SttEngineFactory> {
    Arc::new(WhisperOnnxFactory::new())
}

pub struct WhisperOnnxFactory {
    descriptor: EngineDescriptor,
}

impl WhisperOnnxFactory {
    pub fn new() -> Self {
        let model_options = MODELS
            .iter()
            .map(|m| ChoiceOption::new(m.id).with_detail(m.size_label()))
            .collect();
        // 엔진 설정은 이 엔진의 모든 모델이 함께 쓰므로 선택지는 다국어 모델 기준입니다.
        // 영어 전용 모델은 고른 언어와 상관없이 영어로 인식합니다 (`languages`).
        let languages = models::multilingual();
        Self {
            descriptor: EngineDescriptor {
                id: ENGINE_ID.into(),
                name_key: "engine.whisper_onnx.name".into(),
                description_key: "engine.whisper_onnx.description".into(),
                kind: EngineKind::Local,
                settings: vec![
                    SettingField::choice(
                        keys::MODEL,
                        "engine.whisper_onnx.settings.model",
                        DEFAULT_MODEL,
                        model_options,
                    )
                    .with_help("engine.whisper_onnx.settings.model_help"),
                    SettingField::path(
                        keys::MODEL_PATH,
                        "engine.whisper_onnx.settings.model_path",
                        PathKind::Directory,
                    )
                    .with_help("engine.whisper_onnx.settings.model_path_help"),
                    language_field(&languages, DEFAULT_LANGUAGE),
                    SettingField::choice(
                        keys::DEVICE,
                        "engine.whisper_onnx.settings.device",
                        DevicePreference::Auto.as_str(),
                        accel::device_options(detected_devices(), DIRECTML_BUILT),
                    )
                    .with_help("engine.whisper_onnx.settings.device_help"),
                    keep_loaded_field(),
                ],
                retired_keys: Vec::new(),
                capabilities: EngineCapabilities {
                    input_sample_rate: WHISPER_SAMPLE_RATE,
                    languages,
                    // 용어 힌트(initial prompt)는 첫 버전에서 지원하지 않습니다.
                    supports_prompt: false,
                    streaming: false,
                },
            },
        }
    }
}

impl Default for WhisperOnnxFactory {
    fn default() -> Self {
        Self::new()
    }
}

/// 고른 모델과 그 파일이 있는 폴더. `model_path`가 있으면 그 폴더에서 고른 모델의 파일 이름을 찾습니다.
/// 두 번째 값은 내려받을 수 있는지(직접 지정한 폴더가 아닌지)입니다.
fn resolve(
    paths: &AppPaths,
    settings: &ResolvedSettings,
) -> Option<(&'static WhisperOnnxModel, PathBuf, bool)> {
    let model = find_model(settings.str(keys::MODEL))?;
    let custom = settings.str(keys::MODEL_PATH).trim();
    if custom.is_empty() {
        Some((model, models::default_model_dir(paths), true))
    } else {
        Some((model, PathBuf::from(custom), false))
    }
}

/// 인식에 쓸 인코더·디코더·토큰 파일 경로
pub fn resolve_model_paths(paths: &AppPaths, settings: &ResolvedSettings) -> Option<ModelPaths> {
    resolve(paths, settings).map(|(model, dir, _)| model.paths_in(&dir))
}

impl SttEngineFactory for WhisperOnnxFactory {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    fn create(
        &self,
        ctx: &EngineContext,
        settings: &ResolvedSettings,
    ) -> Result<Box<dyn SttEngine>, SttError> {
        let paths = resolve_model_paths(&ctx.paths, settings)
            .ok_or_else(|| SttError::Backend("no Whisper ONNX model is selected".into()))?;
        let options = EngineOptions {
            device: DevicePreference::from_setting(settings.str(keys::DEVICE))
                .unwrap_or(DevicePreference::Auto),
        };
        Ok(Box::new(WhisperOnnxEngine::load(&paths, options)?))
    }

    /// 영어 전용 모델을 고르면 영어만, 다국어 모델은 설명자의 언어입니다. 직접 지정한 폴더도 고른 모델의
    /// 파일 이름을 찾으므로 같은 규칙입니다.
    fn languages(&self, settings: &ResolvedSettings) -> LanguageSupport {
        match find_model(settings.str(keys::MODEL)) {
            Some(model) if model.is_english_only() => model.languages(),
            _ => self.descriptor.capabilities.languages.clone(),
        }
    }

    fn model(&self, ctx: &EngineContext, settings: &ResolvedSettings) -> Option<ModelRequirement> {
        let (model, dir, downloadable) = resolve(&ctx.paths, settings)?;
        if downloadable {
            return ModelRequirement::from_variant(&model.to_catalog_variant(&dir));
        }
        // 직접 지정한 폴더: 인코더·디코더·토큰 가운데 빠진 파일을 먼저 보고합니다.
        let files = model.paths_in(&dir);
        let missing = [&files.encoder, &files.decoder, &files.tokens]
            .into_iter()
            .find(|path| !path.is_file())
            .unwrap_or(&files.encoder)
            .clone();
        Some(ModelRequirement::custom(missing))
    }

    fn model_variants(&self, ctx: &EngineContext) -> Vec<ModelVariant> {
        let dir = models::default_model_dir(&ctx.paths);
        MODELS
            .iter()
            .map(|model| model.to_catalog_variant(&dir))
            .collect()
    }

    fn model_setting_key(&self) -> Option<&'static str> {
        Some(keys::MODEL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::settings::{self, SettingKind};
    use sulsul_core::stt::{KEEP_LOADED_KEY, LANGUAGE_KEY, SttRegistry};

    fn custom_dir_settings(
        factory: &WhisperOnnxFactory,
        model: &str,
        dir: &str,
    ) -> ResolvedSettings {
        let mut table = sulsul_core::toml::Table::new();
        table.insert(keys::MODEL.into(), model.into());
        table.insert(keys::MODEL_PATH.into(), dir.into());
        settings::resolve(&factory.descriptor().settings, Some(&table))
    }

    #[test]
    fn schema_lists_only_supported_settings_in_order() {
        let factory = WhisperOnnxFactory::new();
        let descriptor = factory.descriptor();
        let schema: Vec<_> = descriptor.settings.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            schema,
            [
                keys::MODEL,
                keys::MODEL_PATH,
                LANGUAGE_KEY,
                keys::DEVICE,
                KEEP_LOADED_KEY
            ]
        );
        assert_eq!(descriptor.id, ENGINE_ID);
        assert_eq!(descriptor.kind, EngineKind::Local);
        assert!(!descriptor.capabilities.supports_prompt);
        assert!(!descriptor.capabilities.streaming);
        assert_eq!(descriptor.capabilities.input_sample_rate, 16_000);
        // 엔진 고유 항목의 번역 키는 모두 engine.whisper_onnx.* 아래에 있습니다.
        for field in &descriptor.settings {
            if [LANGUAGE_KEY, KEEP_LOADED_KEY].contains(&field.key.as_str()) {
                continue;
            }
            assert!(field.label_key.starts_with("engine.whisper_onnx.settings."));
            assert!(
                field
                    .help_key
                    .as_deref()
                    .is_some_and(|k| k.starts_with("engine.whisper_onnx.settings."))
            );
        }
        let path = descriptor
            .settings
            .iter()
            .find(|f| f.key == keys::MODEL_PATH)
            .unwrap();
        assert_eq!(
            path.kind,
            SettingKind::Path {
                kind: PathKind::Directory
            }
        );
    }

    #[test]
    fn defaults_are_recommended_model_korean_and_auto_device() {
        let factory = WhisperOnnxFactory::new();
        let defaults = settings::resolve(&factory.descriptor().settings, None);
        assert_eq!(defaults.str(keys::MODEL), DEFAULT_MODEL);
        assert_eq!(defaults.str(keys::MODEL_PATH), "");
        assert_eq!(defaults.str(LANGUAGE_KEY), "ko");
        assert_eq!(defaults.str(keys::DEVICE), "auto");
        assert_eq!(defaults.int(KEEP_LOADED_KEY), 5);
        assert_eq!(
            factory.descriptor().capabilities.languages.choices(),
            ["auto", "ko", "en", "zh", "ja"]
        );
    }

    #[test]
    fn model_options_cover_catalog_with_sizes() {
        let factory = WhisperOnnxFactory::new();
        let field = factory
            .descriptor()
            .settings
            .iter()
            .find(|f| f.key == keys::MODEL)
            .unwrap();
        let SettingKind::Choice { options } = &field.kind else {
            panic!("모델 항목이 선택 항목이 아닙니다");
        };
        let values: Vec<_> = options.iter().map(|o| o.value.as_str()).collect();
        let ids: Vec<_> = MODELS.iter().map(|m| m.id).collect();
        assert_eq!(values, ids);
        assert!(options.iter().all(|o| o.detail.is_some()));
    }

    #[test]
    fn device_options_have_no_npu() {
        let factory = WhisperOnnxFactory::new();
        let field = factory
            .descriptor()
            .settings
            .iter()
            .find(|f| f.key == keys::DEVICE)
            .unwrap();
        let SettingKind::Choice { options } = &field.kind else {
            panic!("장치 항목이 선택 항목이 아닙니다");
        };
        let values: Vec<_> = options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, ["auto", "gpu", "cpu"]);
        // NPU를 적어 둔 설정은 잘못된 값이라 기본값(auto)으로 읽힙니다.
        let mut table = sulsul_core::toml::Table::new();
        table.insert(keys::DEVICE.into(), "npu".into());
        let schema = &factory.descriptor().settings;
        assert!(!settings::validate(schema, &table).is_empty());
        assert_eq!(
            settings::resolve(schema, Some(&table)).str(keys::DEVICE),
            "auto"
        );
    }

    #[test]
    fn model_requirement_lists_encoder_decoder_and_tokens() {
        let factory = WhisperOnnxFactory::new();
        let ctx = EngineContext {
            paths: AppPaths::under("root"),
        };
        let defaults = settings::resolve(&factory.descriptor().settings, None);
        let req = factory.model(&ctx, &defaults).unwrap();
        let dir = ctx.paths.models_dir().join("whisper_onnx");
        assert_eq!(req.path, dir.join("turbo-encoder.int8.onnx"));
        let extra: Vec<_> = req.extra_files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            extra,
            [
                dir.join("turbo-decoder.int8.onnx"),
                dir.join("turbo-tokens.txt")
            ]
        );
        assert_eq!(req.downloadable_files().len(), 3);
        assert!(!req.is_installed());

        // 외부 가중치가 있는 모델은 그 파일도 함께 받습니다.
        let mut table = sulsul_core::toml::Table::new();
        table.insert(keys::MODEL.into(), "large-v3-turbo".into());
        let turbo = settings::resolve(&factory.descriptor().settings, Some(&table));
        let req = factory.model(&ctx, &turbo).unwrap();
        assert_eq!(req.downloadable_files().len(), 4);
        assert!(
            req.extra_files
                .iter()
                .any(|f| f.path == dir.join("turbo-encoder.weights"))
        );
        assert_eq!(
            req.total_bytes(),
            Some(find_model("large-v3-turbo").unwrap().total_bytes())
        );
    }

    #[test]
    fn custom_directory_reports_first_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let factory = WhisperOnnxFactory::new();
        let ctx = EngineContext {
            paths: AppPaths::under("root"),
        };
        let resolved = custom_dir_settings(&factory, "tiny", &dir.path().display().to_string());
        let paths = resolve_model_paths(&ctx.paths, &resolved).unwrap();
        assert_eq!(paths.encoder, dir.path().join("tiny-encoder.onnx"));

        let req = factory.model(&ctx, &resolved).unwrap();
        assert_eq!(req.path, paths.encoder);
        assert!(req.asset.is_none());
        std::fs::write(&paths.encoder, b"onnx").unwrap();
        assert_eq!(factory.model(&ctx, &resolved).unwrap().path, paths.decoder);
        std::fs::write(&paths.decoder, b"onnx").unwrap();
        assert_eq!(factory.model(&ctx, &resolved).unwrap().path, paths.tokens);
        std::fs::write(&paths.tokens, b"tok").unwrap();
        assert!(factory.model(&ctx, &resolved).unwrap().is_installed());
    }

    #[test]
    fn create_reports_missing_model() {
        let factory = WhisperOnnxFactory::new();
        let ctx = EngineContext {
            paths: AppPaths::under("no-such-sulsul-root"),
        };
        let defaults = settings::resolve(&factory.descriptor().settings, None);
        let expected = ctx
            .paths
            .models_dir()
            .join("whisper_onnx")
            .join("turbo-encoder.int8.onnx");
        match factory.create(&ctx, &defaults) {
            Err(SttError::ModelNotFound(path)) => assert_eq!(path, expected),
            Err(other) => panic!("다른 오류: {other}"),
            Ok(_) => panic!("모델 파일 없이 엔진이 만들어졌습니다"),
        }
    }

    #[test]
    fn english_only_model_supports_only_english() {
        let mut registry = SttRegistry::new();
        registry.register(factory());
        let english = LanguageSupport::Only(vec!["en".into()]);
        for id in [
            "tiny-en-int8",
            "tiny-en",
            "base-en",
            "small-en-int8",
            "medium-en",
        ] {
            assert_eq!(
                registry.variant_languages(ENGINE_ID, id).unwrap(),
                english,
                "{id}"
            );
        }
        assert_eq!(
            registry
                .variant_languages(ENGINE_ID, DEFAULT_MODEL)
                .unwrap(),
            models::multilingual()
        );

        let mut stt = sulsul_core::config::SttConfig::default();
        let table = stt.engines.entry(ENGINE_ID.into()).or_default();
        table.insert(keys::MODEL.into(), "base-en-int8".into());
        assert_eq!(registry.languages(&stt, ENGINE_ID).unwrap(), english);
        // 영어 전용 모델은 한국어를 골라 두어도 영어로 인식합니다.
        assert_eq!(
            registry
                .resolve_language(&stt, ENGINE_ID, Some("ko"))
                .unwrap(),
            sulsul_core::stt::Language::Code("en".into())
        );
    }

    #[test]
    fn registry_accepts_default_settings() {
        let mut registry = SttRegistry::new();
        registry.register(factory());
        let stt = sulsul_core::config::SttConfig::default();
        assert!(registry.validate(&stt, ENGINE_ID).unwrap().is_empty());
        assert!(registry.unknown_keys(&stt, ENGINE_ID).is_empty());
        let ctx = EngineContext {
            paths: AppPaths::under("root"),
        };
        let variants = registry.model_variants(&ctx, ENGINE_ID).unwrap();
        assert_eq!(variants.len(), MODELS.len());
    }
}
