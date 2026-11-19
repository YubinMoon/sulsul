//! 글 다듬기 "사용 가능 모델" 목록의 항목과, 항목을 고르거나 지울 때 설정을 바꾸는 규칙.
//!
//! 화면을 그리지 않습니다. 목록에 무엇이 들어가고 어느 것이 골라져 있는지는 설정과 모델 파일
//! 상태만으로 정해지므로, 여기서 계산해 화면 코드와 테스트가 함께 씁니다.
//! 음성 인식 쪽 규칙(`sulsul-app`의 `pages/models/entries.rs`)과 같은 성질을 지킵니다.
//!
//! `[postprocess] enabled`(다듬기를 쓸지)는 여기서 바꾸지 않습니다. 여기서는 무엇으로 다듬을지만 고릅니다.

use sulsul_core::AppPaths;
use sulsul_core::config::{ApiProfile, PostProcessConfig, ReasoningEffort};
use sulsul_core::stt::ModelVariant;
use sulsul_core::toml::Value;

use crate::catalog::{LlmFamily, families, find_variant};
use crate::codex::{self, auth::CodexAccount};
use crate::presets::{chat_completions_url, requires_api_key};
use crate::{CLOUD_PROCESSOR_ID, CODEX_PROCESSOR_ID, LOCAL_MODEL_KEY, LOCAL_PROCESSOR_ID};

/// 목록의 항목 하나를 가리키는 키
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LlmEntryKey {
    /// 설치된 로컬 모델
    Local { variant_id: String },
    /// Codex 로그인 (ChatGPT 계정). 로그인은 하나뿐입니다.
    Codex,
    /// 등록한 외부 제공자 (API 프로필)
    Provider { profile_id: String },
}

impl LlmEntryKey {
    pub fn local(variant_id: &str) -> Self {
        Self::Local {
            variant_id: variant_id.to_owned(),
        }
    }

    pub fn provider(profile_id: &str) -> Self {
        Self::Provider {
            profile_id: profile_id.to_owned(),
        }
    }
}

/// 항목의 종류별 내용
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmEntryKind {
    Local {
        family: LlmFamily,
        variant: ModelVariant,
    },
    Codex(CodexEntry),
    Provider(ApiProfile),
}

/// Codex 로그인 항목의 내용
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexEntry {
    /// 로그인한 계정
    pub account: CodexAccount,
    /// 고른 모델 ([`codex::chosen_model`])
    pub model: String,
    /// 고른 추론 단계
    pub effort: ReasoningEffort,
}

/// 목록의 항목 하나
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmEntry {
    pub key: LlmEntryKey,
    pub kind: LlmEntryKind,
    /// 지금 설정이 이 항목을 가리키는지
    pub selected: bool,
    /// 지금 바로 다듬기에 쓸 수 있는지 (파일이 있고, 꼭 필요한 값이 채워져 있음)
    pub ready: bool,
}

/// 설정에서 고른 로컬 모델(변형 ID). 없으면 빈 문자열입니다.
pub fn chosen_local_model(config: &PostProcessConfig) -> &str {
    config
        .processors
        .get(LOCAL_PROCESSOR_ID)
        .and_then(|table| table.get(LOCAL_MODEL_KEY))
        .and_then(Value::as_str)
        .map_or("", str::trim)
}

