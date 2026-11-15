//! 로컬 SenseVoice (ONNX) 음성 인식 엔진.
//!
//! - `lib.rs`: 팩토리, 설정 스키마, 설정 키
//! - [`models`]: 양자화 모델 변형 (`int8`, `fp32`) 및 `tokens.txt` 카탈로그
//! - `features.rs`: 80차원 FBANK + LFR + CMVN 특징 추출
//! - `engine.rs`: `ort` ONNX Runtime 추론 및 CTC 디코딩
//! - [`accel`]: 가속 장치(NPU·GPU) 감지와 선택. Windows에서는 DirectML을 씁니다.

pub mod accel;
mod engine;
mod features;
pub mod models;

use std::path::PathBuf;
use std::sync::Arc;

use sulsul_core::AppPaths;
use sulsul_core::settings::{ChoiceOption, PathKind, ResolvedSettings, SettingField};
use sulsul_core::stt::{
    EngineCapabilities, EngineContext, EngineDescriptor, EngineKind, LanguageSupport,
    ModelRequirement, ModelVariant, SttEngine, SttEngineFactory, SttError, WHISPER_SAMPLE_RATE,
};

pub use accel::{DetectedDevices, DevicePreference, detected_devices};
use engine::{EngineOptions, SenseVoiceEngine};
pub use models::{DEFAULT_MODEL, SenseVoiceVariant, VARIANTS, find_variant};

pub const ENGINE_ID: &str = "sense_voice_local";

/// 설정 키
pub mod keys {
    pub const MODEL: &str = "model";
    pub const MODEL_PATH: &str = "model_path";
    pub const USE_ITN: &str = "use_itn";
    /// 가속 장치: `auto` / `npu` / `gpu` / `cpu` ([`crate::DevicePreference`])
    pub const DEVICE: &str = "device";
    /// 예전의 CPU 스레드 수. 지금은 항상 ONNX Runtime 기본값을 쓰므로 파일에 남아 있어도 무시합니다.
    pub const RETIRED_THREADS: &str = "threads";
}

/// `device` 설정의 선택지. 이 PC에 없는 장치는 쓸 수 없음으로 표시하고, 있으면 장치 이름을 붙입니다.
fn device_options(detected: &DetectedDevices, directml_built: bool) -> Vec<ChoiceOption> {
    const PREFIX: &str = "engine.sense_voice_local.settings.device";
    DevicePreference::ALL
        .into_iter()
        .map(|pref| {
            let value = pref.as_str();
            let option = ChoiceOption::new(value).with_label(format!("{PREFIX}_{value}"));
            let found = match pref {
                DevicePreference::Npu => detected.npu(),
                DevicePreference::Gpu => detected.gpu(),
                DevicePreference::Auto | DevicePreference::Cpu => return option,
            };
            match found {
                _ if !directml_built => option.unavailable(format!("{PREFIX}_not_built")),
                Some(name) => option.with_detail(name),
                None => option.unavailable(format!("{PREFIX}_{value}_missing")),
            }
        })
        .collect()
}

pub fn factory() -> Arc<dyn SttEngineFactory> {
    Arc::new(SenseVoiceFactory::new())
}

pub struct SenseVoiceFactory {
    descriptor: EngineDescriptor,
}

