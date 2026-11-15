//! 로컬 Whisper(whisper.cpp) 엔진.
//!
//! - `lib.rs`: 팩토리, 설정 스키마, 설정 키
//! - [`models`]: 내려받을 수 있는 모델 목록
//! - `engine.rs`: `whisper-rs`로 하는 실제 인식
//!
//! GPU 가속은 Cargo 기능 `vulkan`, `cuda`로 넣습니다. 이 크레이트의 기본은 CPU 전용이고,
//! 앱과 CLI가 기본으로 `vulkan`을 켭니다. 넣어 빌드해도 실행 중에 쓸 수 있는 GPU가
//! 없으면([`gpu_device`]) CPU로 인식합니다.

mod engine;
pub mod models;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use sulsul_core::AppPaths;
use sulsul_core::settings::{ChoiceOption, PathKind, ResolvedSettings, SettingField};
use sulsul_core::stt::{
    DEFAULT_LANGUAGE, EngineCapabilities, EngineContext, EngineDescriptor, EngineKind,
    LanguageSupport, ModelFile, ModelRequirement, ModelVariant, SttEngine, SttEngineFactory,
    SttError, WHISPER_SAMPLE_RATE, keep_loaded_field, language_field,
};

use engine::{EngineOptions, WhisperEngine};
pub use models::{DEFAULT_MODEL, MODELS, WhisperModel, find_model};

pub const ENGINE_ID: &str = "whisper_local";

/// 설정 키
pub mod keys {
    pub const MODEL: &str = "model";
    pub const MODEL_PATH: &str = "model_path";
    pub const USE_GPU: &str = "use_gpu";
    pub const BEAM_SIZE: &str = "beam_size";
    /// 예전의 CPU 스레드 수. 지금은 항상 자동으로 정하므로 파일에 남아 있어도 무시합니다.
    pub const RETIRED_THREADS: &str = "threads";
}

/// 이 빌드에 들어 있는 GPU 백엔드. CPU 전용 빌드면 `None`입니다.
pub fn compiled_gpu_backend() -> Option<&'static str> {
    if cfg!(feature = "cuda") {
        Some("CUDA")
    } else if cfg!(feature = "vulkan") {
        Some("Vulkan")
    } else {
        None
    }
}

/// 지금 실행 중인 컴퓨터에서 Whisper가 쓸 GPU 장치의 이름
/// (예: `Vulkan0 (NVIDIA GeForce RTX 3060)`). 쓸 수 있는 GPU가 없으면 `None`입니다.
///
/// - CPU 전용 빌드([`compiled_gpu_backend`]가 `None`)면 장치를 찾아보지 않고 `None`입니다.
/// - GPU 빌드라도 드라이버가 없거나 장치 초기화가 실패하면 `None`입니다.
///
/// 처음 부를 때 ggml 장치를 열거하고 실제로 한 번 초기화해 보며(RTX 3060·Vulkan에서 약
/// 150ms, GPU가 없으면 약 20ms), 결과는 프로세스가 끝날 때까지 캐시합니다.
pub fn gpu_device() -> Option<&'static str> {
    static DEVICE: OnceLock<Option<String>> = OnceLock::new();
    DEVICE
        .get_or_init(|| {
            compiled_gpu_backend()?;
            // 장치를 찾는 동안 나오는 ggml 로그도 stderr 대신 tracing으로 보냅니다.
            whisper_rs::install_logging_hooks();
            let device = engine::usable_gpu_device();
            tracing::info!("whisper GPU device: {device:?}");
            device
        })
        .as_deref()
}

/// 사용할 모델 파일 경로. `model_path`가 있으면 그것을, 없으면 모델 폴더의 선택한 모델을 씁니다.
pub fn resolve_model_path(paths: &AppPaths, settings: &ResolvedSettings) -> Option<PathBuf> {
    let custom = settings.str(keys::MODEL_PATH).trim();
    if !custom.is_empty() {
        return Some(PathBuf::from(custom));
    }
    find_model(settings.str(keys::MODEL)).map(|m| paths.models_dir().join(m.file_name))
}

/// GPU 가속 항목을 잠글 이유의 번역 키. GPU를 쓸 수 있으면 `None`입니다.
fn gpu_unavailable_reason() -> Option<&'static str> {
    if compiled_gpu_backend().is_none() {
        // CPU 전용으로 빌드함 (`--no-default-features`)
        Some("engine.whisper_local.settings.use_gpu_unavailable")
    } else if gpu_device().is_none() {
        // GPU 백엔드는 넣었지만 이 컴퓨터에 쓸 수 있는 GPU가 없음
        Some("engine.whisper_local.settings.use_gpu_no_device")
    } else {
        None
    }
}

