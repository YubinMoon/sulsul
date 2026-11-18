//! "외부 제공자" 탭: OpenAI 호환 API 제공자를 새로 등록합니다.
//!
//! 입력하는 동안의 값(초안)은 화면 상태에만 두고, 연결 테스트를 통과한 뒤 "등록"을 눌러야
//! 설정에 들어갑니다. 등록한 제공자는 "사용 가능 모델" 탭에서 고르고 고치고 지웁니다.
//!
//! 초안([`Draft`])과 등록 칸([`draft_form`])은 음성 인식과 글 다듬기 갈래가 함께 씁니다.
//! 연결 테스트를 돌리는 방법만 갈래마다 다르므로, 그 부분은 부르는 쪽에서 맡습니다.

use egui::Ui;
use rust_i18n::t;
use sulsul_core::config::ApiProfile;
use sulsul_core::stt::EngineContext;
use sulsul_stt_cloud::ENGINE_ID as CLOUD_ENGINE_ID;

use super::entries::{EntryKey, activate, available_entries, profile_test_config, selection_ready};
use super::jobs::TestTarget;
use super::parts::{self, TestClick, TestState};
use super::profiles::{self, PresetKind, next_profile_id};
use super::{Category, ModelsPage};
use crate::i18n::tr;
use crate::session::PageContext;
use crate::widgets::card;
use crate::widgets::controls::{self, ComboItem};
use crate::widgets::row::{self, Row};
use crate::widgets::text;

/// 테스트 결과와 견줄 값. 이 값이 같아야 "테스트한 그대로"라고 봅니다.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    provider: String,
    endpoint: String,
    api_key: String,
    model: String,
}

impl Fingerprint {
    fn of(profile: &ApiProfile) -> Self {
        Self {
            provider: profile.provider.clone(),
            endpoint: profile.endpoint.clone(),
            api_key: profile.api_key.clone(),
            model: profile.model.clone(),
        }
    }
}

/// 지금은 등록할 수 없는 이유
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterBlock {
    /// 설정 파일에 문제가 있어 설정을 바꿀 수 없습니다.
    Locked,
    /// 연결 테스트가 끝나지 않았습니다.
    Testing,
    /// 아직 연결 테스트를 통과하지 못했습니다.
    NotTested,
    /// 테스트를 통과한 뒤에 값을 바꿨습니다.
    Changed,
}

impl RegisterBlock {
    fn text(self) -> String {
        match self {
            Self::Locked => t!("models.providers.register_blocked.locked"),
            Self::Testing => t!("models.providers.register_blocked.testing"),
            Self::NotTested => t!("models.providers.register_blocked.not_tested"),
            Self::Changed => t!("models.providers.register_blocked.changed"),
        }
        .into_owned()
    }
}

/// 아직 등록하지 않은 새 제공자 (초안). 설정 파일에는 들어가지 않습니다.
#[derive(Default)]
pub struct Draft {
    /// 입력 중인 값. 공급자를 고르기 전에는 없습니다.
    profile: Option<ApiProfile>,
    /// API 키를 가리지 않고 보여 줄지
    reveal_key: bool,
    /// 마지막 연결 테스트가 성공했을 때의 값
    passed: Option<Fingerprint>,
    /// 진행 중인 연결 테스트가 쓰는 값
    testing: Option<Fingerprint>,
}

impl Draft {
    /// 입력 중인 값. 공급자를 고르기 전에는 없습니다.
    pub fn profile(&self) -> Option<&ApiProfile> {
        self.profile.as_ref()
    }

    /// 고른 공급자의 ID. 아직 고르지 않았으면 빈 문자열입니다.
    fn provider(&self) -> &str {
        self.profile
            .as_ref()
            .map_or("", |profile| profile.provider.as_str())
    }