/// 실제로 쓸 수 있는 항목 목록: 설치된 로컬 모델(카탈로그 순), Codex 로그인(로그인했을 때),
/// 등록된 제공자(등록 순)
pub fn available_entries(paths: &AppPaths, config: &PostProcessConfig) -> Vec<LlmEntry> {
    let processor = config.processor.trim();
    let local_active = processor == LOCAL_PROCESSOR_ID;
    let chosen = chosen_local_model(config);
    let mut entries = Vec::new();

    for family in families(paths) {
        for variant in family.variants.iter().filter(|v| v.is_installed()) {
            entries.push(LlmEntry {
                key: LlmEntryKey::local(&variant.id),
                selected: local_active && chosen == variant.id,
                ready: true,
                kind: LlmEntryKind::Local {
                    family: family.clone(),
                    variant: variant.clone(),
                },
            });
        }
    }

    if let Some(account) = codex::auth::account(paths) {
        entries.push(LlmEntry {
            key: LlmEntryKey::Codex,
            selected: processor == CODEX_PROCESSOR_ID,
            ready: true,
            kind: LlmEntryKind::Codex(CodexEntry {
                account,
                model: codex::chosen_model(config).to_owned(),
                effort: codex::chosen_effort(config),
            }),
        });
    }

    let cloud_active = processor == CLOUD_PROCESSOR_ID;
    let active_profile = config.active_api_profile().map(|p| p.id.as_str());
    let mut seen: Vec<&str> = Vec::new();
    for profile in &config.api_profiles {
        // ID가 겹치는 프로필은 앞의 것만 쓰이므로(설정 파일을 직접 고친 경우) 뒤의 것은 뺍니다.
        if seen.contains(&profile.id.as_str()) {
            continue;
        }
        seen.push(&profile.id);
        entries.push(LlmEntry {
            key: LlmEntryKey::provider(&profile.id),
            selected: cloud_active && active_profile == Some(profile.id.as_str()),
            ready: profile_ready(profile),
            kind: LlmEntryKind::Provider(profile.clone()),
        });
    }
    entries
}

/// 지금 설정이 가리키는 항목을 바로 쓸 수 있는지
pub fn selection_ready(entries: &[LlmEntry]) -> bool {
    entries.iter().any(|entry| entry.selected && entry.ready)
}

/// 항목을 고릅니다. 고른 항목 하나만 켜지게 `processor`·`api_profile`·
/// `[postprocess.processors.llm_local] model`을 바꿉니다. Codex를 고르면 쓸 모델도 설정에 적어 둡니다.
pub fn activate(config: &mut PostProcessConfig, key: &LlmEntryKey) {
    match key {
        LlmEntryKey::Local { variant_id } => {
            config.processor = LOCAL_PROCESSOR_ID.to_owned();
            config
                .processors
                .entry(LOCAL_PROCESSOR_ID.to_owned())
                .or_default()
                .insert(
                    LOCAL_MODEL_KEY.to_owned(),
                    Value::String(variant_id.clone()),
                );
        }
        LlmEntryKey::Codex => {
            config.processor = CODEX_PROCESSOR_ID.to_owned();
            let model = codex::chosen_model(config).to_owned();
            codex::set_model(config, &model);
        }
        LlmEntryKey::Provider { profile_id } => {
            config.processor = CLOUD_PROCESSOR_ID.to_owned();
            config.api_profile.clone_from(profile_id);
        }
    }
}

/// 준비된 첫 항목을 고릅니다. 고른 항목이 있으면 `true`, 없으면 선택을 비우고 `false`입니다.
pub fn activate_first_ready(entries: &[LlmEntry], config: &mut PostProcessConfig) -> bool {
    match entries.iter().find(|entry| entry.ready) {
        Some(entry) => {
            activate(config, &entry.key);
            true
        }
        None => {
            clear_selection(config);
            false
        }
    }
}

/// 아무것도 고르지 않은 상태로 만듭니다 (`processor = ""`, `api_profile = ""`).
///
/// 로컬 모델 키(`model`)는 지우지 않습니다. `processor`가 비어 있으면 쓰이지 않고, 키를 지우면
/// 저장할 때 파일에 남은 옛 값이 되살아나기 때문입니다(`sulsul_core::config`의 병합 규칙).
fn clear_selection(config: &mut PostProcessConfig) {
    config.processor.clear();
    config.api_profile.clear();
}

/// 제공자를 지웁니다. 그게 고른 항목이었으면 준비된 첫 항목으로 바꾸거나 선택을 비웁니다.
pub fn remove_profile(paths: &AppPaths, config: &mut PostProcessConfig, profile_id: &str) {
    let was_selected = config.processor.trim() == CLOUD_PROCESSOR_ID
        && config.active_api_profile().map(|p| p.id.as_str()) == Some(profile_id);
    config
        .api_profiles
        .retain(|profile| profile.id != profile_id);
    if config.api_profile.trim() == profile_id {
        config.api_profile.clear();
    }
    if was_selected {
        let entries = available_entries(paths, config);
        activate_first_ready(&entries, config);
    }
}