pub fn factory() -> Arc<dyn SttEngineFactory> {
    Arc::new(WhisperFactory::new())
}

pub struct WhisperFactory {
    descriptor: EngineDescriptor,
}

impl WhisperFactory {
    pub fn new() -> Self {
        let model_options = MODELS
            .iter()
            .map(|m| ChoiceOption::new(m.id).with_detail(m.size_label()))
            .collect();
        let mut use_gpu =
            SettingField::bool(keys::USE_GPU, "engine.whisper_local.settings.use_gpu", true)
                .with_help("engine.whisper_local.settings.use_gpu_help");
        // GPU를 쓸 수 없으면 켜 둬도 쓰이지 않으므로(engine.rs) 화면에서 잠그고 이유를 보여 줍니다.
        // 저장된 값은 그대로 둡니다.
        if let Some(reason) = gpu_unavailable_reason() {
            use_gpu = use_gpu.unavailable(reason);
        }
        // 엔진 설정은 이 엔진의 모든 모델이 함께 쓰므로 선택지는 다국어 모델 기준입니다.
        // 영어 전용 모델은 고른 언어와 상관없이 영어로 인식합니다 (`languages`).
        let languages = models::multilingual();
        Self {
            descriptor: EngineDescriptor {
                id: ENGINE_ID.into(),
                name_key: "engine.whisper_local.name".into(),
                description_key: "engine.whisper_local.description".into(),
                kind: EngineKind::Local,
                settings: vec![
                    SettingField::choice(
                        keys::MODEL,
                        "engine.whisper_local.settings.model",
                        DEFAULT_MODEL,
                        model_options,
                    )
                    .with_help("engine.whisper_local.settings.model_help"),
                    SettingField::path(
                        keys::MODEL_PATH,
                        "engine.whisper_local.settings.model_path",
                        PathKind::File,
                    )
                    .with_help("engine.whisper_local.settings.model_path_help"),
                    language_field(&languages, DEFAULT_LANGUAGE),
                    use_gpu,
                    SettingField::integer(
                        keys::BEAM_SIZE,
                        "engine.whisper_local.settings.beam_size",
                        5,
                        1,
                        8,
                    )
                    .with_help("engine.whisper_local.settings.beam_size_help"),
                    keep_loaded_field(),
                ],
                retired_keys: vec![keys::RETIRED_THREADS.into()],
                capabilities: EngineCapabilities {
                    input_sample_rate: WHISPER_SAMPLE_RATE,
                    languages,
                    supports_prompt: true,
                    streaming: false,
                },
            },
        }
    }
}

impl Default for WhisperFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl SttEngineFactory for WhisperFactory {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    fn create(
        &self,
        ctx: &EngineContext,
        settings: &ResolvedSettings,
    ) -> Result<Box<dyn SttEngine>, SttError> {
        let path = resolve_model_path(&ctx.paths, settings)
            .ok_or_else(|| SttError::Backend("no whisper model is selected".into()))?;
        if !path.is_file() {
            return Err(SttError::ModelNotFound(path));
        }
        // whisper.cpp 로그가 stderr로 쏟아지지 않게 tracing으로 돌립니다.
        // 여러 번 불러도 처음 한 번만 적용됩니다.
        whisper_rs::install_logging_hooks();

        let options = EngineOptions {
            use_gpu: settings.bool(keys::USE_GPU),
            beam_size: settings.int(keys::BEAM_SIZE).clamp(1, 8) as i32,
        };
        Ok(Box::new(WhisperEngine::load(&path, options)?))
    }

    /// 영어 전용 모델을 고르면 영어만, 그 밖(다국어 모델, 직접 지정한 파일)은 설명자의 언어입니다.
    fn languages(&self, settings: &ResolvedSettings) -> LanguageSupport {
        let custom = !settings.str(keys::MODEL_PATH).trim().is_empty();
        match find_model(settings.str(keys::MODEL)) {
            Some(model) if !custom && model.is_english_only() => model.languages(),
            _ => self.descriptor.capabilities.languages.clone(),
        }
    }