impl SenseVoiceFactory {
    pub fn new() -> Self {
        let model_options = VARIANTS
            .iter()
            .map(|v| ChoiceOption::new(v.id).with_detail(v.size_label()))
            .collect();
        Self {
            descriptor: EngineDescriptor {
                id: ENGINE_ID.into(),
                name_key: "engine.sense_voice_local.name".into(),
                description_key: "engine.sense_voice_local.description".into(),
                kind: EngineKind::Local,
                capabilities: EngineCapabilities {
                    input_sample_rate: WHISPER_SAMPLE_RATE,
                    languages: LanguageSupport::AutoAnd(vec![
                        "ko".into(),
                        "en".into(),
                        "zh".into(),
                        "ja".into(),
                        "yue".into(),
                    ]),
                    supports_prompt: false,
                    streaming: false,
                },
                settings: vec![
                    SettingField::choice(
                        keys::MODEL,
                        "engine.sense_voice_local.settings.model",
                        DEFAULT_MODEL,
                        model_options,
                    )
                    .with_help("engine.sense_voice_local.settings.model_help"),
                    SettingField::path(
                        keys::MODEL_PATH,
                        "engine.sense_voice_local.settings.model_path",
                        PathKind::Directory,
                    )
                    .with_help("engine.sense_voice_local.settings.model_path_help"),
                    SettingField::bool(
                        keys::USE_ITN,
                        "engine.sense_voice_local.settings.use_itn",
                        true,
                    )
                    .with_help("engine.sense_voice_local.settings.use_itn_help"),
                    SettingField::choice(
                        keys::DEVICE,
                        "engine.sense_voice_local.settings.device",
                        DevicePreference::Auto.as_str(),
                        device_options(detected_devices(), accel::DIRECTML_BUILT),
                    )
                    .with_help("engine.sense_voice_local.settings.device_help"),
                ],
                retired_keys: vec![keys::RETIRED_THREADS.into()],
            },
        }
    }
}

impl Default for SenseVoiceFactory {
    fn default() -> Self {
        Self::new()
    }
}

fn resolve_paths(
    paths: &AppPaths,
    settings: &ResolvedSettings,
) -> Option<(PathBuf, PathBuf, Option<&'static SenseVoiceVariant>)> {
    let variant = find_variant(settings.str(keys::MODEL))?;
    let custom = settings.str(keys::MODEL_PATH).trim();
    let dir = if custom.is_empty() {
        models::default_model_dir(paths)
    } else {
        PathBuf::from(custom)
    };
    Some((
        dir.join(variant.model_file_name),
        dir.join(models::TOKENS_FILE),
        custom.is_empty().then_some(variant),
    ))
}

impl SttEngineFactory for SenseVoiceFactory {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    fn create(
        &self,
        ctx: &EngineContext,
        settings: &ResolvedSettings,
    ) -> Result<Box<dyn SttEngine>, SttError> {
        let (model_path, tokens_path, _) = resolve_paths(&ctx.paths, settings)
            .ok_or_else(|| SttError::Backend("선택된 SenseVoice 모델을 찾을 수 없습니다".into()))?;
        let options = EngineOptions {
            use_itn: settings.bool(keys::USE_ITN),
            device: DevicePreference::from_setting(settings.str(keys::DEVICE))
                .unwrap_or(DevicePreference::Auto),
        };
        Ok(Box::new(SenseVoiceEngine::load(
            &model_path,
            &tokens_path,
            options,
        )?))
    }

    fn model(&self, ctx: &EngineContext, settings: &ResolvedSettings) -> Option<ModelRequirement> {
        let (model_path, tokens_path, downloadable) = resolve_paths(&ctx.paths, settings)?;
        if let Some(variant) = downloadable {
            let dir = model_path.parent()?;
            return ModelRequirement::from_variant(&variant.to_catalog_variant(dir));
        }
        // 사용자가 폴더를 직접 지정한 경우: model 또는 tokens 중 빠진 파일을 우선 보고합니다.
        let missing = if !model_path.is_file() {
            model_path
        } else {
            tokens_path
        };
        Some(ModelRequirement::custom(missing))
    }

