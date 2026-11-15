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
    EngineCapabilities, EngineContext, EngineDescriptor, EngineKind, LanguageSupport,
    ModelRequirement, SttEngine, SttEngineFactory, SttError, WHISPER_SAMPLE_RATE,
};

use engine::{EngineOptions, WhisperEngine};
pub use models::{DEFAULT_MODEL, MODELS, WhisperModel, find_model};

pub const ENGINE_ID: &str = "whisper_local";

/// 설정 키
pub mod keys {
    pub const MODEL: &str = "model";
    pub const MODEL_PATH: &str = "model_path";
    pub const USE_GPU: &str = "use_gpu";
    pub const THREADS: &str = "threads";
    pub const BEAM_SIZE: &str = "beam_size";
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
                    SettingField::integer(
                        keys::BEAM_SIZE,
                        "engine.whisper_local.settings.beam_size",
                        5,
                        1,
                        8,
                    )
                    .with_help("engine.whisper_local.settings.beam_size_help"),
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
            threads: settings.int(keys::THREADS).max(0) as usize,
            beam_size: settings.int(keys::BEAM_SIZE).clamp(1, 8) as i32,
        };
        Ok(Box::new(WhisperEngine::load(&path, options)?))
    }

    fn model(&self, ctx: &EngineContext, settings: &ResolvedSettings) -> Option<ModelRequirement> {
        let custom = settings.str(keys::MODEL_PATH).trim();
        if !custom.is_empty() {
            return Some(ModelRequirement {
                path: PathBuf::from(custom),
                asset: None,
            });
        }
        let model = find_model(settings.str(keys::MODEL))?;
        Some(ModelRequirement {
            path: ctx.paths.models_dir().join(model.file_name),
            asset: Some(model.asset()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::settings;

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

    #[test]
    fn cpu_build_finds_no_gpu_device() {
        if compiled_gpu_backend().is_none() {
            assert_eq!(gpu_device(), None);
        }
    }
}