/// Codex에서 로그아웃한 뒤 부릅니다. Codex를 쓰고 있었으면 준비된 첫 항목으로 바꾸거나 선택을 비웁니다.
pub fn on_codex_signed_out(paths: &AppPaths, config: &mut PostProcessConfig) {
    if config.processor.trim() == CODEX_PROCESSOR_ID {
        let entries = available_entries(paths, config);
        activate_first_ready(&entries, config);
    }
}

/// 로컬 모델 파일을 지운 뒤 부릅니다. 그게 고른 항목이었으면 준비된 첫 항목으로 바꾸거나 선택을 비웁니다.
pub fn on_local_removed(paths: &AppPaths, config: &mut PostProcessConfig, variant_id: &str) {
    if config.processor.trim() == LOCAL_PROCESSOR_ID && chosen_local_model(config) == variant_id {
        let entries = available_entries(paths, config);
        activate_first_ready(&entries, config);
    }
}

/// 제공자가 요청을 보낼 수 있을 만큼 채워졌는지 (주소·모델, 필요하면 API 키)
pub fn profile_ready(profile: &ApiProfile) -> bool {
    if chat_completions_url(&profile.endpoint).is_err() || profile.model.trim().is_empty() {
        return false;
    }
    !requires_api_key(&profile.provider) || !profile.api_key.trim().is_empty()
}

/// 지금 설정이 가리키는 것을 쓸 수 없는 이유
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionIssue {
    /// 고른 로컬 모델이 아직 설치되지 않았습니다.
    NotInstalled { variant_id: String },
    /// 카탈로그에 없는 로컬 모델을 골랐습니다 (비어 있는 경우 포함).
    UnknownModel { variant_id: String },
    /// 외부 제공자를 쓰도록 되어 있는데 고른 제공자가 없습니다.
    NoProvider,
    /// 고른 제공자의 값이 덜 채워졌습니다.
    ProviderIncomplete { profile_id: String },
    /// Codex를 쓰도록 되어 있는데 로그인하지 않았습니다 (로그아웃했거나 로그인이 만료됨).
    CodexSignedOut,
    /// 알 수 없는 후처리기 ID입니다.
    UnknownProcessor { id: String },
}

/// 지금 설정이 가리키는 것을 바로 쓸 수 없으면 그 이유를 돌려줍니다.
/// 아무것도 고르지 않았으면(`processor = ""`) `None`입니다 — 다듬기는 선택 사항입니다.
pub fn selection_issue(paths: &AppPaths, config: &PostProcessConfig) -> Option<SelectionIssue> {
    match config.processor.trim() {
        "" => None,
        LOCAL_PROCESSOR_ID => {
            let chosen = chosen_local_model(config);
            match find_variant(paths, chosen) {
                None => Some(SelectionIssue::UnknownModel {
                    variant_id: chosen.to_owned(),
                }),
                Some((_, variant)) if !variant.is_installed() => {
                    Some(SelectionIssue::NotInstalled {
                        variant_id: variant.id,
                    })
                }
                Some(_) => None,
            }
        }
        CLOUD_PROCESSOR_ID => match config.active_api_profile() {
            None => Some(SelectionIssue::NoProvider),
            Some(profile) if !profile_ready(profile) => Some(SelectionIssue::ProviderIncomplete {
                profile_id: profile.id.clone(),
            }),
            Some(_) => None,
        },
        CODEX_PROCESSOR_ID => codex::auth::account(paths)
            .is_none()
            .then_some(SelectionIssue::CodexSignedOut),
        other => Some(SelectionIssue::UnknownProcessor {
            id: other.to_owned(),
        }),
    }
}