    fn model(&self, ctx: &EngineContext, settings: &ResolvedSettings) -> Option<ModelRequirement> {
        let custom = settings.str(keys::MODEL_PATH).trim();
        if !custom.is_empty() {
            return Some(ModelRequirement::custom(PathBuf::from(custom)));
        }
        let model = find_model(settings.str(keys::MODEL))?;
        Some(ModelRequirement::single(
            ctx.paths.models_dir().join(model.file_name),
            model.asset(),
        ))
    }

    fn model_variants(&self, ctx: &EngineContext) -> Vec<ModelVariant> {
        MODELS
            .iter()
            .map(|m| ModelVariant {
                id: m.id.to_owned(),
                name_key: format!("engine.{ENGINE_ID}.models.{}.name", m.id),
                description_key: format!("engine.{ENGINE_ID}.models.{}.description", m.id),
                recommended: m.recommended,
                files: vec![ModelFile {
                    path: ctx.paths.models_dir().join(m.file_name),
                    asset: m.asset(),
                }],
                choices: m.choices(),
            })
            .collect()
    }

    fn model_setting_key(&self) -> Option<&'static str> {
        Some(keys::MODEL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::settings;
    use sulsul_core::stt::{ModelAxis, axis_options, catalog_axes, find_by_choices};

    fn custom_path_settings(factory: &WhisperFactory, path: &str) -> ResolvedSettings {
        let mut table = sulsul_core::toml::Table::new();
        table.insert(keys::MODEL_PATH.into(), path.into());
        settings::resolve(&factory.descriptor().settings, Some(&table))
    }

    #[test]
    fn model_path_prefers_custom_file() {
        let factory = WhisperFactory::new();
        let paths = AppPaths::under("root");

        let defaults = settings::resolve(&factory.descriptor().settings, None);
        assert_eq!(
            resolve_model_path(&paths, &defaults),
            Some(paths.models_dir().join("ggml-large-v3.bin"))
        );

        let custom = custom_path_settings(&factory, "D:/models/custom.bin");
        assert_eq!(
            resolve_model_path(&paths, &custom),
            Some(PathBuf::from("D:/models/custom.bin"))
        );
    }

    #[test]
    fn model_requirement_follows_settings() {
        let factory = WhisperFactory::new();
        let ctx = EngineContext {
            paths: AppPaths::under("root"),
        };

        let defaults = settings::resolve(&factory.descriptor().settings, None);
        let requirement = factory.model(&ctx, &defaults).unwrap();
        assert_eq!(
            requirement.path,
            ctx.paths.models_dir().join("ggml-large-v3.bin")
        );
        assert_eq!(
            requirement.asset,
            Some(find_model(DEFAULT_MODEL).unwrap().asset())
        );

        let custom = custom_path_settings(&factory, "D:/models/custom.bin");
        let requirement = factory.model(&ctx, &custom).unwrap();
        assert_eq!(requirement.path, PathBuf::from("D:/models/custom.bin"));
        assert_eq!(requirement.asset, None);
    }

    #[test]
    fn create_reports_missing_model() {
        let factory = WhisperFactory::new();
        let ctx = EngineContext {
            paths: AppPaths::under("no-such-sulsul-root"),
        };
        let defaults = settings::resolve(&factory.descriptor().settings, None);
        let expected = ctx.paths.models_dir().join("ggml-large-v3.bin");
        match factory.create(&ctx, &defaults) {
            Err(SttError::ModelNotFound(path)) => assert_eq!(path, expected),
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("engine was created without a model file"),
        }
    }

    #[test]
    fn beam_size_defaults_to_five() {
        let factory = WhisperFactory::new();
        let defaults = settings::resolve(&factory.descriptor().settings, None);
        assert_eq!(defaults.int(keys::BEAM_SIZE), 5);
    }

    #[test]
    fn gpu_backend_matches_features() {
        let expected = if cfg!(feature = "cuda") {
            Some("CUDA")
        } else if cfg!(feature = "vulkan") {
            Some("Vulkan")
        } else {
            None
        };
        assert_eq!(compiled_gpu_backend(), expected);
    }

    /// 쓸 수 있는 GPU가 없으면 GPU 가속 항목을 화면에서 잠그고, 이유를 구분해 보여 줘야 합니다.
    #[test]
    fn gpu_setting_is_locked_only_without_usable_gpu() {
        let factory = WhisperFactory::new();
        let field = factory
            .descriptor()
            .settings
            .iter()
            .find(|f| f.key == keys::USE_GPU)
            .unwrap();
        let expected = match (compiled_gpu_backend(), gpu_device()) {
            (None, _) => Some("engine.whisper_local.settings.use_gpu_unavailable"),
            (Some(_), None) => Some("engine.whisper_local.settings.use_gpu_no_device"),
            (Some(_), Some(_)) => None,
        };
        assert_eq!(field.unavailable_key.as_deref(), expected);
    }

    #[test]
    fn cpu_build_finds_no_gpu_device() {
        if compiled_gpu_backend().is_none() {
            assert_eq!(gpu_device(), None);
        }
    }

    fn catalog() -> Vec<ModelVariant> {
        let ctx = EngineContext {
            paths: AppPaths::under("root"),
        };
        WhisperFactory::new().model_variants(&ctx)
    }

    fn picked(variant: &ModelVariant) -> Vec<(ModelAxis, &str)> {
        variant
            .choices
            .iter()
            .map(|c| (c.axis, c.value.as_str()))
            .collect()
    }

    #[test]
    fn every_variant_has_same_axes_in_order() {
        let catalog = catalog();
        assert_eq!(
            catalog_axes(&catalog),
            [ModelAxis::Size, ModelAxis::Language, ModelAxis::Precision]
        );
        for variant in &catalog {
            let axes: Vec<_> = variant.choices.iter().map(|c| c.axis).collect();
            assert_eq!(
                axes,
                catalog_axes(&catalog),
                "{}: 단계 순서가 다른 변형과 다릅니다",
                variant.id
            );
        }
    }

    #[test]
    fn choice_combinations_are_unique_and_findable() {
        let catalog = catalog();
        let mut seen = std::collections::HashSet::new();
        for variant in &catalog {
            let picks = picked(variant);
            assert!(
                seen.insert(picks.clone()),
                "{}: 같은 (크기, 언어, 정밀도) 조합을 가진 변형이 또 있습니다",
                variant.id
            );
            let found = find_by_choices(&catalog, &picks).unwrap();
            assert_eq!(
                found.id, variant.id,
                "단계 값으로 찾은 변형이 {}가 아닙니다",
                variant.id
            );
        }
    }

    #[test]
    fn choice_label_keys_follow_rule() {
        for variant in catalog() {
            for choice in &variant.choices {
                assert_eq!(
                    choice.label_key,
                    format!(
                        "engine.whisper_local.choices.{}.{}",
                        choice.axis.id(),
                        choice.value
                    ),
                    "{}: 번역 키 규칙이 다릅니다",
                    variant.id
                );
            }
        }
    }

    #[test]
    fn choice_values_match_model_ids() {
        let expected = [
            ("tiny", "tiny", "multi", "f16"),
            ("tiny-en", "tiny", "en", "f16"),
            ("base", "base", "multi", "f16"),
            ("base-en", "base", "en", "f16"),
            ("small", "small", "multi", "f16"),
            ("small-en", "small", "en", "f16"),
            ("medium", "medium", "multi", "f16"),
            ("medium-en", "medium", "en", "f16"),
            ("large-v3-turbo-q5_0", "large-v3-turbo", "multi", "q5_0"),
            ("large-v3-turbo", "large-v3-turbo", "multi", "f16"),
            ("large-v3", "large-v3", "multi", "f16"),
        ];
        let catalog = catalog();
        assert_eq!(catalog.len(), expected.len());
        for (variant, (id, size, language, precision)) in catalog.iter().zip(expected) {
            assert_eq!(variant.id, id, "모델 목록 순서가 다릅니다");
            let value = |axis| variant.choice(axis).unwrap().value.as_str();
            assert_eq!(value(ModelAxis::Size), size, "{id}: 크기 값");
            assert_eq!(value(ModelAxis::Language), language, "{id}: 언어 값");
            assert_eq!(value(ModelAxis::Precision), precision, "{id}: 정밀도 값");
        }
        let recommended: Vec<_> = catalog
            .iter()
            .filter(|v| v.recommended)
            .map(|v| v.id.as_str())
            .collect();
        assert_eq!(recommended, ["large-v3-turbo"]);
    }

    #[test]
    fn language_options_list_multilingual_first() {
        let catalog = catalog();
        let languages = |size: &str| -> Vec<String> {
            axis_options(&catalog, &[(ModelAxis::Size, size)], ModelAxis::Language)
                .into_iter()
                .map(|c| c.value.clone())
                .collect()
        };
        for size in ["tiny", "base", "small", "medium"] {
            assert_eq!(languages(size), ["multi", "en"], "{size}: 언어 선택지");
        }
        assert_eq!(languages("large-v3-turbo"), ["multi"]);
        assert_eq!(languages("large-v3"), ["multi"]);
    }

    /// whisper.cpp가 아는 언어 목록과 코어의 언어 목록(다국어 모델이 아는 언어)이 순서까지 같아야 합니다.
    #[test]
    fn whisper_languages_match_whisper_cpp() {
        let from_whisper: Vec<_> = (0..=whisper_rs::get_lang_max_id())
            .map(|id| whisper_rs::get_lang_str(id).expect("언어 ID에 코드가 없습니다"))
            .collect();
        assert_eq!(from_whisper, sulsul_core::stt::WHISPER_LANGUAGES);
    }

    #[test]
    fn schema_has_language_and_keep_loaded_in_order() {
        let factory = WhisperFactory::new();
        let schema: Vec<_> = factory
            .descriptor()
            .settings
            .iter()
            .map(|f| f.key.as_str())
            .collect();
        assert_eq!(
            schema,
            [
                keys::MODEL,
                keys::MODEL_PATH,
                sulsul_core::stt::LANGUAGE_KEY,
                keys::USE_GPU,
                keys::BEAM_SIZE,
                sulsul_core::stt::KEEP_LOADED_KEY,
            ]
        );
        let defaults = settings::resolve(&factory.descriptor().settings, None);
        assert_eq!(defaults.str(sulsul_core::stt::LANGUAGE_KEY), "ko");
        assert_eq!(defaults.int(sulsul_core::stt::KEEP_LOADED_KEY), 5);
        // 다국어 모델은 100개 언어를 알지만, 고를 수 있는 언어는 한국어·영어·중국어·일본어입니다.
        assert_eq!(
            factory.descriptor().capabilities.languages.choices(),
            ["auto", "ko", "en", "zh", "ja"]
        );
    }

    /// 예전 설정 파일에 남은 `threads = 0`은 오류도 "모르는 키" 경고도 아닙니다.
    #[test]
    fn retired_threads_key_is_ignored() {
        let mut registry = sulsul_core::stt::SttRegistry::new();
        registry.register(factory());
        let mut stt = sulsul_core::config::SttConfig::default();
        let table = stt.engines.entry(ENGINE_ID.into()).or_default();
        table.insert(keys::RETIRED_THREADS.into(), 0.into());
        table.insert(keys::MODEL.into(), "tiny".into());
        assert!(registry.validate(&stt, ENGINE_ID).unwrap().is_empty());
        assert!(registry.unknown_keys(&stt, ENGINE_ID).is_empty());
    }

    #[test]
    fn english_only_model_supports_only_english() {
        let mut registry = sulsul_core::stt::SttRegistry::new();
        registry.register(factory());
        let english = LanguageSupport::Only(vec!["en".into()]);
        for id in ["tiny-en", "base-en", "small-en", "medium-en"] {
            assert_eq!(registry.variant_languages(ENGINE_ID, id).unwrap(), english);
        }
        assert_eq!(
            registry.variant_languages(ENGINE_ID, "large-v3").unwrap(),
            models::multilingual()
        );

        let mut stt = sulsul_core::config::SttConfig::default();
        let table = stt.engines.entry(ENGINE_ID.into()).or_default();
        table.insert(keys::MODEL.into(), "base-en".into());
        assert_eq!(registry.languages(&stt, ENGINE_ID).unwrap(), english);

        // 직접 지정한 파일은 어떤 모델인지 알 수 없으므로 다국어로 봅니다.
        let table = stt.engines.get_mut(ENGINE_ID).unwrap();
        table.insert(keys::MODEL_PATH.into(), "D:/models/custom.bin".into());
        assert_eq!(
            registry.languages(&stt, ENGINE_ID).unwrap(),
            models::multilingual()
        );
    }

    #[test]
    fn size_options_are_ordered_small_to_large() {
        let catalog = catalog();
        let sizes: Vec<_> = axis_options(&catalog, &[], ModelAxis::Size)
            .into_iter()
            .map(|c| c.value.as_str())
            .collect();
        assert_eq!(
            sizes,
            [
                "tiny",
                "base",
                "small",
                "medium",
                "large-v3-turbo",
                "large-v3"
            ],
            "크기 선택지 순서가 작은 모델부터가 아닙니다"
        );

        let turbo = [(ModelAxis::Size, "large-v3-turbo")];
        let precisions: Vec<_> = axis_options(&catalog, &turbo, ModelAxis::Precision)
            .into_iter()
            .map(|c| c.value.as_str())
            .collect();
        assert_eq!(precisions, ["q5_0", "f16"]);
    }
}
