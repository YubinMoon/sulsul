//! 클라우드 STT (OpenAI 호환 `/audio/transcriptions`) 엔진.
//!
//! `[[stt.api_profiles]]`에 저장된 활성 API 프로필(`provider`, `endpoint`, `api_key`, `model`)을
//! 읽어 동작하며, CLI `--set` 재정의 값도 함께 반영합니다.

mod client;
pub mod presets;

use std::sync::Arc;
use std::time::Duration;

use sulsul_core::config::SttConfig;
use sulsul_core::settings::{self, ResolvedSettings, SettingField};
use sulsul_core::stt::{
    DEFAULT_LANGUAGE, EngineCapabilities, EngineContext, EngineDescriptor, EngineKind,
    LanguageSupport, SttEngine, SttEngineFactory, SttError, WHISPER_SAMPLE_RATE, language_field,
};

pub use client::normalize_transcriptions_url;
use client::{CloudConfig, CloudEngine};
pub use presets::{PRESETS, ProviderPreset, find_preset, profile_from_preset, requires_api_key};

pub const ENGINE_ID: &str = "cloud_api";

/// 설정 키
pub mod keys {
    pub const TIMEOUT_SECONDS: &str = "timeout_seconds";
    pub const PROFILE_ID: &str = "profile_id";
    pub const PROVIDER: &str = "provider";
    pub const ENDPOINT: &str = "endpoint";
    pub const API_KEY: &str = "api_key";
    pub const MODEL: &str = "model";
}

pub fn factory() -> Arc<dyn SttEngineFactory> {
    Arc::new(CloudFactory::new())
}

pub struct CloudFactory {
    descriptor: EngineDescriptor,
}