/// `prefix`로 시작하고 `existing`과 겹치지 않는 ID (`openai`, `openai-2`, …)
pub fn next_profile_id(existing: &[ApiProfile], prefix: &str) -> String {
    let clean_prefix: String = prefix
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    let base = if clean_prefix.is_empty() {
        "profile"
    } else {
        clean_prefix.as_str()
    };
    (1usize..)
        .map(|n| {
            if n == 1 {
                base.to_owned()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|candidate| !existing.iter().any(|p| &p.id == candidate))
        .unwrap_or_else(|| base.to_owned())
}

/// `base_name`이 이미 있으면 뒤에 번호를 붙입니다 (`내 OpenAI 2`, …).
pub fn next_profile_name(existing: &[ApiProfile], base_name: &str) -> String {
    (1usize..)
        .map(|n| {
            if n == 1 {
                base_name.to_owned()
            } else {
                format!("{base_name} {n}")
            }
        })
        .find(|candidate| !existing.iter().any(|p| &p.name == candidate))
        .unwrap_or_else(|| base_name.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sulsul_core::{Config, ConfigStore};

    use super::*;
    use crate::catalog::{find_variant, models_root};
    use crate::presets::{MOCK_PROVIDER_ID, profile_from_preset};

    const SMALL: &str = "qwen3_5-2b-q4_k_m";
    const MEDIUM: &str = "gemma4-e4b-q4_k_m";

    struct Fixture {
        _dir: tempfile::TempDir,
        paths: AppPaths,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let paths = AppPaths::under(dir.path());
            Self { _dir: dir, paths }
        }

        fn entries(&self, config: &PostProcessConfig) -> Vec<LlmEntry> {
            available_entries(&self.paths, config)
        }

        /// 카탈로그 모델의 파일을 빈 내용으로 만들어 설치된 것처럼 꾸밉니다.
        fn install(&self, variant_id: &str) {
            let (_, variant) = find_variant(&self.paths, variant_id).unwrap();
            for file in &variant.files {
                fs::create_dir_all(file.path.parent().unwrap()).unwrap();
                fs::write(&file.path, b"gguf").unwrap();
            }
        }

        /// 앱의 지우기 흐름과 같이 파일을 지운 뒤 [`on_local_removed`]를 부릅니다.
        fn uninstall(&self, config: &mut PostProcessConfig, variant_id: &str) {
            let (family, variant) = find_variant(&self.paths, variant_id).unwrap();
            sulsul_core::stt::delete_variant(&variant, &family.variants, &models_root(&self.paths))
                .unwrap();
            on_local_removed(&self.paths, config, variant_id);
        }
    }

    fn profile(id: &str) -> ApiProfile {
        let mut profile = profile_from_preset("openai", id.into(), format!("이름 {id}"));
        profile.api_key = "sk-test".into();
        profile
    }

    fn selected_keys(entries: &[LlmEntry]) -> Vec<&LlmEntryKey> {
        entries
            .iter()
            .filter(|entry| entry.selected)
            .map(|entry| &entry.key)
            .collect()
    }

    #[test]
    fn lists_installed_local_models_then_providers_in_order() {
        let fixture = Fixture::new();
        // 설치 순서와 상관없이 카탈로그 순서여야 합니다.
        fixture.install(MEDIUM);
        fixture.install(SMALL);
        let mut config = PostProcessConfig::default();
        config.api_profiles.push(profile("b"));
        config.api_profiles.push(profile("a"));

        let keys: Vec<_> = fixture
            .entries(&config)
            .into_iter()
            .map(|e| e.key)
            .collect();
        assert_eq!(
            keys,
            [
                LlmEntryKey::local(SMALL),
                LlmEntryKey::local(MEDIUM),
                LlmEntryKey::provider("b"),
                LlmEntryKey::provider("a"),
            ],
            "설치된 로컬 모델(카탈로그 순) 다음 제공자(등록 순)여야 합니다"
        );
        assert!(
            selected_keys(&fixture.entries(&config)).is_empty(),
            "아무것도 고르지 않았으면 선택된 항목이 없어야 합니다"
        );
    }

    #[test]
    fn local_models_that_are_not_installed_are_not_listed() {
        let fixture = Fixture::new();
        let config = PostProcessConfig::default();
        assert!(fixture.entries(&config).is_empty());

        // 내려받다 멈춘 조각 파일만 있어도 설치된 것이 아닙니다.
        let (_, variant) = find_variant(&fixture.paths, SMALL).unwrap();
        let path = variant.primary_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(sulsul_core::download::part_path(path), b"part").unwrap();
        assert!(fixture.entries(&config).is_empty());
    }

    #[test]
    fn exactly_one_entry_is_selected_after_each_activation() {
        let fixture = Fixture::new();
        fixture.install(SMALL);
        fixture.install(MEDIUM);
        let mut config = PostProcessConfig::default();
        config.api_profiles.push(profile("openai"));
        config.api_profiles.push(profile("groq"));

        let keys: Vec<_> = fixture
            .entries(&config)
            .into_iter()
            .map(|e| e.key)
            .collect();
        assert_eq!(keys.len(), 4);
        for key in keys.iter().chain(keys.iter().rev()) {
            activate(&mut config, key);
            let entries = fixture.entries(&config);
            assert_eq!(
                selected_keys(&entries),
                [key],
                "{key:?}를 고른 뒤에는 그 항목만 골라져 있어야 합니다"
            );
            assert!(selection_ready(&entries));
            assert_eq!(selection_issue(&fixture.paths, &config), None);
        }
    }

    #[test]
    fn activate_writes_processor_profile_and_local_model() {
        let mut config = PostProcessConfig::default();
        activate(&mut config, &LlmEntryKey::local(SMALL));
        assert_eq!(config.processor, LOCAL_PROCESSOR_ID);
        assert_eq!(chosen_local_model(&config), SMALL);
        assert_eq!(
            config.processors[LOCAL_PROCESSOR_ID].get(LOCAL_MODEL_KEY),
            Some(&Value::String(SMALL.into()))
        );

        activate(&mut config, &LlmEntryKey::provider("p1"));
        assert_eq!(config.processor, CLOUD_PROCESSOR_ID);
        assert_eq!(config.api_profile, "p1");
        assert_eq!(
            chosen_local_model(&config),
            SMALL,
            "로컬 선택 값은 남겨 둡니다"
        );
    }

    #[test]
    fn incomplete_provider_is_listed_but_not_ready() {
        let fixture = Fixture::new();
        let mut config = PostProcessConfig::default();
        let mut missing_key = profile("no-key");
        missing_key.api_key.clear();
        let mut missing_model = profile("no-model");
        missing_model.model = " ".into();
        let mut bad_endpoint = profile("bad-endpoint");
        bad_endpoint.endpoint = "api.openai.com".into();
        config.api_profiles = vec![missing_key, missing_model, bad_endpoint];

        let entries = fixture.entries(&config);
        assert_eq!(
            entries.len(),
            3,
            "덜 채워진 제공자도 목록에는 있어야 합니다"
        );
        assert!(entries.iter().all(|e| !e.ready));

        activate(&mut config, &LlmEntryKey::provider("no-key"));
        let entries = fixture.entries(&config);
        assert_eq!(selected_keys(&entries), [&LlmEntryKey::provider("no-key")]);
        assert!(!selection_ready(&entries));
        assert_eq!(
            selection_issue(&fixture.paths, &config),
            Some(SelectionIssue::ProviderIncomplete {
                profile_id: "no-key".into()
            })
        );
    }

    #[test]
    fn keyless_presets_and_mock_are_ready_without_a_key() {
        for provider in ["ollama", "lmstudio", MOCK_PROVIDER_ID] {
            let profile = profile_from_preset(provider, "x".into(), "X".into());
            assert!(
                profile_ready(&profile),
                "{provider}는 키 없이 준비돼야 합니다"
            );
        }
        let custom = profile_from_preset("custom", "c".into(), "C".into());
        assert!(
            !profile_ready(&custom),
            "custom은 모델 이름을 채워야 준비됩니다"
        );
        let openai = profile_from_preset("openai", "o".into(), "O".into());
        assert!(!profile_ready(&openai), "OpenAI는 키가 필요합니다");
    }

    #[test]
    fn removing_the_selected_profile_falls_back_to_the_first_ready_entry() {
        let fixture = Fixture::new();
        fixture.install(MEDIUM);
        let mut config = PostProcessConfig::default();
        let mut incomplete = profile("incomplete");
        incomplete.api_key.clear();
        config.api_profiles = vec![incomplete, profile("openai"), profile("groq")];
        activate(&mut config, &LlmEntryKey::provider("groq"));

        // 준비된 첫 항목은 설치된 로컬 모델입니다.
        remove_profile(&fixture.paths, &mut config, "groq");
        assert_eq!(
            selected_keys(&fixture.entries(&config)),
            [&LlmEntryKey::local(MEDIUM)]
        );
        assert_eq!(config.api_profile, "", "지운 프로필을 가리키면 안 됩니다");

        // 로컬 모델이 없으면 덜 채워진 제공자를 건너뛰고 준비된 제공자를 고릅니다.
        fixture.uninstall(&mut config, MEDIUM);
        assert_eq!(
            selected_keys(&fixture.entries(&config)),
            [&LlmEntryKey::provider("openai")]
        );

        // 준비된 항목이 없으면 선택을 비웁니다.
        remove_profile(&fixture.paths, &mut config, "openai");
        assert_eq!(config.processor, "");
        assert_eq!(config.api_profile, "");
        assert!(selected_keys(&fixture.entries(&config)).is_empty());
        assert_eq!(selection_issue(&fixture.paths, &config), None);
        assert_eq!(
            config.api_profiles.len(),
            1,
            "덜 채워진 프로필은 남아 있어야 합니다"
        );
    }

    #[test]
    fn removing_an_unselected_profile_keeps_the_selection() {
        let fixture = Fixture::new();
        fixture.install(SMALL);
        let mut config = PostProcessConfig {
            api_profiles: vec![profile("openai"), profile("groq")],
            ..PostProcessConfig::default()
        };
        activate(&mut config, &LlmEntryKey::provider("openai"));
        activate(&mut config, &LlmEntryKey::local(SMALL));

        remove_profile(&fixture.paths, &mut config, "openai");
        assert_eq!(config.processor, LOCAL_PROCESSOR_ID);
        assert_eq!(config.api_profile, "", "지운 프로필 ID를 남기면 안 됩니다");
        assert_eq!(
            selected_keys(&fixture.entries(&config)),
            [&LlmEntryKey::local(SMALL)]
        );

        // 비어 있는 api_profile은 첫 프로필을 뜻하므로, 그 프로필을 지우면 고른 항목이 바뀝니다.
        activate(&mut config, &LlmEntryKey::provider(""));
        remove_profile(&fixture.paths, &mut config, "groq");
        assert_eq!(
            selected_keys(&fixture.entries(&config)),
            [&LlmEntryKey::local(SMALL)]
        );
    }

    #[test]
    fn removing_the_selected_local_model_falls_back_or_clears() {
        let fixture = Fixture::new();
        fixture.install(SMALL);
        fixture.install(MEDIUM);
        let mut config = PostProcessConfig::default();
        activate(&mut config, &LlmEntryKey::local(MEDIUM));

        // 고르지 않은 모델을 지우면 그대로입니다.
        fixture.uninstall(&mut config, SMALL);
        assert_eq!(chosen_local_model(&config), MEDIUM);

        fixture.install(SMALL);
        fixture.uninstall(&mut config, MEDIUM);
        assert_eq!(
            selected_keys(&fixture.entries(&config)),
            [&LlmEntryKey::local(SMALL)]
        );

        fixture.uninstall(&mut config, SMALL);
        assert_eq!(config.processor, "");
        assert!(fixture.entries(&config).is_empty());
        assert!(
            models_root(&fixture.paths).is_dir(),
            "모델을 모두 지워도 로컬 LLM 폴더 자체는 남아야 합니다"
        );
    }

    #[test]
    fn nothing_selected_has_no_issue() {
        let fixture = Fixture::new();
        let mut config = PostProcessConfig::default();
        assert_eq!(selection_issue(&fixture.paths, &config), None);
        assert!(!activate_first_ready(
            &fixture.entries(&config),
            &mut config
        ));
        assert_eq!(config, PostProcessConfig::default());
    }

    #[test]
    fn selection_issue_explains_each_problem() {
        let fixture = Fixture::new();
        let mut config = PostProcessConfig::default();

        activate(&mut config, &LlmEntryKey::local(SMALL));
        assert_eq!(
            selection_issue(&fixture.paths, &config),
            Some(SelectionIssue::NotInstalled {
                variant_id: SMALL.into()
            })
        );
        fixture.install(SMALL);
        assert_eq!(selection_issue(&fixture.paths, &config), None);

        activate(&mut config, &LlmEntryKey::local("no-such-model"));
        assert_eq!(
            selection_issue(&fixture.paths, &config),
            Some(SelectionIssue::UnknownModel {
                variant_id: "no-such-model".into()
            })
        );
        config.processors.clear();
        assert_eq!(
            selection_issue(&fixture.paths, &config),
            Some(SelectionIssue::UnknownModel {
                variant_id: String::new()
            })
        );

        config.processor = CLOUD_PROCESSOR_ID.into();
        assert_eq!(
            selection_issue(&fixture.paths, &config),
            Some(SelectionIssue::NoProvider)
        );
        config.api_profiles.push(profile("p1"));
        assert_eq!(
            selection_issue(&fixture.paths, &config),
            None,
            "빈 ID는 첫 프로필입니다"
        );
        config.api_profile = "gone".into();
        assert_eq!(
            selection_issue(&fixture.paths, &config),
            Some(SelectionIssue::NoProvider)
        );

        config.processor = "llm_magic".into();
        assert_eq!(
            selection_issue(&fixture.paths, &config),
            Some(SelectionIssue::UnknownProcessor {
                id: "llm_magic".into()
            })
        );
    }

    #[test]
    fn duplicate_profile_ids_are_listed_once() {
        let fixture = Fixture::new();
        let mut config = PostProcessConfig::default();
        let mut second = profile("dup");
        second.model = "other".into();
        config.api_profiles = vec![profile("dup"), second];
        activate(&mut config, &LlmEntryKey::provider("dup"));
        let entries = fixture.entries(&config);
        assert_eq!(entries.len(), 1);
        match &entries[0].kind {
            LlmEntryKind::Provider(p) => {
                assert_eq!(p.model, "gpt-6-luna", "앞의 것이 쓰여야 합니다")
            }
            other => panic!("unexpected entry: {other:?}"),
        }
    }

    /// 설정을 파일에 저장한 뒤 다시 읽습니다. 앱이 설정을 저장하고 파일과 맞추는 과정과 같습니다.
    fn save_and_reload(store: &ConfigStore, config: &Config) -> Config {
        store.save(config).unwrap();
        store.load().unwrap().config
    }

    #[test]
    fn selection_survives_saving_and_reloading() {
        let fixture = Fixture::new();
        fixture.install(SMALL);
        let store = ConfigStore::new(fixture.paths.config_file());
        let mut config = store.load_or_create().unwrap().config;
        config.postprocess.api_profiles = vec![profile("openai"), profile("groq")];

        for key in [
            LlmEntryKey::local(SMALL),
            LlmEntryKey::provider("groq"),
            LlmEntryKey::provider("openai"),
            LlmEntryKey::local(SMALL),
        ] {
            activate(&mut config.postprocess, &key);
            config = save_and_reload(&store, &config);
            assert_eq!(
                selected_keys(&fixture.entries(&config.postprocess)),
                [&key],
                "저장하고 다시 읽은 뒤에도 {key:?}만 골라져 있어야 합니다"
            );
        }

        // 제공자를 지우고 선택을 비운 것도 저장 뒤에 그대로여야 합니다.
        activate(&mut config.postprocess, &LlmEntryKey::provider("groq"));
        fixture.uninstall(&mut config.postprocess, SMALL);
        remove_profile(&fixture.paths, &mut config.postprocess, "groq");
        remove_profile(&fixture.paths, &mut config.postprocess, "openai");
        let reloaded = save_and_reload(&store, &config);
        assert!(reloaded.postprocess.api_profiles.is_empty());
        assert_eq!(reloaded.postprocess.processor, "");
        assert_eq!(selection_issue(&fixture.paths, &reloaded.postprocess), None);
    }

    /// Codex에 로그인해 둡니다.
    fn sign_in_codex(paths: &AppPaths) {
        use crate::codex::auth::tests::{hour_from_now, tokens};
        crate::codex::auth::store(paths, &tokens(hour_from_now())).unwrap();
    }

    #[test]
    fn codex_is_listed_between_local_models_and_providers_when_signed_in() {
        let fixture = Fixture::new();
        fixture.install(SMALL);
        let mut config = PostProcessConfig::default();
        config.api_profiles.push(profile("openai"));
        let keys = |config: &PostProcessConfig| -> Vec<LlmEntryKey> {
            fixture.entries(config).into_iter().map(|e| e.key).collect()
        };
        assert_eq!(
            keys(&config),
            [LlmEntryKey::local(SMALL), LlmEntryKey::provider("openai")],
            "로그인하지 않았으면 Codex 항목이 없어야 합니다"
        );

        sign_in_codex(&fixture.paths);
        assert_eq!(
            keys(&config),
            [
                LlmEntryKey::local(SMALL),
                LlmEntryKey::Codex,
                LlmEntryKey::provider("openai"),
            ]
        );
        let entries = fixture.entries(&config);
        let LlmEntryKind::Codex(codex_entry) = &entries[1].kind else {
            panic!("Codex 항목이어야 합니다: {:?}", entries[1]);
        };
        assert_eq!(codex_entry.account.email.as_deref(), Some("me@example.com"));
        assert_eq!(codex_entry.model, codex::DEFAULT_MODEL);
        assert!(entries[1].ready);
    }

    #[test]
    fn activating_codex_selects_only_codex_and_writes_the_model() {
        let fixture = Fixture::new();
        fixture.install(SMALL);
        sign_in_codex(&fixture.paths);
        let mut config = PostProcessConfig::default();
        config.api_profiles.push(profile("openai"));

        for key in [
            LlmEntryKey::Codex,
            LlmEntryKey::local(SMALL),
            LlmEntryKey::provider("openai"),
            LlmEntryKey::Codex,
        ] {
            activate(&mut config, &key);
            let entries = fixture.entries(&config);
            assert_eq!(
                selected_keys(&entries),
                [&key],
                "{key:?}만 골라져 있어야 합니다"
            );
            assert_eq!(selection_issue(&fixture.paths, &config), None);
        }
        assert_eq!(config.processor, CODEX_PROCESSOR_ID);
        assert_eq!(
            config.processors[CODEX_PROCESSOR_ID].get(codex::MODEL_KEY),
            Some(&Value::String(codex::DEFAULT_MODEL.into())),
            "고른 모델이 설정 파일에 보이게 적어야 합니다"
        );
    }

    #[test]
    fn signing_out_of_codex_moves_the_selection() {
        let fixture = Fixture::new();
        fixture.install(MEDIUM);
        sign_in_codex(&fixture.paths);
        let mut config = PostProcessConfig::default();
        activate(&mut config, &LlmEntryKey::Codex);

        crate::codex::auth::sign_out(&fixture.paths).unwrap();
        assert_eq!(
            selection_issue(&fixture.paths, &config),
            Some(SelectionIssue::CodexSignedOut)
        );
        on_codex_signed_out(&fixture.paths, &mut config);
        assert_eq!(
            selected_keys(&fixture.entries(&config)),
            [&LlmEntryKey::local(MEDIUM)]
        );

        // 쓰지 않던 Codex에서 로그아웃하면 고른 항목은 그대로입니다.
        sign_in_codex(&fixture.paths);
        crate::codex::auth::sign_out(&fixture.paths).unwrap();
        on_codex_signed_out(&fixture.paths, &mut config);
        assert_eq!(config.processor, LOCAL_PROCESSOR_ID);
    }

    #[test]
    fn codex_selection_survives_saving_and_reloading() {
        let fixture = Fixture::new();
        sign_in_codex(&fixture.paths);
        let store = ConfigStore::new(fixture.paths.config_file());
        let mut config = store.load_or_create().unwrap().config;
        activate(&mut config.postprocess, &LlmEntryKey::Codex);
        codex::set_model(&mut config.postprocess, "gpt-5.6-terra");
        codex::set_effort(&mut config.postprocess, ReasoningEffort::None);

        let reloaded = save_and_reload(&store, &config);
        assert_eq!(
            selected_keys(&fixture.entries(&reloaded.postprocess)),
            [&LlmEntryKey::Codex]
        );
        assert_eq!(codex::chosen_model(&reloaded.postprocess), "gpt-5.6-terra");
        assert_eq!(
            codex::chosen_effort(&reloaded.postprocess),
            ReasoningEffort::None
        );
    }

    #[test]
    fn next_profile_id_and_name_are_unique() {
        let existing = vec![
            ApiProfile {
                id: "openai".into(),
                name: "내 OpenAI".into(),
                ..ApiProfile::default()
            },
            ApiProfile {
                id: "openai-2".into(),
                name: "내 OpenAI 2".into(),
                ..ApiProfile::default()
            },
        ];
        assert_eq!(next_profile_id(&existing, "openai"), "openai-3");
        assert_eq!(next_profile_id(&existing, "groq"), "groq");
        assert_eq!(next_profile_id(&existing, "한글.."), "profile");
        assert_eq!(next_profile_name(&existing, "내 OpenAI"), "내 OpenAI 3");
        assert_eq!(next_profile_name(&existing, "내 Groq"), "내 Groq");
    }
}
