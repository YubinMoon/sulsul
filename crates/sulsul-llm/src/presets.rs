//! 글 다듬기 외부 제공자 프리셋 (모두 OpenAI 호환 `/chat/completions`).
//!
//! 주소와 모델 이름은 각 제공자의 공식 문서에서 확인한 값입니다 (마지막 확인: 2026-11-16).
//! 짧은 글 교정용이라 기본 모델은 빠르고 싼 것으로, 추천 목록에는 상위 모델도 함께 둡니다.
//! 제공자를 더하면 `locales/*.yml`의 `processor.llm_cloud.providers.<ID>` 문구도 추가합니다.

use sulsul_core::config::ApiProfile;

/// 외부 제공자 프리셋 하나
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LlmProviderPreset {
    pub id: &'static str,
    /// `processor.llm_cloud.providers.<id>`
    pub name_key: &'static str,
    /// OpenAI 호환 기본 URL (`.../v1` 등). [`chat_completions_url`]로 실제 요청 주소를 만듭니다.
    pub default_endpoint: &'static str,
    pub default_model: &'static str,
    pub model_suggestions: &'static [&'static str],
    pub requires_api_key: bool,
    /// Ollama·LM Studio처럼 이 PC에서 도는 서버
    pub runs_locally: bool,
}

/// 직접 입력하는 OpenAI 호환 서버 (목록의 마지막 항목)
pub const CUSTOM_PROVIDER_ID: &str = "custom";

/// 개발 빌드에서만 보이는 가짜 제공자. 네트워크 없이 등록 흐름을 확인하는 데 씁니다.
pub const MOCK_PROVIDER_ID: &str = "mock";

/// 가짜 제공자 프리셋. [`PRESETS`]에는 없고 [`presets`]`(true)`와 [`find_preset`]으로만 나옵니다.
pub const MOCK_PRESET: LlmProviderPreset = LlmProviderPreset {
    id: MOCK_PROVIDER_ID,
    name_key: "processor.llm_cloud.providers.mock",
    default_endpoint: "http://mock.invalid/v1",
    default_model: "mock-small",
    model_suggestions: &["mock-small", "mock-large"],
    requires_api_key: false,
    runs_locally: true,
};

