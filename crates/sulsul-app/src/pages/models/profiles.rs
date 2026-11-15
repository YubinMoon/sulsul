//! 외부 제공자(API 프로필)를 다루는 갈래 공통 규칙.
//!
//! 음성 인식 제공자(`sulsul_stt_cloud`)와 글 다듬기 제공자(`sulsul_llm`)는 공급자 프리셋과 요청 주소를
//! 만드는 규칙만 다르고, 새 제공자의 이름을 짓고 공급자를 바꾸는 방식은 같습니다. 화면 코드는
//! 여기를 거쳐 갈래에 맞는 프리셋을 봅니다. 화면을 그리지 않습니다.

use rust_i18n::t;
use sulsul_core::config::ApiProfile;
// ID와 이름을 겹치지 않게 짓는 규칙은 갈래와 상관없어 두 갈래가 같은 함수를 씁니다.
pub use sulsul_llm::entries::next_profile_id;
use sulsul_llm::entries::next_profile_name;
use sulsul_llm::presets::{CUSTOM_PROVIDER_ID, MOCK_PROVIDER_ID};

use super::Category;
use crate::i18n::tr;

/// 공급자를 고르는 목록에서 프리셋이 놓이는 묶음
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetKind {
    /// 인터넷의 클라우드 서비스
    Cloud,
    /// 이 PC에서 도는 서버
    Local,
    /// 주소를 직접 넣는 OpenAI 호환 서버
    Custom,
}

/// 공급자 프리셋 하나. 두 갈래의 프리셋을 같은 모양으로 봅니다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    /// 공급자 이름의 번역 키
    pub name_key: &'static str,
    pub default_endpoint: &'static str,
    pub default_model: &'static str,
    pub model_suggestions: &'static [&'static str],
    pub requires_api_key: bool,
    /// Ollama·LM Studio처럼 이 PC에서 도는 서버인지
    pub runs_locally: bool,
}

impl Preset {
    /// 개발 빌드에만 있는 가짜 제공자인지. 실제 요청을 보내지 않습니다.
    pub fn is_mock(&self, category: Category) -> bool {
        category == Category::Llm && self.id == MOCK_PROVIDER_ID
    }

    /// 공급자를 고르는 목록에서 놓일 묶음. 두 갈래 모두 직접 입력 프리셋의 ID는 `custom`입니다.
    pub fn kind(&self) -> PresetKind {
        if self.id == CUSTOM_PROVIDER_ID {
            PresetKind::Custom
        } else if self.runs_locally {
            PresetKind::Local
        } else {
            PresetKind::Cloud
        }
    }
}

impl From<&sulsul_stt_cloud::ProviderPreset> for Preset {
    fn from(preset: &sulsul_stt_cloud::ProviderPreset) -> Self {
        Self {
            id: preset.id,
            name_key: preset.name_key,
            default_endpoint: preset.default_endpoint,
            default_model: preset.default_model,
            model_suggestions: preset.model_suggestions,
            requires_api_key: preset.requires_api_key,
            runs_locally: false,
        }
    }
}

impl From<&sulsul_llm::presets::LlmProviderPreset> for Preset {
    fn from(preset: &sulsul_llm::presets::LlmProviderPreset) -> Self {
        Self {
            id: preset.id,
            name_key: preset.name_key,
            default_endpoint: preset.default_endpoint,
            default_model: preset.default_model,
            model_suggestions: preset.model_suggestions,
            requires_api_key: preset.requires_api_key,
            runs_locally: preset.runs_locally,
        }
    }
}

/// 새 제공자를 등록할 때 고를 수 있는 공급자 (화면 순서).
/// 글 다듬기의 가짜 제공자는 개발 빌드에서만 나옵니다.
pub fn presets(category: Category) -> Vec<Preset> {
    match category {
        Category::Stt => sulsul_stt_cloud::PRESETS.iter().map(Preset::from).collect(),
        Category::Llm => sulsul_llm::presets::presets(cfg!(debug_assertions))
            .into_iter()
            .map(Preset::from)
            .collect(),
    }
}

/// ID로 프리셋을 찾습니다. 이미 등록해 둔 제공자의 공급자도 알아봅니다.
pub fn find_preset(category: Category, id: &str) -> Option<Preset> {
    match category {
        Category::Stt => sulsul_stt_cloud::find_preset(id).map(Preset::from),
        Category::Llm => sulsul_llm::presets::find_preset(id).map(Preset::from),
    }
}