    /// 공급자를 고르거나 바꿉니다. `existing`은 그 갈래에 이미 등록한 제공자입니다.
    ///
    /// 처음 고르면 그 공급자의 기본값으로 채웁니다. 바꾸면 사용자가 손대지 않은 주소·모델·이름만
    /// 새 공급자의 기본값으로 바꾸고, 직접 넣은 값은 둡니다.
    pub fn choose_provider(&mut self, category: Category, existing: &[ApiProfile], provider: &str) {
        let Some(profile) = &mut self.profile else {
            self.profile = Some(profiles::new_profile(category, existing, provider));
            return;
        };
        if profile.provider == provider {
            return;
        }
        let old_provider = std::mem::replace(&mut profile.provider, provider.to_owned());
        profiles::switch_provider(category, profile, &old_provider, provider);
        let old_default_name = profiles::new_profile(category, existing, &old_provider).name;
        if profile.name.trim().is_empty() || profile.name == old_default_name {
            profile.name = profiles::new_profile(category, existing, provider).name;
        }
    }

    /// 지금 값으로 연결 테스트를 시작했음을 적어 둡니다.
    pub fn begin_test(&mut self) {
        self.testing = self.profile.as_ref().map(Fingerprint::of);
    }

    /// 연결 테스트가 끝났습니다. 성공했으면 그때의 값을 기억합니다.
    pub fn finish_test(&mut self, passed: bool) {
        let tested = self.testing.take();
        self.passed = if passed { tested } else { None };
    }

    /// 지금 등록할 수 없으면 그 이유를 돌려줍니다.
    ///
    /// 마지막 연결 테스트가 성공했고, 그때의 공급자·주소·API 키·모델이 지금과 같아야 등록할 수 있습니다.
    pub fn register_block(&self, locked: bool) -> Option<RegisterBlock> {
        if locked {
            return Some(RegisterBlock::Locked);
        }
        if self.testing.is_some() {
            return Some(RegisterBlock::Testing);
        }
        let current = self.profile.as_ref().map(Fingerprint::of);
        match &self.passed {
            None => Some(RegisterBlock::NotTested),
            Some(passed) if Some(passed) != current.as_ref() => Some(RegisterBlock::Changed),
            Some(_) => None,
        }
    }

    /// 등록할 수 있으면 초안을 비우고, 설정에 넣을 제공자를 돌려줍니다.
    /// ID는 `existing`(그 갈래에 이미 등록한 제공자)과 겹치지 않게 붙입니다.
    pub fn take_registered(&mut self, existing: &[ApiProfile], locked: bool) -> Option<ApiProfile> {
        if self.register_block(locked).is_some() {
            return None;
        }
        let mut profile = std::mem::take(self).profile?;
        profile.id = next_profile_id(existing, &profile.provider);
        Some(profile)
    }
}

/// 공급자를 고르는 목록. 이 PC에서 도는 서버가 섞여 있는 갈래에서는 묶음 제목을 붙여
/// 클라우드 서비스와 나눠 보여 줍니다.
fn provider_items(category: Category) -> Vec<ComboItem<String>> {
    let presets = profiles::presets(category);
    let grouped = presets
        .iter()
        .any(|preset| preset.kind() == PresetKind::Local);
    presets
        .iter()
        .map(|preset| {
            let item = ComboItem::new(preset.id.to_owned(), tr(preset.name_key));
            if !grouped {
                return item;
            }
            item.group(match preset.kind() {
                PresetKind::Cloud => t!("models.providers.group.cloud"),
                PresetKind::Local => t!("models.providers.group.local"),
                PresetKind::Custom => t!("models.providers.group.custom"),
            })
        })
        .collect()
}

/// 등록 칸([`draft_form`])에서 누른 버튼
pub struct FormOutcome {
    /// 연결 테스트 버튼 (시작 또는 취소)
    pub test: Option<TestClick>,
    /// 등록 버튼
    pub register: bool,
}