/// 외부 제공자 프리셋 (화면 순서). 마지막 항목은 [`CUSTOM_PROVIDER_ID`]입니다.
pub const PRESETS: &[LlmProviderPreset] = &[
    // https://developers.openai.com/api/docs/models
    LlmProviderPreset {
        id: "openai",
        name_key: "processor.llm_cloud.providers.openai",
        default_endpoint: "https://api.openai.com/v1",
        default_model: "gpt-6-luna",
        model_suggestions: &["gpt-6-luna", "gpt-6.1-terra", "gpt-6.1-sol"],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://platform.claude.com/docs/en/api/openai-sdk
    LlmProviderPreset {
        id: "anthropic",
        name_key: "processor.llm_cloud.providers.anthropic",
        default_endpoint: "https://api.anthropic.com/v1",
        default_model: "claude-haiku-5-5",
        model_suggestions: &["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5"],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://ai.google.dev/gemini-api/docs/openai
    LlmProviderPreset {
        id: "gemini",
        name_key: "processor.llm_cloud.providers.gemini",
        default_endpoint: "https://generativelanguage.googleapis.com/v1beta/openai",
        default_model: "gemini-3.5-flash-lite",
        model_suggestions: &[
            "gemini-3.5-flash-lite",
            "gemini-3.8-flash",
            "gemini-4-hydrogen",
            "gemini-4-carbon",
        ],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://docs.x.ai/developers/models (Chat Completions는 문서상 legacy지만 계속 제공됨)
    LlmProviderPreset {
        id: "xai",
        name_key: "processor.llm_cloud.providers.xai",
        default_endpoint: "https://api.x.ai/v1",
        default_model: "grok-4.20-0309-non-reasoning",
        model_suggestions: &["grok-4.20-0309-non-reasoning", "grok-4.3", "grok-4.7"],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://docs.mistral.ai/getting-started/models/models_overview/
    LlmProviderPreset {
        id: "mistral",
        name_key: "processor.llm_cloud.providers.mistral",
        default_endpoint: "https://api.mistral.ai/v1",
        default_model: "mistral-small-latest",
        model_suggestions: &[
            "mistral-small-latest",
            "ministral-8b-latest",
            "mistral-large-latest",
        ],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://api-docs.deepseek.com/
    LlmProviderPreset {
        id: "deepseek",
        name_key: "processor.llm_cloud.providers.deepseek",
        default_endpoint: "https://api.deepseek.com/v1",
        default_model: "deepseek-flash",
        model_suggestions: &["deepseek-flash", "deepseek-v4-pro"],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://console.groq.com/docs/models
    LlmProviderPreset {
        id: "groq",
        name_key: "processor.llm_cloud.providers.groq",
        default_endpoint: "https://api.groq.com/openai/v1",
        default_model: "openai/gpt-oss-20b",
        model_suggestions: &[
            "openai/gpt-oss-20b",
            "openai/gpt-oss-120b",
            "qwen/qwen3.8-27b",
        ],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://openrouter.ai/docs/quickstart, 모델 목록: https://openrouter.ai/api/v1/models
    LlmProviderPreset {
        id: "openrouter",
        name_key: "processor.llm_cloud.providers.openrouter",
        default_endpoint: "https://openrouter.ai/api/v1",
        default_model: "anthropic/claude-haiku-5.5",
        model_suggestions: &[
            "anthropic/claude-haiku-5.5",
            "openai/gpt-6-luna",
            "google/gemini-3.5-flash-lite",
            "anthropic/claude-sonnet-5.5",
        ],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://docs.together.ai/docs/serverless-models
    LlmProviderPreset {
        id: "together",
        name_key: "processor.llm_cloud.providers.together",
        default_endpoint: "https://api.together.ai/v1",
        default_model: "Qwen/Qwen3.8-Flash",
        model_suggestions: &[
            "Qwen/Qwen3.8-Flash",
            "Qwen/Qwen3.5-9B",
            "deepseek-ai/DeepSeek-V4.1-Flash",
            "Qwen/Qwen3.7-Max",
        ],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://docs.fireworks.ai/tools-sdks/openai-compatibility, 모델: https://fireworks.ai/models
    LlmProviderPreset {
        id: "fireworks",
        name_key: "processor.llm_cloud.providers.fireworks",
        default_endpoint: "https://api.fireworks.ai/inference/v1",
        default_model: "accounts/fireworks/models/deepseek-v4p1-flash",
        model_suggestions: &[
            "accounts/fireworks/models/deepseek-v4p1-flash",
            "accounts/fireworks/models/glm-5p3-flash",
            "accounts/fireworks/models/kimi-k3p1-pro",
        ],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://inference-docs.cerebras.ai/models/overview
    LlmProviderPreset {
        id: "cerebras",
        name_key: "processor.llm_cloud.providers.cerebras",
        default_endpoint: "https://api.cerebras.ai/v1",
        default_model: "gpt-oss-120b",
        model_suggestions: &["gpt-oss-120b", "qwen-3.8-27b"],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://docs.cohere.com/docs/compatibility-api, 모델: https://docs.cohere.com/docs/models
    LlmProviderPreset {
        id: "cohere",
        name_key: "processor.llm_cloud.providers.cohere",
        default_endpoint: "https://api.cohere.ai/compatibility/v1",
        default_model: "command-r7b-12-2024",
        model_suggestions: &[
            "command-r7b-12-2024",
            "command-a-03-2025",
            "command-a-plus-05-2026",
        ],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://console.upstage.ai/docs/capabilities/chat
    LlmProviderPreset {
        id: "upstage",
        name_key: "processor.llm_cloud.providers.upstage",
        default_endpoint: "https://api.upstage.ai/v1",
        default_model: "solar-mini4",
        model_suggestions: &["solar-mini4", "solar-pro4"],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://api.ncloud-docs.com/docs/clovastudio-openaicompatibility
    LlmProviderPreset {
        id: "clova",
        name_key: "processor.llm_cloud.providers.clova",
        default_endpoint: "https://clovastudio.stream.ntruss.com/v1/openai",
        default_model: "HCX-DASH-002",
        model_suggestions: &["HCX-DASH-002", "HCX-005", "HCX-007"],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://www.alibabacloud.com/help/en/model-studio/compatibility-of-openai-with-dashscope
    // 문서는 작업 공간별 주소(`https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1`)를
    // 권하지만, 기본값은 작업 공간 ID 없이 쓸 수 있는 국제(싱가포르) 공용 주소로 둡니다.
    LlmProviderPreset {
        id: "alibaba",
        name_key: "processor.llm_cloud.providers.alibaba",
        default_endpoint: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        default_model: "qwen3.8-flash",
        model_suggestions: &["qwen3.8-flash", "qwen3.7-plus", "qwen3.8-max"],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://platform.kimi.ai/docs/api/chat
    LlmProviderPreset {
        id: "moonshot",
        name_key: "processor.llm_cloud.providers.moonshot",
        default_endpoint: "https://api.moonshot.ai/v1",
        default_model: "kimi-k3.1-pro",
        model_suggestions: &["kimi-k3.1-pro", "kimi-k3.1-pro-highspeed", "kimi-k3"],
        requires_api_key: true,
        runs_locally: false,
    },
    // https://docs.ollama.com/openai, 모델: https://ollama.com/library
    LlmProviderPreset {
        id: "ollama",
        name_key: "processor.llm_cloud.providers.ollama",
        default_endpoint: "http://localhost:11434/v1",
        default_model: "gemma4:e4b",
        model_suggestions: &["gemma4:e4b", "qwen3.5:4b", "qwen3.5:9b", "gemma4:12b"],
        requires_api_key: false,
        runs_locally: true,
    },
    // https://lmstudio.ai/docs/developer/openai-compat (모델 이름은 LM Studio에 보이는 식별자)
    LlmProviderPreset {
        id: "lmstudio",
        name_key: "processor.llm_cloud.providers.lmstudio",
        default_endpoint: "http://localhost:1234/v1",
        default_model: "google/gemma-4-e4b",
        model_suggestions: &["google/gemma-4-e4b", "qwen/qwen3.5-4b", "qwen/qwen3.5-9b"],
        requires_api_key: false,
        runs_locally: true,
    },
    // llama.cpp `llama-server`(기본 포트 8080), vLLM 등 OpenAI 호환 서버를 직접 입력합니다.
    // 서버마다 모델 이름이 달라 기본 모델은 비워 두고 사용자가 채웁니다.
    LlmProviderPreset {
        id: CUSTOM_PROVIDER_ID,
        name_key: "processor.llm_cloud.providers.custom",
        default_endpoint: "http://127.0.0.1:8080/v1",
        default_model: "",
        model_suggestions: &[],
        requires_api_key: false,
        runs_locally: false,
    },
];

/// 화면에 보일 프리셋 목록 ([`PRESETS`] 순서). `include_mock`이면 `custom` 바로 앞에
/// [`MOCK_PRESET`]을 넣습니다. 앱은 `cfg!(debug_assertions)`를 넘깁니다.
pub fn presets(include_mock: bool) -> Vec<&'static LlmProviderPreset> {
    let mut list: Vec<_> = PRESETS.iter().collect();
    if include_mock {
        let custom = list
            .iter()
            .position(|preset| preset.id == CUSTOM_PROVIDER_ID)
            .unwrap_or(list.len());
        list.insert(custom, &MOCK_PRESET);
    }
    list
}

/// ID로 프리셋을 찾습니다. 이미 등록된 가짜 프로필도 알아보도록 `mock`도 찾습니다.
pub fn find_preset(id: &str) -> Option<&'static LlmProviderPreset> {
    if id == MOCK_PROVIDER_ID {
        return Some(&MOCK_PRESET);
    }
    PRESETS.iter().find(|preset| preset.id == id)
}

/// 고른 제공자 프리셋의 기본값으로 [`ApiProfile`]을 만듭니다. 모르는 제공자면 `custom`을 씁니다.
pub fn profile_from_preset(provider: &str, id: String, name: String) -> ApiProfile {
    let preset = find_preset(provider).unwrap_or_else(custom_preset);
    ApiProfile {
        id,
        name,
        provider: preset.id.to_owned(),
        endpoint: preset.default_endpoint.to_owned(),
        api_key: String::new(),
        model: preset.default_model.to_owned(),
    }
}

fn custom_preset() -> &'static LlmProviderPreset {
    PRESETS
        .iter()
        .find(|preset| preset.id == CUSTOM_PROVIDER_ID)
        .unwrap_or(&PRESETS[PRESETS.len() - 1])
}

/// 프로필의 제공자에 API 키가 꼭 필요한지. 모르는 제공자는 키 없이도 보냅니다.
pub fn requires_api_key(provider: &str) -> bool {
    find_preset(provider).is_some_and(|preset| preset.requires_api_key)
}

/// 기본 URL이나 전체 주소를 실제 요청할 `/chat/completions` 주소로 바꿉니다.
///
/// - `.../chat/completions`로 끝나면 그대로 씁니다.
/// - 경로가 있으면(`https://api.openai.com/v1`) 뒤에 `/chat/completions`를 붙입니다.
/// - 경로가 없으면(`http://localhost:11434`) `/v1/chat/completions`를 붙입니다.
///
/// 쓸 수 없는 주소면 영어 이유를 담은 `Err`를 돌려줍니다.
pub fn chat_completions_url(endpoint: &str) -> Result<String, String> {
    const SUFFIX: &str = "/chat/completions";
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("endpoint is empty".into());
    }
    let after_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .ok_or_else(|| format!("endpoint must start with http:// or https://: {trimmed}"))?;
    let host = after_scheme.split('/').next().unwrap_or_default();
    if host.is_empty() || host.chars().any(char::is_whitespace) {
        return Err(format!("endpoint has no valid host: {trimmed}"));
    }
    if trimmed.ends_with(SUFFIX) {
        return Ok(trimmed.to_owned());
    }
    if after_scheme.contains('/') {
        Ok(format!("{trimmed}{SUFFIX}"))
    } else {
        Ok(format!("{trimmed}/v1{SUFFIX}"))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn presets_are_well_formed() {
        let mut ids = HashSet::new();
        for preset in PRESETS {
            assert!(ids.insert(preset.id), "{}: 프리셋 ID가 겹칩니다", preset.id);
            assert!(
                !preset.id.contains('.'),
                "{}: ID에 점이 있습니다",
                preset.id
            );
            assert_eq!(
                preset.name_key,
                format!("processor.llm_cloud.providers.{}", preset.id)
            );
            assert!(
                chat_completions_url(preset.default_endpoint).is_ok(),
                "{}: 기본 주소를 쓸 수 없습니다",
                preset.id
            );
            assert_eq!(find_preset(preset.id), Some(preset));
            if preset.id == CUSTOM_PROVIDER_ID {
                continue;
            }
            assert!(!preset.default_model.is_empty(), "{}", preset.id);
            assert!(
                (2..=4).contains(&preset.model_suggestions.len()),
                "{}: 추천 모델은 2~4개여야 합니다",
                preset.id
            );
            assert_eq!(
                preset.model_suggestions.first(),
                Some(&preset.default_model),
                "{}: 추천 목록의 첫 항목은 기본 모델이어야 합니다",
                preset.id
            );
        }
        assert_eq!(
            PRESETS.last().map(|p| p.id),
            Some(CUSTOM_PROVIDER_ID),
            "마지막 프리셋은 custom이어야 합니다"
        );
        assert!(
            !ids.contains(MOCK_PROVIDER_ID),
            "PRESETS에 mock이 있으면 안 됩니다"
        );
    }

    #[test]
    fn local_servers_need_no_key() {
        for id in ["ollama", "lmstudio"] {
            let preset = find_preset(id).unwrap();
            assert!(preset.runs_locally && !preset.requires_api_key, "{id}");
        }
        for preset in PRESETS.iter().filter(|p| p.runs_locally) {
            assert!(!preset.requires_api_key, "{}", preset.id);
        }
        assert!(requires_api_key("openai"));
        assert!(!requires_api_key("custom"));
        assert!(!requires_api_key("unknown-provider"));
    }

    #[test]
    fn mock_preset_is_listed_only_when_asked() {
        let ids = |list: Vec<&LlmProviderPreset>| list.iter().map(|p| p.id).collect::<Vec<_>>();
        let without = ids(presets(false));
        assert_eq!(without, PRESETS.iter().map(|p| p.id).collect::<Vec<_>>());
        assert!(!without.contains(&MOCK_PROVIDER_ID));

        let with = ids(presets(true));
        assert_eq!(with.len(), PRESETS.len() + 1);
        assert_eq!(
            &with[with.len() - 2..],
            [MOCK_PROVIDER_ID, CUSTOM_PROVIDER_ID],
            "mock은 custom 바로 앞에 있어야 합니다"
        );
        assert_eq!(find_preset(MOCK_PROVIDER_ID), Some(&MOCK_PRESET));
        assert_eq!(MOCK_PRESET.name_key, "processor.llm_cloud.providers.mock");
        let mock = find_preset(MOCK_PROVIDER_ID).unwrap();
        assert!(
            !mock.requires_api_key && mock.runs_locally,
            "가짜 제공자는 키 없이 이 PC에서 도는 것으로 봅니다"
        );
    }

    #[test]
    fn profile_from_preset_fills_defaults() {
        let groq = profile_from_preset("groq", "g1".into(), "내 Groq".into());
        assert_eq!(groq.provider, "groq");
        assert_eq!(groq.endpoint, "https://api.groq.com/openai/v1");
        assert_eq!(groq.model, "openai/gpt-oss-20b");
        assert!(groq.api_key.is_empty());

        let unknown = profile_from_preset("nope", "x".into(), "X".into());
        assert_eq!(
            unknown.provider, CUSTOM_PROVIDER_ID,
            "모르는 제공자는 custom이어야 합니다"
        );

        let mock = profile_from_preset(MOCK_PROVIDER_ID, "m".into(), "M".into());
        assert_eq!(mock.endpoint, "http://mock.invalid/v1");
        assert_eq!(mock.model, "mock-small");
    }

    #[test]
    fn chat_completions_url_accepts_base_and_full_urls() {
        let cases = [
            (
                "https://api.openai.com/v1",
                "https://api.openai.com/v1/chat/completions",
            ),
            (
                "https://api.openai.com/v1/",
                "https://api.openai.com/v1/chat/completions",
            ),
            (
                " https://generativelanguage.googleapis.com/v1beta/openai/ ",
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
            ),
            (
                "http://localhost:11434",
                "http://localhost:11434/v1/chat/completions",
            ),
            (
                "https://proxy.example.com/v1/chat/completions",
                "https://proxy.example.com/v1/chat/completions",
            ),
            (
                "https://proxy.example.com/v1/chat/completions/",
                "https://proxy.example.com/v1/chat/completions",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(
                chat_completions_url(input).as_deref(),
                Ok(expected),
                "{input}"
            );
        }
        for bad in [
            "",
            "   ",
            "ftp://bad",
            "api.openai.com/v1",
            "https://",
            "https:///v1",
        ] {
            assert!(
                chat_completions_url(bad).is_err(),
                "{bad:?}는 거부해야 합니다"
            );
        }
    }
}
