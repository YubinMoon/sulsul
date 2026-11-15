//! "외부 제공자" 탭: OpenAI 호환 API 제공자를 새로 등록합니다.
//!
//! 입력하는 동안의 값(초안)은 화면 상태에만 두고, 연결 테스트를 통과한 뒤 "등록"을 눌러야
//! 설정에 들어갑니다. 등록한 제공자는 "사용 가능 엔진" 탭에서 고르고 고치고 지웁니다.

use egui::Ui;
use rust_i18n::t;
use sulsul_core::config::ApiProfile;
use sulsul_core::stt::EngineContext;
use sulsul_stt_cloud::{ENGINE_ID as CLOUD_ENGINE_ID, PRESETS};

use super::ModelsPage;
use super::entries::{
    EntryKey, activate, apply_provider_preset_switch, available_entries,
    create_profile_from_preset, next_profile_id, profile_display_name, profile_test_config,
    selection_ready,
};
use super::jobs::TestTarget;
use super::parts::{self, TestClick};
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
    /// 고른 공급자의 ID. 아직 고르지 않았으면 빈 문자열입니다.
    fn provider(&self) -> &str {
        self.profile
            .as_ref()
            .map_or("", |profile| profile.provider.as_str())
    }

    /// 공급자를 고르거나 바꿉니다.
    ///
    /// 처음 고르면 그 공급자의 기본값으로 채웁니다. 바꾸면 사용자가 손대지 않은 주소·모델·이름만
    /// 새 공급자의 기본값으로 바꾸고, 직접 넣은 값은 둡니다.
    pub fn choose_provider(&mut self, existing: &[ApiProfile], provider: &str) {
        let Some(profile) = &mut self.profile else {
            self.profile = Some(create_profile_from_preset(existing, provider));
            return;
        };
        if profile.provider == provider {
            return;
        }
        let old_provider = std::mem::replace(&mut profile.provider, provider.to_owned());
        apply_provider_preset_switch(profile, &old_provider, provider);
        let old_default_name = create_profile_from_preset(existing, &old_provider).name;
        if profile.name.trim().is_empty() || profile.name == old_default_name {
            profile.name = create_profile_from_preset(existing, provider).name;
        }
    }

    /// 지금 값으로 연결 테스트를 시작했음을 적어 둡니다.
    fn begin_test(&mut self) {
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
}

impl ModelsPage {
    pub(super) fn providers_tab(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        ui.label(text::caption(ui, t!("models.providers.help")));
        ui.add_space(6.0);

        let target = TestTarget::Draft;
        let mut test_click = None;
        let mut register = false;
        card::card(ui, |ui| {
            // 공급자. 고르기 전에는 아래 입력 칸을 보여 주지 않습니다.
            let items: Vec<ComboItem<String>> = PRESETS
                .iter()
                .map(|preset| ComboItem::new(preset.id.to_owned(), tr(preset.name_key)))
                .collect();
            let mut provider = self.draft.provider().to_owned();
            let changed = Row::new(&t!("models.providers.field.provider")).show(ui, |ui| {
                let width = row::CONTROL_WIDTH.min(ui.available_width());
                controls::combo(
                    ui,
                    "draft_provider",
                    width,
                    &mut provider,
                    &items,
                    &t!("models.providers.choose"),
                )
            });
            if changed {
                self.draft
                    .choose_provider(&cx.config.stt.api_profiles, &provider);
            }

            let block = self.draft.register_block(cx.locked);
            let Some(profile) = &mut self.draft.profile else {
                ui.add_space(4.0);
                ui.label(text::muted(ui, t!("models.providers.choose_hint")));
                return;
            };
            row::separator(ui);
            parts::profile_fields(ui, "draft", profile, &mut self.draft.reveal_key);

            ui.add_space(10.0);
            parts::action_row(
                ui,
                |ui| {
                    test_click = parts::test_button(
                        ui,
                        &t!("models.providers.test"),
                        self.test.as_ref(),
                        &target,
                    );
                },
                |ui| {
                    ui.add_enabled_ui(block.is_none(), |ui| {
                        register =
                            controls::primary_button(ui, t!("models.providers.register")).clicked();
                    });
                },
            );
            // 등록 버튼을 누를 수 없으면 이유를 알립니다.
            if let Some(block) = block {
                ui.label(text::caption(ui, block.text()));
            }
            parts::test_progress(ui, self.test.as_ref(), &target);
            parts::test_outcome(ui, self.last_test.as_ref(), &target);
        });
        ui.add_space(6.0);
        ui.label(text::caption(ui, t!("models.providers.note")));

        match test_click {
            Some(TestClick::Start) => self.start_draft_test(ui, cx),
            Some(TestClick::Cancel) => self.cancel_test(),
            None => {}
        }
        if register {
            self.register_draft(cx);
        }
    }

    /// 초안의 값으로 연결 테스트를 시작합니다. 설정은 바꾸지 않습니다.
    fn start_draft_test(&mut self, ui: &Ui, cx: &mut PageContext<'_>) {
        let Some(mut profile) = self.draft.profile.clone() else {
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

    /// 초안을 설정에 등록하고 "사용 가능 엔진" 탭에서 그 카드를 펼쳐 보여 줍니다.
    fn register_draft(&mut self, cx: &mut PageContext<'_>) {
        if self.draft.register_block(cx.locked).is_some() {
            return;
        }
        let Some(mut profile) = self.draft.profile.take() else {
            return;
        };
        self.draft = Draft::default();
        self.forget_test(&TestTarget::Draft);

        profile.id = next_profile_id(&cx.config.stt.api_profiles, &profile.provider);
        let key = EntryKey::provider(&profile.id);
        let name = profile_display_name(&profile);

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

        cx.shared
            .toasts
            .info(t!("models.providers.registered", name = name));
        self.show_entry(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft_with(provider: &str) -> Draft {
        let mut draft = Draft::default();
        draft.choose_provider(&[], provider);
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
        let existing = [create_profile_from_preset(&[], "openai")];
        let mut draft = Draft::default();
        draft.choose_provider(&existing, "openai");

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
        draft.choose_provider(&[], "groq");
        let groq = create_profile_from_preset(&[], "groq");
        assert_eq!(profile(&mut draft).provider, "groq");
        assert_eq!(profile(&mut draft).endpoint, groq.endpoint);
        assert_eq!(profile(&mut draft).model, groq.model);
        assert_eq!(profile(&mut draft).name, groq.name);
        assert_ne!(groq.name, openai_name);

        // 직접 넣은 값은 공급자를 바꿔도 그대로입니다.
        profile(&mut draft).name = "회사 서버".into();
        profile(&mut draft).endpoint = "https://stt.example.com/v1".into();
        profile(&mut draft).api_key = "secret".into();
        draft.choose_provider(&[], "custom");
        assert_eq!(profile(&mut draft).provider, "custom");
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
    fn failed_test_clears_an_earlier_pass() {
        let mut draft = draft_with("custom");
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
        let mut draft = draft_with("custom");
        draft.begin_test();
        profile(&mut draft).endpoint = "https://changed.example.com/v1".into();
        draft.finish_test(true);
        assert_eq!(draft.register_block(false), Some(RegisterBlock::Changed));
    }
}