/// 새 제공자를 등록하는 칸: 공급자 고르기, 이름·주소·API 키·모델, 연결 테스트와 등록 버튼.
///
/// - `existing`: 이 갈래에 이미 등록한 제공자 (새 이름이 겹치지 않게 하는 데 씁니다)
/// - `test_state`: 연결 테스트 버튼이 보일 모습
/// - `add_test_status`: 버튼 아래에 놓을 연결 테스트 진행 상황과 결과
///
/// 누른 버튼을 돌려주고, 연결 테스트와 등록은 부르는 쪽에서 합니다.
pub fn draft_form(
    ui: &mut Ui,
    category: Category,
    draft: &mut Draft,
    existing: &[ApiProfile],
    locked: bool,
    test_state: TestState,
    add_test_status: impl FnOnce(&mut Ui),
) -> FormOutcome {
    // 음성 인식 갈래에는 직접 입력 공급자가 없어, 다른 호환 서버를 쓰는 법만 알립니다.
    if category == Category::Stt {
        ui.label(text::caption(ui, t!("models.providers.help.stt")));
        ui.add_space(6.0);
    }

    // 입력 칸 ID가 두 갈래에서 겹치지 않게 합니다.
    let id_salt = match category {
        Category::Stt => "stt_draft",
        Category::Llm => "llm_draft",
    };
    let mut outcome = FormOutcome {
        test: None,
        register: false,
    };
    card::card(ui, |ui| {
        // 공급자. 고르기 전에는 아래 입력 칸을 보여 주지 않습니다.
        let items = provider_items(category);
        let mut provider = draft.provider().to_owned();
        let note = profiles::provider_note(category, &provider);
        let changed = Row::new(&t!("models.providers.field.provider"))
            .help_opt(note.as_deref())
            .show(ui, |ui| {
                let width = row::CONTROL_WIDTH.min(ui.available_width());
                controls::combo(
                    ui,
                    (id_salt, "provider"),
                    width,
                    &mut provider,
                    &items,
                    &t!("models.providers.choose"),
                )
            });
        if changed {
            draft.choose_provider(category, existing, &provider);
        }

        let block = draft.register_block(locked);
        let Some(profile) = &mut draft.profile else {
            return;
        };
        row::separator(ui);
        parts::profile_fields(ui, category, id_salt, profile, &mut draft.reveal_key);

        ui.add_space(10.0);
        parts::action_row(
            ui,
            |ui| {
                outcome.test = parts::test_button(ui, &t!("models.providers.test"), test_state);
            },
            |ui| {
                ui.add_enabled_ui(block.is_none(), |ui| {
                    outcome.register =
                        controls::primary_button(ui, t!("models.providers.register")).clicked();
                });
            },
        );
        // 등록 버튼을 누를 수 없으면 이유를 알립니다.
        if let Some(block) = block {
            ui.label(text::caption(ui, block.text()));
        }
        add_test_status(ui);
    });
    outcome
}

impl ModelsPage {
    pub(super) fn providers_tab(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let target = TestTarget::Draft;
        let test_state = self.test_state(&target);
        let outcome = draft_form(
            ui,
            Category::Stt,
            &mut self.draft,
            &cx.config.stt.api_profiles,
            cx.locked,
            test_state,
            |ui| {
                parts::test_progress(ui, self.test.as_ref(), &target);
                parts::test_outcome(ui, self.last_test.as_ref(), &target);
            },
        );

        match outcome.test {
            Some(TestClick::Start) => self.start_draft_test(ui, cx),
            Some(TestClick::Cancel) => self.cancel_test(),
            None => {}
        }
        if outcome.register {
            self.register_draft(cx);
        }
    }

    /// 초안의 값으로 연결 테스트를 시작합니다. 설정은 바꾸지 않습니다.
    fn start_draft_test(&mut self, ui: &Ui, cx: &mut PageContext<'_>) {
        let Some(mut profile) = self.draft.profile().cloned() else {
            return;
        };
        // 등록된 제공자와 겹치지 않는 ID를 붙여, 테스트 설정에서 이 초안이 쓰이게 합니다.
        profile.id = next_profile_id(&cx.config.stt.api_profiles, &profile.provider);
        let stt = profile_test_config(&cx.config.stt, &profile);
        let started = self.start_test(
            cx.shared,
            ui.ctx(),
            TestTarget::Draft,
            stt,
            CLOUD_ENGINE_ID.to_owned(),
        );
        if started {
            self.draft.begin_test();
        }
    }

