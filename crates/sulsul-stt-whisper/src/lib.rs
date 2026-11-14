//! 로컬 Whisper(whisper.cpp) 엔진.
//!
//! 지금은 설명자·설정 스키마·모델 목록만 있습니다. 실제 인식은 M1에서
//! `whisper-rs`로 구현합니다.

use std::path::PathBuf;
use std::sync::Arc;

use sulsul_core::AppPaths;
use sulsul_core::settings::{ChoiceOption, PathKind, ResolvedSettings, SettingField};
use sulsul_core::stt::{
    EngineCapabilities, EngineContext, EngineDescriptor, EngineKind, LanguageSupport, SttEngine,
    SttEngineFactory, SttError, WHISPER_SAMPLE_RATE,
};

pub const ENGINE_ID: &str = "whisper_local";
pub const DEFAULT_MODEL: &str = "large-v3-turbo-q5_0";

/// 설정 키
pub mod keys {
    pub const MODEL: &str = "model";
    pub const MODEL_PATH: &str = "model_path";
    pub const USE_GPU: &str = "use_gpu";
    pub const THREADS: &str = "threads";
}

/// 내려받을 수 있는 whisper.cpp GGML 모델
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperModel {
    pub id: &'static str,
    pub file_name: &'static str,
    /// 대략적인 파일 크기 (MiB)
    pub size_mib: u32,
}

impl WhisperModel {
    pub fn download_url(&self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
            self.file_name
        )
    }
}

/// 한국어 인식에는 `large-v3` 계열을 권장합니다. 작은 모델은 한국어 정확도가 많이 떨어집니다.
pub const MODELS: &[WhisperModel] = &[
    WhisperModel {
        id: "large-v3-turbo-q5_0",
        file_name: "ggml-large-v3-turbo-q5_0.bin",
        size_mib: 547,
    },
    WhisperModel {
        id: "large-v3-turbo-q8_0",
        file_name: "ggml-large-v3-turbo-q8_0.bin",
        size_mib: 834,
    },
    WhisperModel {
        id: "large-v3-turbo",
        file_name: "ggml-large-v3-turbo.bin",
        size_mib: 1549,
    },
    WhisperModel {
        id: "large-v3",
        file_name: "ggml-large-v3.bin",
        size_mib: 2950,
    },
    WhisperModel {
        id: "medium",
        file_name: "ggml-medium.bin",
        size_mib: 1462,
    },
    WhisperModel {
        id: "small",
        file_name: "ggml-small.bin",
        size_mib: 466,
    },
    WhisperModel {
        id: "base",
        file_name: "ggml-base.bin",
        size_mib: 142,
    },
];

pub fn find_model(id: &str) -> Option<&'static WhisperModel> {
    MODELS.iter().find(|m| m.id == id)
}

/// 사용할 모델 파일 경로. `model_path`가 있으면 그것을, 없으면 모델 폴더의 선택한 모델을 씁니다.
pub fn resolve_model_path(paths: &AppPaths, settings: &ResolvedSettings) -> Option<PathBuf> {
    let custom = settings.str(keys::MODEL_PATH).trim();
    if !custom.is_empty() {
        return Some(PathBuf::from(custom));
    }
    find_model(settings.str(keys::MODEL)).map(|m| paths.models_dir().join(m.file_name))
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
            .map(|m| ChoiceOption::new(m.id).with_detail(format!("{} MB", m.size_mib)))
            .collect();
        Self {
            descriptor: EngineDescriptor {
                id: ENGINE_ID.into(),
                name_key: "engine.whisper_local.name".into(),
                description_key: "engine.whisper_local.description".into(),
                kind: EngineKind::Local,
                capabilities: EngineCapabilities {
                    input_sample_rate: WHISPER_SAMPLE_RATE,
                    languages: LanguageSupport::Any,
                    supports_prompt: true,
                    streaming: false,
                },
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
                    SettingField::bool(
                        keys::USE_GPU,
                        "engine.whisper_local.settings.use_gpu",
                        true,
                    )
                    .with_help("engine.whisper_local.settings.use_gpu_help"),
                    SettingField::integer(
                        keys::THREADS,
                        "engine.whisper_local.settings.threads",
                        0,
                        0,
                        64,
                    )
                    .with_help("engine.whisper_local.settings.threads_help"),
                ],
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
        _ctx: &EngineContext,
        _settings: &ResolvedSettings,
    ) -> Result<Box<dyn SttEngine>, SttError> {
        Err(SttError::NotImplemented(ENGINE_ID.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::settings;

    #[test]
    fn default_model_is_listed() {
        assert!(find_model(DEFAULT_MODEL).is_some());
    }

    #[test]
    fn model_path_prefers_custom_file() {
        let factory = WhisperFactory::new();
        let schema = &factory.descriptor().settings;
        let paths = AppPaths::under("root");

        let defaults = settings::resolve(schema, None);
        assert_eq!(
            resolve_model_path(&paths, &defaults),
            Some(paths.models_dir().join("ggml-large-v3-turbo-q5_0.bin"))
        );

        let mut table = sulsul_core::toml::Table::new();
        table.insert(keys::MODEL_PATH.into(), "D:/models/custom.bin".into());
        let custom = settings::resolve(schema, Some(&table));
        assert_eq!(
            resolve_model_path(&paths, &custom),
            Some(PathBuf::from("D:/models/custom.bin"))
        );
    }
}