    fn model_variants(&self, ctx: &EngineContext) -> Vec<ModelVariant> {
        let dir = models::default_model_dir(&ctx.paths);
        VARIANTS
            .iter()
            .map(|variant| variant.to_catalog_variant(&dir))
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

    /// 예전 설정 파일에 남은 `threads = 0`은 오류도 "모르는 키" 경고도 아닙니다.
    #[test]
    fn retired_threads_key_is_ignored() {
        let mut registry = sulsul_core::stt::SttRegistry::new();
        registry.register(factory());
        let mut stt = sulsul_core::config::SttConfig::default();
        let table = stt.engines.entry(ENGINE_ID.into()).or_default();
        table.insert(keys::RETIRED_THREADS.into(), 0.into());
        assert!(registry.validate(&stt, ENGINE_ID).unwrap().is_empty());
        assert!(registry.unknown_keys(&stt, ENGINE_ID).is_empty());
    }

    #[test]
    fn device_setting_defaults_to_auto() {
        let factory = SenseVoiceFactory::new();
        let resolved = settings::resolve(&factory.descriptor().settings, None);
        assert_eq!(resolved.str(keys::DEVICE), "auto");
    }

    #[test]
    fn missing_devices_are_marked_unavailable() {
        let unavailable = |options: &[ChoiceOption], value: &str| {
            options
                .iter()
                .find(|o| o.value == value)
                .unwrap()
                .unavailable_key
                .clone()
        };
        let none = DetectedDevices::default();
        let options = device_options(&none, true);
        let values: Vec<_> = options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, ["auto", "npu", "gpu", "cpu"]);
        assert_eq!(
            unavailable(&options, "npu").as_deref(),
            Some("engine.sense_voice_local.settings.device_npu_missing")
        );
        assert_eq!(
            unavailable(&options, "gpu").as_deref(),
            Some("engine.sense_voice_local.settings.device_gpu_missing")
        );
        assert_eq!(unavailable(&options, "auto"), None);
        assert_eq!(unavailable(&options, "cpu"), None);

        let gpu_only = DetectedDevices {
            npus: Vec::new(),
            gpus: vec!["RTX".into()],
        };
        let options = device_options(&gpu_only, true);
        let gpu = options.iter().find(|o| o.value == "gpu").unwrap();
        assert_eq!(gpu.unavailable_key, None);
        assert_eq!(gpu.detail.as_deref(), Some("RTX"));

        let options = device_options(&gpu_only, false);
        assert_eq!(
            unavailable(&options, "gpu").as_deref(),
            Some("engine.sense_voice_local.settings.device_not_built")
        );
    }

    #[test]
    fn unavailable_saved_device_is_kept() {
        // 쓸 수 없는 장치를 저장해 두어도 값은 그대로 남고 실행할 때 CPU로 넘어갑니다.
        let factory = SenseVoiceFactory::new();
        let mut table = sulsul_core::toml::Table::new();
        table.insert(keys::DEVICE.into(), "npu".into());
        let schema = &factory.descriptor().settings;
        assert!(settings::validate(schema, &table).is_empty());
        let resolved = settings::resolve(schema, Some(&table));
        assert_eq!(resolved.str(keys::DEVICE), "npu");
    }

    #[test]
    fn model_requirement_includes_both_onnx_and_tokens() {
        let factory = SenseVoiceFactory::new();
        let ctx = EngineContext {
            paths: AppPaths::under("root"),
        };
        let resolved = settings::resolve(&factory.descriptor().settings, None);
        let req = factory.model(&ctx, &resolved).unwrap();
        assert_eq!(req.downloadable_files().len(), 2);
        assert!(req.path.ends_with("model.int8.onnx"));
        assert!(req.extra_files[0].path.ends_with("tokens.txt"));
        assert!(!req.is_installed());
    }

    #[test]
    fn custom_directory_requires_both_model_and_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let model_file = dir.path().join("model.int8.onnx");
        std::fs::write(&model_file, b"onnx").unwrap();

        let factory = SenseVoiceFactory::new();
        let ctx = EngineContext {
            paths: AppPaths::under("root"),
        };
        let mut table = sulsul_core::toml::Table::new();
        table.insert(
            keys::MODEL_PATH.into(),
            dir.path().display().to_string().into(),
        );
        let resolved = settings::resolve(&factory.descriptor().settings, Some(&table));

        // model만 있고 tokens.txt가 없으면 미설치로 보고합니다.
        let req = factory.model(&ctx, &resolved).unwrap();
        assert!(req.path.ends_with("tokens.txt"));
        assert!(!req.is_installed());

        std::fs::write(dir.path().join("tokens.txt"), b"tok").unwrap();
        let req = factory.model(&ctx, &resolved).unwrap();
        assert!(req.is_installed());
    }
}