    /// 초안을 설정에 등록하고 "사용 가능 모델" 탭에서 그 카드를 펼쳐 보여 줍니다.
    fn register_draft(&mut self, cx: &mut PageContext<'_>) {
        let registered = self
            .draft
            .take_registered(&cx.config.stt.api_profiles, cx.locked);
        let Some(profile) = registered else {
            return;
        };
        self.forget_test(&TestTarget::Draft);

        let key = EntryKey::provider(&profile.id);
        // 지금 설정으로 받아쓰기를 할 수 없는 상태일 때만 새 제공자를 바로 켭니다.
        let engine_ctx = EngineContext {
            paths: cx.shared.paths.clone(),
        };
        let usable = selection_ready(&available_entries(
            &cx.shared.registry,
            &engine_ctx,
            &cx.config.stt,
        ));
        cx.config.stt.api_profiles.push(profile);
        if !usable {
            activate(&cx.shared.registry, &mut cx.config.stt, &key);
        }

        // 등록한 카드를 펼쳐 보여 주므로 따로 알리지 않습니다.
        self.show_entry(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft_with(provider: &str) -> Draft {
        let mut draft = Draft::default();
        draft.choose_provider(Category::Stt, &[], provider);
        draft
    }

    fn profile(draft: &mut Draft) -> &mut ApiProfile {
        draft.profile.as_mut().unwrap()
    }

    #[test]
    fn nothing_is_chosen_at_first() {
        let draft = Draft::default();
        assert_eq!(draft.provider(), "");
        assert_eq!(
            draft.register_block(false),
            Some(RegisterBlock::NotTested),
            "공급자를 고르기 전에는 등록할 수 없습니다"
        );
    }

    #[test]
    fn choosing_a_provider_fills_its_defaults() {
        rust_i18n::set_locale("ko");
        let existing = [profiles::new_profile(Category::Stt, &[], "openai")];
        let mut draft = Draft::default();
        draft.choose_provider(Category::Stt, &existing, "openai");

        let chosen = profile(&mut draft).clone();
        assert_eq!(chosen.provider, "openai");
        assert_eq!(chosen.endpoint, existing[0].endpoint);
        assert_ne!(
            chosen.name, existing[0].name,
            "이미 있는 이름과 겹치지 않아야 합니다"
        );
    }

    #[test]
    fn switching_provider_keeps_what_the_user_typed() {
        rust_i18n::set_locale("ko");
        let mut draft = draft_with("openai");
        let openai_name = profile(&mut draft).name.clone();

        // 손대지 않은 주소·모델·이름은 새 공급자의 기본값으로 바뀝니다.
        draft.choose_provider(Category::Stt, &[], "groq");
        let groq = profiles::new_profile(Category::Stt, &[], "groq");
        assert_eq!(profile(&mut draft).provider, "groq");
        assert_eq!(profile(&mut draft).endpoint, groq.endpoint);
        assert_eq!(profile(&mut draft).model, groq.model);
        assert_eq!(profile(&mut draft).name, groq.name);
        assert_ne!(groq.name, openai_name);

        // 직접 넣은 값은 공급자를 바꿔도 그대로입니다.
        profile(&mut draft).name = "회사 서버".into();
        profile(&mut draft).endpoint = "https://stt.example.com/v1".into();
        profile(&mut draft).api_key = "secret".into();
        draft.choose_provider(Category::Stt, &[], "openai");
        assert_eq!(profile(&mut draft).provider, "openai");
        assert_eq!(profile(&mut draft).name, "회사 서버");
        assert_eq!(profile(&mut draft).endpoint, "https://stt.example.com/v1");
        assert_eq!(profile(&mut draft).api_key, "secret");
    }

    #[test]
    fn registering_needs_a_passed_test_of_the_same_values() {
        let mut draft = draft_with("openai");
        profile(&mut draft).api_key = "sk-test".into();
        assert_eq!(draft.register_block(false), Some(RegisterBlock::NotTested));

        draft.begin_test();
        assert_eq!(draft.register_block(false), Some(RegisterBlock::Testing));
        draft.finish_test(true);
        assert_eq!(draft.register_block(false), None);
        assert_eq!(
            draft.register_block(true),
            Some(RegisterBlock::Locked),
            "설정이 잠겨 있으면 등록할 수 없습니다"
        );

        // 이름은 테스트와 상관없으므로 바꿔도 다시 테스트하지 않아도 됩니다.
        profile(&mut draft).name = "다른 이름".into();
        assert_eq!(draft.register_block(false), None);

        // 주소·API 키·모델·공급자 중 하나라도 바뀌면 다시 테스트해야 합니다.
        let tested_model = profile(&mut draft).model.clone();
        profile(&mut draft).model = "another-model".into();
        assert_eq!(draft.register_block(false), Some(RegisterBlock::Changed));
        profile(&mut draft).model = tested_model;
        assert_eq!(draft.register_block(false), None);
        profile(&mut draft).api_key.push('x');
        assert_eq!(draft.register_block(false), Some(RegisterBlock::Changed));
    }

    #[test]
    fn registering_takes_the_draft_and_gives_it_an_unused_id() {
        rust_i18n::set_locale("ko");
        let existing = [profiles::new_profile(Category::Llm, &[], "openai")];
        let mut draft = Draft::default();
        draft.choose_provider(Category::Llm, &existing, "openai");

        // 테스트를 통과하기 전이나 설정이 잠겨 있을 때는 초안을 그대로 둡니다.
        assert!(draft.take_registered(&existing, false).is_none());
        draft.begin_test();
        draft.finish_test(true);
        assert!(draft.take_registered(&existing, true).is_none());
        assert_eq!(draft.provider(), "openai");

        let registered = draft.take_registered(&existing, false).unwrap();
        assert_eq!(registered.provider, "openai");
        assert_ne!(
            registered.id, existing[0].id,
            "이미 있는 ID와 겹치면 안 됩니다"
        );
        assert!(!registered.id.is_empty());
        // 등록한 뒤에는 빈 초안으로 돌아갑니다.
        assert!(draft.profile().is_none());
        assert_eq!(draft.register_block(false), Some(RegisterBlock::NotTested));
    }

    #[test]
    fn provider_list_is_grouped_only_when_local_servers_are_listed() {
        rust_i18n::set_locale("ko");
        // 음성 인식: 이 PC에서 도는 서버가 없으므로 묶음 제목을 붙이지 않습니다.
        let stt = provider_items(Category::Stt);
        assert_eq!(
            stt.iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            ["openai", "groq"],
            "음성 인식의 호환 서버는 OpenAI에서 설정하고 별도 커스텀 항목은 두지 않습니다"
        );
        assert!(stt.iter().all(|item| item.group.is_none()));

        // 글 다듬기: 클라우드 → 이 PC에서 실행 → 직접 입력 순서로, 묶음마다 이어져 있어야
        // 제목이 한 번씩만 나옵니다.
        let llm = provider_items(Category::Llm);
        let mut titles: Vec<&str> = Vec::new();
        for item in &llm {
            let title = item.group.as_deref().expect("묶음 제목이 없습니다");
            if titles.last() != Some(&title) {
                titles.push(title);
            }
        }
        assert_eq!(titles, ["클라우드 서비스", "이 PC에서 실행", "직접 입력"]);
        let group_of = |id: &str| {
            llm.iter()
                .find(|item| item.value == id)
                .and_then(|item| item.group.clone())
        };
        assert_eq!(group_of("openai").as_deref(), Some("클라우드 서비스"));
        assert_eq!(group_of("ollama").as_deref(), Some("이 PC에서 실행"));
        assert_eq!(group_of("custom").as_deref(), Some("직접 입력"));
    }

    #[test]
    fn failed_test_clears_an_earlier_pass() {
        let mut draft = draft_with("openai");
        draft.begin_test();
        draft.finish_test(true);
        assert_eq!(draft.register_block(false), None);

        draft.begin_test();
        draft.finish_test(false);
        assert_eq!(
            draft.register_block(false),
            Some(RegisterBlock::NotTested),
            "마지막 테스트가 실패했으면 등록할 수 없습니다"
        );
    }

    #[test]
    fn values_changed_during_a_test_do_not_count_as_tested() {
        let mut draft = draft_with("openai");
        draft.begin_test();
        profile(&mut draft).endpoint = "https://changed.example.com/v1".into();
        draft.finish_test(true);
        assert_eq!(draft.register_block(false), Some(RegisterBlock::Changed));
    }
}