/// 공급자 프리셋의 이름. 모르는 공급자면 ID를 그대로 보여 줍니다.
pub fn provider_label(category: Category, provider: &str) -> String {
    find_preset(category, provider)
        .map_or_else(|| provider.to_owned(), |preset| tr(preset.name_key))
}

/// 공급자와 함께 알릴 것: 개발 빌드 전용 가짜 제공자이거나, 이 PC에서 켜 두어야 하는 서버일 때.
pub fn provider_note(category: Category, provider: &str) -> Option<String> {
    let preset = find_preset(category, provider)?;
    if preset.is_mock(category) {
        Some(t!("models.providers.mock_note").into_owned())
    } else if preset.runs_locally {
        Some(t!("models.providers.local_note").into_owned())
    } else {
        None
    }
}

/// 입력한 주소로 실제 요청을 보낼 주소. 쓸 수 없는 주소면 이유(영어)를 돌려줍니다.
pub fn request_url(category: Category, endpoint: &str) -> Result<String, String> {
    match category {
        Category::Stt => sulsul_stt_cloud::normalize_transcriptions_url(endpoint)
            .map_err(|error| error.to_string()),
        Category::Llm => sulsul_llm::presets::chat_completions_url(endpoint),
    }
}

/// 제공자의 이름. 이름을 비워 두었으면 ID를 보여 줍니다.
pub fn profile_display_name(profile: &ApiProfile) -> String {
    if profile.name.trim().is_empty() {
        profile.id.clone()
    } else {
        profile.name.clone()
    }
}

/// 공급자 프리셋의 기본값으로 새 제공자를 만듭니다. ID와 이름은 이미 있는 것과 겹치지 않게 정합니다.
pub fn new_profile(category: Category, existing: &[ApiProfile], provider: &str) -> ApiProfile {
    let id = next_profile_id(existing, provider);
    let base_name = t!(
        "models.providers.default_name",
        provider = provider_label(category, provider)
    )
    .into_owned();
    let name = next_profile_name(existing, &base_name);
    match category {
        Category::Stt => sulsul_stt_cloud::profile_from_preset(provider, id, name),
        Category::Llm => sulsul_llm::presets::profile_from_preset(provider, id, name),
    }
}