impl CloudFactory {
    pub fn new() -> Self {
        let languages = LanguageSupport::Any;
        Self {
            descriptor: EngineDescriptor {
                id: ENGINE_ID.into(),
                name_key: "engine.cloud_api.name".into(),
                description_key: "engine.cloud_api.description".into(),
                kind: EngineKind::Cloud,
                // 모델을 메모리에 올리지 않으므로 메모리 유지 시간 항목은 없습니다.
                settings: vec![
                    language_field(&languages, DEFAULT_LANGUAGE),
                    SettingField::integer(
                        keys::TIMEOUT_SECONDS,
                        "engine.cloud_api.settings.timeout_seconds",
                        30,
                        5,
                        300,
                    )
                    .with_help("engine.cloud_api.settings.timeout_seconds_help"),
                ],
                retired_keys: Vec::new(),
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

impl Default for CloudFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl SttEngineFactory for CloudFactory {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    fn resolve_settings(&self, stt: &SttConfig) -> ResolvedSettings {
        let mut resolved = settings::resolve(&self.descriptor.settings, stt.engines.get(ENGINE_ID));
        if let Some(profile) = stt.active_api_profile() {
            resolved.set(keys::PROFILE_ID, profile.id.clone().into());
            resolved.set(keys::PROVIDER, profile.provider.clone().into());
            resolved.set(keys::ENDPOINT, profile.endpoint.clone().into());
            resolved.set(keys::API_KEY, profile.api_key.clone().into());
            resolved.set(keys::MODEL, profile.model.clone().into());
        }
        // CLI `--set provider=... endpoint=... api_key=... model=...` 재정의도 지원합니다.
        if let Some(table) = stt.engines.get(ENGINE_ID) {
            for key in [keys::PROVIDER, keys::ENDPOINT, keys::API_KEY, keys::MODEL] {
                if let Some(value) = table.get(key).and_then(|v| v.as_str())
                    && !value.trim().is_empty()
                {
                    resolved.set(key, value.to_owned().into());
                }
            }
        }
        resolved
    }

    fn create(
        &self,
        _ctx: &EngineContext,
        settings: &ResolvedSettings,
    ) -> Result<Box<dyn SttEngine>, SttError> {
        let provider = settings.str(keys::PROVIDER).trim();
        let endpoint = settings.str(keys::ENDPOINT).trim();
        let api_key = settings.str(keys::API_KEY).trim();
        let model = settings.str(keys::MODEL).trim();

        if endpoint.is_empty() {
            return Err(SttError::Backend(
                "선택된 API 프로필이 없거나 엔드포인트가 비어 있습니다. 모델 관리 탭에서 API 프로필을 추가하세요."
                    .into(),
            ));
        }
        if requires_api_key(provider, endpoint) && api_key.is_empty() {
            return Err(SttError::Backend(
                "선택한 API 공급자에는 API 키가 필요합니다. 모델 관리 탭에서 API 키를 입력하세요."
                    .into(),
            ));
        }
        let timeout_secs = settings.int(keys::TIMEOUT_SECONDS).clamp(5, 300) as u64;

        Ok(Box::new(CloudEngine::new(CloudConfig {
            endpoint: endpoint.to_owned(),
            api_key: api_key.to_owned(),
            model: model.to_owned(),
            timeout: Duration::from_secs(timeout_secs),
        })?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::AppPaths;
    use sulsul_core::config::ApiProfile;

    #[test]
    fn resolve_settings_picks_active_profile_and_allows_cli_override() {
        let factory = CloudFactory::new();
        let mut stt = SttConfig {
            engine: ENGINE_ID.into(),
            api_profile: "p2".into(),
            api_profiles: vec![
                ApiProfile {
                    id: "p1".into(),
                    name: "OpenAI".into(),
                    provider: "openai".into(),
                    endpoint: "https://api.openai.com/v1".into(),
                    api_key: "sk-1".into(),
                    model: "whisper-1".into(),
                },
                ApiProfile {
                    id: "p2".into(),
                    name: "Groq".into(),
                    provider: "groq".into(),
                    endpoint: "https://api.groq.com/openai/v1".into(),
                    api_key: "gsk-2".into(),
                    model: "whisper-large-v3-turbo".into(),
                },
            ],
            ..SttConfig::default()
        };

        let resolved = factory.resolve_settings(&stt);
        assert_eq!(resolved.str(keys::PROFILE_ID), "p2");
        assert_eq!(
            resolved.str(keys::ENDPOINT),
            "https://api.groq.com/openai/v1"
        );
        assert_eq!(resolved.str(keys::API_KEY), "gsk-2");
        assert_eq!(resolved.str(keys::MODEL), "whisper-large-v3-turbo");

        let table = stt.engines.entry(ENGINE_ID.into()).or_default();
        table.insert(keys::MODEL.into(), "custom-override".into());
        let overridden = factory.resolve_settings(&stt);
        assert_eq!(overridden.str(keys::MODEL), "custom-override");
        assert_ne!(resolved, overridden);
    }

    #[test]
    fn language_setting_lists_auto_and_offered_languages() {
        let factory = CloudFactory::new();
        let schema: Vec<_> = factory
            .descriptor()
            .settings
            .iter()
            .map(|f| f.key.as_str())
            .collect();
        assert_eq!(
            schema,
            [sulsul_core::stt::LANGUAGE_KEY, keys::TIMEOUT_SECONDS],
            "클라우드 엔진에는 메모리 유지 시간 항목이 없어야 합니다"
        );
        let resolved = factory.resolve_settings(&SttConfig::default());
        assert_eq!(resolved.str(sulsul_core::stt::LANGUAGE_KEY), "ko");
        assert_eq!(
            factory.descriptor().capabilities.languages.choices(),
            ["auto", "ko", "en", "zh", "ja"]
        );
    }

    #[test]
    fn create_rejects_missing_endpoint_or_required_key() {
        let factory = CloudFactory::new();
        let ctx = EngineContext {
            paths: AppPaths::under("unused"),
        };

        let missing_profile = factory.resolve_settings(&SttConfig::default());
        assert!(factory.create(&ctx, &missing_profile).is_err());

        let mut stt = SttConfig::default();
        stt.api_profiles
            .push(profile_from_preset("openai", "o1".into(), "OpenAI".into()));
        let missing_key = factory.resolve_settings(&stt);
        assert!(factory.create(&ctx, &missing_key).is_err());
    }

    #[test]
    fn create_accepts_keyless_compatible_servers_and_legacy_custom_profiles() {
        let factory = CloudFactory::new();
        let ctx = EngineContext {
            paths: AppPaths::under("unused"),
        };
        for provider in ["openai", "custom"] {
            let profile = ApiProfile {
                id: "compatible".into(),
                name: "호환 서버".into(),
                provider: provider.into(),
                endpoint: "http://127.0.0.1:8000/v1".into(),
                model: "my-whisper".into(),
                ..ApiProfile::default()
            };
            let stt = SttConfig {
                api_profile: profile.id.clone(),
                api_profiles: vec![profile.clone()],
                ..SttConfig::default()
            };
            let resolved = factory.resolve_settings(&stt);
            assert_eq!(resolved.str(keys::PROVIDER), provider);
            assert_eq!(resolved.str(keys::ENDPOINT), profile.endpoint);
            assert_eq!(resolved.str(keys::MODEL), profile.model);
            assert!(
                factory.create(&ctx, &resolved).is_ok(),
                "{provider}: 인증 없는 호환 서버를 쓸 수 있어야 합니다"
            );
        }
    }
}
