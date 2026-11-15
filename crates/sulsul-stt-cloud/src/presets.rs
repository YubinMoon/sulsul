//! 클라우드 STT API 공급자 프리셋 (OpenAI 호환 `/audio/transcriptions`).

use sulsul_core::config::ApiProfile;

#[derive(Debug, Clone, Copy)]
pub struct ProviderPreset {
    pub id: &'static str,
    pub name_key: &'static str,
    pub default_endpoint: &'static str,
    pub default_model: &'static str,
    pub model_suggestions: &'static [&'static str],
    pub requires_api_key: bool,
}

pub const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "openai",
        name_key: "engine.cloud_api.providers.openai",
        default_endpoint: "https://api.openai.com/v1",
        default_model: "whisper-1",
        model_suggestions: &["whisper-1", "gpt-4o-mini-transcribe", "gpt-4o-transcribe"],
        requires_api_key: true,
    },
    ProviderPreset {
        id: "groq",
        name_key: "engine.cloud_api.providers.groq",
        default_endpoint: "https://api.groq.com/openai/v1",
        default_model: "whisper-large-v3-turbo",
        model_suggestions: &["whisper-large-v3-turbo", "whisper-large-v3"],
        requires_api_key: true,
    },
    ProviderPreset {
        id: "custom",
        name_key: "engine.cloud_api.providers.custom",
        default_endpoint: "http://127.0.0.1:8000/v1",
        default_model: "whisper-1",
        model_suggestions: &["whisper-1"],
        requires_api_key: false,
    },
];

pub fn find_preset(id: &str) -> Option<&'static ProviderPreset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// 선택한 공급자 프리셋에 맞는 기본 [`ApiProfile`]을 만듭니다.
pub fn profile_from_preset(provider: &str, id: String, name: String) -> ApiProfile {
    let preset = find_preset(provider).unwrap_or(&PRESETS[2]);
    ApiProfile {
        id,
        name,
        provider: preset.id.to_owned(),
        endpoint: preset.default_endpoint.to_owned(),
        api_key: String::new(),
        model: preset.default_model.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_provide_endpoints_and_models() {
        for preset in PRESETS {
            assert!(!preset.default_endpoint.is_empty());
            assert!(!preset.default_model.is_empty());
            assert!(find_preset(preset.id).is_some());
        }
        let groq = profile_from_preset("groq", "g1".into(), "내 Groq".into());
        assert_eq!(groq.provider, "groq");
        assert_eq!(groq.endpoint, "https://api.groq.com/openai/v1");
        assert_eq!(groq.model, "whisper-large-v3-turbo");
    }
}