/// 공급자를 바꿨을 때, 사용자가 손대지 않은 주소·모델만 새 공급자의 기본값으로 바꿉니다.
pub fn switch_provider(
    category: Category,
    profile: &mut ApiProfile,
    old_provider: &str,
    new_provider: &str,
) {
    let Some(new_preset) = find_preset(category, new_provider) else {
        return;
    };
    let old_preset = find_preset(category, old_provider);
    let old_default_endpoint = old_preset.map(|preset| preset.default_endpoint);
    let old_default_model = old_preset.map(|preset| preset.default_model);

    if profile.endpoint.trim().is_empty()
        || old_default_endpoint.is_some_and(|default| profile.endpoint.trim() == default)
    {
        profile.endpoint = new_preset.default_endpoint.to_owned();
    }
    if profile.model.trim().is_empty()
        || old_default_model.is_some_and(|default| profile.model.trim() == default)
    {
        profile.model = new_preset.default_model.to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [Category; 2] = [Category::Stt, Category::Llm];

    #[test]
    fn every_listed_preset_can_be_found_by_its_id() {
        for category in BOTH {
            let listed = presets(category);
            assert!(!listed.is_empty(), "{category:?}: 공급자 목록이 비었습니다");
            for preset in &listed {
                assert_eq!(
                    find_preset(category, preset.id).as_ref(),
                    Some(preset),
                    "{category:?}/{}: 목록의 공급자를 ID로 찾지 못했습니다",
                    preset.id
                );
            }
        }
    }

    #[test]
    fn mock_provider_is_listed_only_for_text_polishing_in_dev_builds() {
        let has_mock = |category| {
            presets(category)
                .iter()
                .any(|preset| preset.is_mock(category))
        };
        assert!(
            !has_mock(Category::Stt),
            "음성 인식 공급자에는 가짜 제공자가 없습니다"
        );
        assert_eq!(
            has_mock(Category::Llm),
            cfg!(debug_assertions),
            "글 다듬기의 가짜 제공자는 개발 빌드에서만 보여야 합니다"
        );
    }

    #[test]
    fn presets_fall_into_cloud_local_and_custom_groups() {
        rust_i18n::set_locale("ko");
        // 음성 인식 공급자에는 이 PC에서 도는 서버가 없습니다.
        assert!(
            presets(Category::Stt)
                .iter()
                .all(|preset| preset.kind() != PresetKind::Local)
        );
        for category in BOTH {
            let custom = find_preset(category, "custom").unwrap();
            assert_eq!(custom.kind(), PresetKind::Custom, "{category:?}");
            assert_eq!(provider_note(category, "custom"), None);
            assert_eq!(provider_note(category, "openai"), None);
            assert_eq!(provider_note(category, "no-such-provider"), None);
        }

        // 글 다듬기: 이 PC에서 도는 서버는 따로 묶이고, 서버를 켜 두라는 안내가 붙습니다.
        let local: Vec<Preset> = presets(Category::Llm)
            .into_iter()
            .filter(|preset| preset.kind() == PresetKind::Local)
            .collect();
        assert!(!local.is_empty(), "이 PC에서 도는 서버 프리셋이 없습니다");
        for preset in local {
            assert!(
                provider_note(Category::Llm, preset.id).is_some(),
                "{}: 안내가 없습니다",
                preset.id
            );
        }
        // 가짜 제공자는 등록된 뒤에도(프리셋 목록에 없는 빌드에서도) 무엇인지 알립니다.
        let mock_note = provider_note(Category::Llm, MOCK_PROVIDER_ID);
        assert_eq!(
            mock_note.as_deref(),
            Some(&*t!("models.providers.mock_note"))
        );
    }

    #[test]
    fn new_profile_gets_an_unused_id_and_name() {
        rust_i18n::set_locale("ko");
        for category in BOTH {
            let first = new_profile(category, &[], "openai");
            assert_eq!(first.id, "openai");
            assert_eq!(first.provider, "openai");
            assert!(
                request_url(category, &first.endpoint).is_ok(),
                "{category:?}: 기본 주소로 요청 주소를 만들지 못했습니다"
            );

            let second = new_profile(category, std::slice::from_ref(&first), "openai");
            assert_eq!(second.id, "openai-2");
            assert_ne!(second.name, first.name, "이름이 겹치면 안 됩니다");
        }
    }

    #[test]
    fn request_url_follows_the_category() {
        let endpoint = "https://api.openai.com/v1";
        let stt = request_url(Category::Stt, endpoint).unwrap();
        let llm = request_url(Category::Llm, endpoint).unwrap();
        assert!(stt.ends_with("/audio/transcriptions"), "{stt}");
        assert!(llm.ends_with("/chat/completions"), "{llm}");
        for category in BOTH {
            assert!(
                request_url(category, "not a url").is_err(),
                "{category:?}: 쓸 수 없는 주소를 받아들였습니다"
            );
        }
    }

    #[test]
    fn provider_switch_updates_untouched_defaults_only() {
        rust_i18n::set_locale("ko");
        for category in BOTH {
            let mut profile = new_profile(category, &[], "openai");
            switch_provider(category, &mut profile, "openai", "groq");
            let groq = find_preset(category, "groq").unwrap();
            assert_eq!(profile.endpoint, groq.default_endpoint);
            assert_eq!(profile.model, groq.default_model);

            // 직접 넣은 값은 공급자를 바꿔도 그대로 둡니다.
            profile.endpoint = "https://my.server/v1".into();
            profile.model = "my-model".into();
            switch_provider(category, &mut profile, "groq", "custom");
            assert_eq!(profile.endpoint, "https://my.server/v1");
            assert_eq!(profile.model, "my-model");
        }
    }

    #[test]
    fn display_name_falls_back_to_the_id() {
        let mut profile = ApiProfile {
            id: "openai-2".into(),
            name: "  ".into(),
            ..ApiProfile::default()
        };
        assert_eq!(profile_display_name(&profile), "openai-2");
        profile.name = "회사 서버".into();
        assert_eq!(profile_display_name(&profile), "회사 서버");
    }
}
