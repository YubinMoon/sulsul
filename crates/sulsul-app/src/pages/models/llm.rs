//! 글 다듬기 갈래: 다듬는 데 쓸 로컬 모델과 외부 제공자를 고르고, 내려받고, 등록합니다.
//!
//! 음성 인식 갈래와 같은 탭 세 개를 같은 화면 조각(`parts`, `download`, `providers`)으로 그립니다.
//! 목록에 무엇이 들어가고 어느 것이 골라져 있는지는 `sulsul_llm::entries`가 정하고, 값은 설정의
//! `[postprocess]`에 들어갑니다.
//!
//! 여기서는 "무엇으로 다듬을지"만 고릅니다. 다듬기를 켜고 끄는 스위치는 설정 화면의 후처리 묶음에 있습니다.

use egui::Ui;
use rust_i18n::t;
use sulsul_core::AppPaths;
use sulsul_core::config::{ApiProfile, PostProcessConfig};
use sulsul_core::stt::{ModelVariant, delete_variant, format_bytes};
use sulsul_llm::catalog::{LlmFamily, families, find_variant, models_root};
use sulsul_llm::client::{ConnectionReply, ConnectionTest, LlmError};
use sulsul_llm::entries::{
    LlmEntry, LlmEntryKey, LlmEntryKind, SelectionIssue, activate, available_entries,
    chosen_local_model, on_local_removed, remove_profile, selection_issue, selection_ready,
};

use super::download::{
    Picker, PickerGroup, VariantClick, axis_name_key, open_folder, other_download, picker_row,
    variant_card, variant_folder,
};
use super::entries::variant_label;
use super::jobs::{DownloadTarget, Downloads};
use super::parts::{self, BODY_GAP, Badge, CARD_GAP, CardHead, ListState, TestClick, TestState};
use super::profiles::{self, profile_display_name};
use super::providers::{Draft, draft_form};
use super::{Category, Tab, tab_bar};
use crate::error_text;
use crate::format;
use crate::i18n::{tr, tr_opt};
use crate::icons::Icon;
use crate::pages::SECTION_GAP;
use crate::session::{PageContext, Shared};
use crate::theme::Palette;
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::confirm::Answer;
use crate::widgets::controls;
use crate::widgets::row;
use crate::widgets::text;
use crate::widgets::tile;

/// 화면에 보여 줄 응답 텍스트의 최대 글자 수. 연결 테스트의 응답은 짧지만, 길게 답하는 모델도 있습니다.
const MAX_REPLY_CHARS: usize = 200;

/// 연결 테스트의 대상
#[derive(Debug, Clone, PartialEq, Eq)]
enum LlmTestTarget {
    /// 등록한 제공자 (프로필 ID)
    Provider(String),
    /// "외부 제공자" 탭에서 입력 중인 초안
    Draft,
}

/// 진행 중인 연결 테스트. 한 번에 하나만 돌립니다.
struct LlmTest {
    target: LlmTestTarget,
    handle: ConnectionTest,
}

/// 끝난 연결 테스트의 결과
struct LlmTestOutcome {
    target: LlmTestTarget,
    result: Result<ConnectionReply, LlmError>,
}

/// 사용자에게 확인을 받고 있는 제거
enum PendingRemoval {
    /// 내려받은 모델 파일을 지웁니다.
    Variant { variant_id: String, label: String },
    /// 등록한 제공자를 지웁니다.
    Profile { id: String, name: String },
}

/// 글 다듬기 갈래의 화면 상태
#[derive(Default)]
pub struct LlmPane {
    tab: Tab,
    /// "사용 가능 모델" 목록의 펼침 상태
    list: ListState<LlmEntryKey>,
    /// "로컬 모델 내려받기" 탭에서 고른 모델 계열과 단계 값
    picker: Picker,
    /// "외부 제공자" 탭에서 입력 중인 새 제공자
    draft: Draft,
    test: Option<LlmTest>,
    last_test: Option<LlmTestOutcome>,
    confirm: Option<PendingRemoval>,
}

impl LlmPane {
    /// 연결 테스트가 진행 중이면 `true`입니다. (내려받기는 두 갈래가 함께 쓰는 쪽에서 봅니다)
    pub fn is_busy(&self) -> bool {
        self.test.is_some()
    }

    /// 연결 테스트가 끝났는지 확인합니다. 다른 화면을 보고 있을 때도 불립니다.
    pub fn tick(&mut self) {
        let finished = self.test.as_ref().and_then(|test| test.handle.poll());
        if let Some(result) = finished
            && let Some(test) = self.test.take()
        {
            if test.target == LlmTestTarget::Draft {
                self.draft.finish_test(result.is_ok());
            }
            self.last_test = Some(LlmTestOutcome {
                target: test.target,
                result,
            });
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, downloads: &mut Downloads) {
        // 다듬기 자체는 아직 동작하지 않습니다. 고른 모델이 바로 쓰이지 않는다는 것을 먼저 알립니다.
        Banner::new(Tone::Info)
            .line(t!("models.llm.preview_note"))
            .show(ui);
        ui.add_space(10.0);

        tab_bar(ui, &mut self.tab);
        match self.tab {
            Tab::Available => self.available_tab(ui, cx, downloads),
            Tab::Download => self.download_tab(ui, cx, downloads),
            Tab::Providers => self.providers_tab(ui, cx),
        }
    }

    // ── 사용 가능 모델 ──────────────────────────────────────────────────────

    fn available_tab(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, downloads: &Downloads) {
        let entries = available_entries(&cx.shared.paths, &cx.config.postprocess);

        // 빈 목록의 모델 준비 안내와 선택한 모델의 안내가 겹치지 않게 합니다.
        let issue = selection_issue(&cx.shared.paths, &cx.config.postprocess);
        if let Some(issue) = issue
            && !(entries.is_empty()
                && matches!(
                    issue,
                    SelectionIssue::NotInstalled { .. } | SelectionIssue::UnknownModel { .. }
                ))
        {
            let message = issue_text(&cx.shared.paths, &cx.config.postprocess, &issue);
            match issue {
                SelectionIssue::NoProvider | SelectionIssue::ProviderIncomplete { .. } => {
                    Banner::new(Tone::Warning).line(message).show(ui);
                }
                _ => parts::model_notice(ui, &message),
            }
            ui.add_space(8.0);
        }

        self.list
            .begin(|key| entries.iter().any(|entry| &entry.key == key));
        if entries.is_empty() {
            self.list.end();
            if parts::empty_state(ui) {
                self.tab = Tab::Download;
            }
            return;
        }

        let mut turn_on = None;
        let mut previous_local = None;
        for entry in &entries {
            let local = matches!(entry.key, LlmEntryKey::Local { .. });
            parts::list_group(ui, &mut previous_local, local);
            if self.entry_card(ui, cx, downloads, entry) {
                turn_on = Some(entry.key.clone());
            }
            ui.add_space(CARD_GAP);
        }
        self.list.end();

        // 스위치를 켠 항목으로 바꿉니다. 다른 항목의 스위치는 다음 프레임부터 꺼진 모습이 됩니다.
        if let Some(key) = turn_on
            && !cx.locked
        {
            activate(&mut cx.config.postprocess, &key);
        }
    }

    /// 항목 카드 하나를 그립니다. 스위치를 눌러 켰으면 `true`를 돌려줍니다.
    fn entry_card(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        downloads: &Downloads,
        entry: &LlmEntry,
    ) -> bool {
        let head = match &entry.kind {
            LlmEntryKind::Local { family, variant } => local_head(family, variant),
            LlmEntryKind::Provider(profile) => {
                parts::provider_head(Category::Llm, profile, entry.ready)
            }
        };
        let open = self.list.is_open(&entry.key);
        // 값이 덜 채워진 제공자도 켤 수 있습니다. 무엇이 빠졌는지는 목록 위의 배너가 알립니다.
        let outcome = parts::entry_card(
            ui,
            ("llm_entry", &entry.key),
            &head,
            open,
            entry.selected,
            !cx.locked,
            |ui| match &entry.kind {
                LlmEntryKind::Local { family, variant } => {
                    self.local_body(ui, cx, downloads, family, variant);
                }
                LlmEntryKind::Provider(profile) => self.provider_body(ui, cx, profile),
            },
        );
        self.list.after_card(ui, &entry.key, &outcome);
        outcome.turned_on
    }

    /// 로컬 모델: 모델 정보, 파일 위치, 모델 폴더 열기와 제거
    fn local_body(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        downloads: &Downloads,
        family: &LlmFamily,
        variant: &ModelVariant,
    ) {
        let palette = Palette::of(ui);
        let note = license_note(family);
        tile::panel(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            parts::fact(
                ui,
                &t!("models.llm.info.family"),
                &tr(&family.name_key),
                palette.text,
            );
            for choice in &variant.choices {
                let value = tr_opt(&choice.label_key).unwrap_or_else(|| choice.value.clone());
                parts::fact(ui, &tr(&axis_name_key(choice.axis)), &value, palette.text);
            }
            parts::fact(
                ui,
                &t!("models.llm.info.file_size"),
                &format_bytes(variant.total_bytes()),
                palette.text,
            );
            // 쓰임새가 제한되는 라이선스는 경고색으로 적어 눈에 띄게 합니다.
            let (license, color) = match &note {
                Some(note) => (format!("{} · {note}", family.license), palette.warning_text),
                None => (family.license.to_owned(), palette.text),
            };
            parts::fact(ui, &t!("models.llm.info.license"), &license, color);
        });
        ui.add_space(4.0);
        if let Some(description) = tr_opt(&family.description_key) {
            ui.label(text::caption(ui, description));
        }
        model_card_link(ui, family);
        ui.add_space(BODY_GAP);

        if let Some(path) = variant.primary_path() {
            parts::sub_title(ui, &t!("models.llm.info.location"));
            ui.add(text::path_label(ui, path));
            ui.add_space(BODY_GAP);
        }

        // 내려받는 중에는 모델 폴더가 바뀌고 있으므로 지우지 않습니다.
        let downloading = downloads
            .active()
            .is_some_and(|download| download.target.is_llm());
        let remove_blocked = if cx.locked {
            Some(t!("models.available.remove_blocked.locked"))
        } else if downloading {
            Some(t!("models.llm.remove_blocked.downloading"))
        } else {
            None
        };
        let mut open_dir = false;
        let mut remove = false;
        parts::action_row(
            ui,
            |ui| {
                let label = t!("models.download.open_dir");
                open_dir = controls::icon_text_button(ui, Icon::FolderOpen, label).clicked();
            },
            |ui| remove = parts::remove_button(ui, remove_blocked.as_deref()),
        );
        if open_dir {
            let folder = variant_folder(variant, models_root(&cx.shared.paths));
            open_folder(cx.shared, &folder);
        }
        if remove {
            self.confirm = Some(PendingRemoval::Variant {
                variant_id: variant.id.clone(),
                label: variant_label(variant),
            });
        }
    }

    /// 외부 제공자: 이름·주소·API 키·모델 편집, 연결 테스트와 제거
    fn provider_body(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, shown: &ApiProfile) {
        let profile_id = shown.id.as_str();
        let Some(index) = cx
            .config
            .postprocess
            .api_profiles
            .iter()
            .position(|profile| profile.id == profile_id)
        else {
            return;
        };

        ui.add_enabled_ui(!cx.locked, |ui| {
            let profile = &mut cx.config.postprocess.api_profiles[index];
            parts::provider_row(ui, Category::Llm, &profile.provider);
            row::separator(ui);
            parts::profile_fields(
                ui,
                Category::Llm,
                profile_id,
                profile,
                &mut self.list.reveal_api_key,
            );
        });
        ui.add_space(BODY_GAP);

        let target = LlmTestTarget::Provider(profile_id.to_owned());
        let test_state = self.test_state(&target);
        let remove_blocked = if cx.locked {
            Some(t!("models.available.remove_blocked.locked"))
        } else if test_state == TestState::Running {
            Some(t!("models.available.remove_blocked.testing"))
        } else {
            None
        };
        let mut test_click = None;
        let mut remove = false;
        parts::action_row(
            ui,
            |ui| test_click = parts::test_button(ui, &t!("models.providers.test"), test_state),
            |ui| remove = parts::remove_button(ui, remove_blocked.as_deref()),
        );
        test_status(ui, self.test.as_ref(), self.last_test.as_ref(), &target);

        match test_click {
            Some(TestClick::Start) => {
                // 설정에 저장된 값이 아니라 지금 카드에 적힌 값으로 테스트합니다.
                let profile = cx.config.postprocess.api_profiles[index].clone();
                self.start_test(ui.ctx(), cx.shared, target, profile);
            }
            Some(TestClick::Cancel) => self.cancel_test(),
            None => {}
        }
        if remove {
            self.confirm = Some(PendingRemoval::Profile {
                id: profile_id.to_owned(),
                name: profile_display_name(&cx.config.postprocess.api_profiles[index]),
            });
        }
    }

    /// 제거 확인 창을 띄우고, 확인을 받으면 제거합니다.
    pub fn confirm_modal(&mut self, ui: &Ui, cx: &mut PageContext<'_>) {
        let Some(pending) = self.confirm.take() else {
            return;
        };
        let (id, title, body) = match &pending {
            PendingRemoval::Variant { label, .. } => (
                "confirm_delete_llm_model",
                t!("models.remove.variant.title"),
                t!("models.remove.llm_variant.body", model = label),
            ),
            PendingRemoval::Profile { name, .. } => (
                "confirm_delete_llm_provider",
                t!("models.remove.provider.title"),
                t!("models.remove.provider.body", name = name),
            ),
        };
        match parts::confirm_removal(ui, id, &title, &body) {
            Some(Answer::Confirm) => self.remove(cx, pending),
            Some(Answer::Cancel) => {}
            None => self.confirm = Some(pending),
        }
    }

    /// 항목을 제거합니다. 다듬기에 쓰던 항목이었으면 준비된 첫 항목을 대신 고르거나 선택을 비웁니다
    /// (그 규칙은 `sulsul_llm::entries`에 있습니다).
    fn remove(&mut self, cx: &mut PageContext<'_>, pending: PendingRemoval) {
        if cx.locked {
            return;
        }
        match pending {
            PendingRemoval::Variant { variant_id, label } => {
                let deleted =
                    delete_local(&cx.shared.paths, &mut cx.config.postprocess, &variant_id);
                if let Err(error) = deleted {
                    tracing::warn!("could not delete LLM model {variant_id}: {error}");
                    cx.shared.toasts.error(t!(
                        "models.remove.variant.failed",
                        message = error.to_string()
                    ));
                    return;
                }
                cx.shared
                    .toasts
                    .info(t!("models.remove.variant.done", model = label));
                self.list.forget(&LlmEntryKey::local(&variant_id));
            }
            PendingRemoval::Profile { id, name } => {
                remove_profile(&cx.shared.paths, &mut cx.config.postprocess, &id);
                cx.shared
                    .toasts
                    .info(t!("models.remove.provider.done", name = name));
                self.list.forget(&LlmEntryKey::provider(&id));
                self.forget_test(&LlmTestTarget::Provider(id));
            }
        }
    }

    /// 다른 탭에서 "사용 가능 모델" 탭으로 넘어와 `key` 카드를 펼쳐 보여 줍니다.
    fn show_entry(&mut self, key: LlmEntryKey) {
        self.tab = Tab::Available;
        self.list.show(key);
    }

    // ── 로컬 모델 내려받기 ──────────────────────────────────────────────────

    fn download_tab(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, downloads: &mut Downloads) {
        let families = families(&cx.shared.paths);
        let Some(first) = families.first() else {
            ui.label(text::muted(ui, t!("models.download.no_engines")));
            return;
        };

        // 처음 열 때는 지금 고른 로컬 모델의 계열을, 고른 모델이 없으면 첫 계열을 보여 줍니다.
        if families.iter().all(|f| f.id != self.picker.group_id()) {
            let chosen = chosen_local_model(&cx.config.postprocess);
            let initial = families
                .iter()
                .find(|f| f.variants.iter().any(|variant| variant.id == chosen))
                .unwrap_or(first);
            self.picker.select_group(&initial.id, &initial.variants);
        }
        let family = families
            .iter()
            .find(|f| f.id == self.picker.group_id())
            .unwrap_or(first);
        self.picker.normalize(&family.variants);

        ui.label(text::caption(ui, t!("models.download.help.llm")));
        ui.add_space(6.0);
        let names: Vec<String> = families.iter().map(|f| tr(&f.name_key)).collect();
        let groups: Vec<PickerGroup<'_>> = families
            .iter()
            .zip(&names)
            .map(|(f, name)| PickerGroup { id: &f.id, name })
            .collect();
        let switched = picker_row(
            ui,
            "llm_download",
            &mut self.picker,
            &groups,
            &family.variants,
        );
        ui.add_space(SECTION_GAP - 6.0);

        let variant = self.picker.variant(&family.variants);
        let target = variant.map(|variant| DownloadTarget::llm(&variant.id));
        match (variant, &target) {
            (Some(variant), Some(target)) => {
                self.family_variant(ui, cx, downloads, family, variant, target);
            }
            _ => {
                ui.label(text::muted(ui, t!("models.download.no_variant")));
            }
        }
        other_download(ui, downloads, target.as_ref());

        // 계열을 바꿨으면 다음 프레임부터 그 계열의 단계와 모델을 보여 줍니다.
        if let Some(family) = switched.and_then(|id| families.iter().find(|f| f.id == id)) {
            self.picker.select_group(&family.id, &family.variants);
        }
    }

    /// 고른 글 다듬기 모델의 카드와, 카드에서 누른 버튼의 동작
    fn family_variant(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        downloads: &mut Downloads,
        family: &LlmFamily,
        variant: &ModelVariant,
        target: &DownloadTarget,
    ) {
        let click = variant_card(ui, variant, target, downloads, |ui| {
            if let Some(description) = tr_opt(&family.description_key) {
                ui.label(text::caption(ui, description));
            }
            row::title_line(ui, |ui| {
                controls::badge(ui, &tr(&family.name_key));
                controls::badge(
                    ui,
                    &t!("models.llm.license_badge", license = family.license),
                );
                if let Some(note) = license_note(family) {
                    Badge::warning(note).show(ui);
                }
            });
            model_card_link(ui, family);
        });
        match click {
            Some(VariantClick::Start) => {
                downloads.start(ui.ctx(), target.clone(), variant, variant_label(variant));
            }
            Some(VariantClick::Cancel) => downloads.cancel(),
            Some(VariantClick::View) => self.show_entry(LlmEntryKey::local(&variant.id)),
            Some(VariantClick::OpenFolder) => {
                let folder = variant_folder(variant, models_root(&cx.shared.paths));
                open_folder(cx.shared, &folder);
            }
            None => {}
        }
    }

    // ── 외부 제공자 ─────────────────────────────────────────────────────────

    fn providers_tab(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let target = LlmTestTarget::Draft;
        let test_state = self.test_state(&target);
        let outcome = draft_form(
            ui,
            Category::Llm,
            &mut self.draft,
            &cx.config.postprocess.api_profiles,
            cx.locked,
            test_state,
            |ui| test_status(ui, self.test.as_ref(), self.last_test.as_ref(), &target),
        );

        match outcome.test {
            Some(TestClick::Start) => {
                // 초안의 값으로 테스트합니다. 설정은 바꾸지 않습니다.
                if let Some(profile) = self.draft.profile().cloned()
                    && self.start_test(ui.ctx(), cx.shared, target, profile)
                {
                    self.draft.begin_test();
                }
            }
            Some(TestClick::Cancel) => self.cancel_test(),
            None => {}
        }
        if outcome.register {
            self.register_draft(cx);
        }
    }

    /// 초안을 설정에 등록하고 "사용 가능 모델" 탭에서 그 카드를 펼쳐 보여 줍니다.
    fn register_draft(&mut self, cx: &mut PageContext<'_>) {
        let registered = self
            .draft
            .take_registered(&cx.config.postprocess.api_profiles, cx.locked);
        let Some(profile) = registered else {
            return;
        };
        self.forget_test(&LlmTestTarget::Draft);

        let name = profile_display_name(&profile);
        let key = register(&cx.shared.paths, &mut cx.config.postprocess, profile);
        cx.shared
            .toasts
            .info(t!("models.providers.registered", name = name));
        self.show_entry(key);
    }

    // ── 연결 테스트 ─────────────────────────────────────────────────────────

    /// `target`의 연결 테스트 버튼이 보일 모습
    fn test_state(&self, target: &LlmTestTarget) -> TestState {
        TestState::of(self.test.as_ref().map(|test| &test.target), target)
    }

    fn cancel_test(&self) {
        if let Some(test) = &self.test {
            test.handle.cancel();
        }
    }

    /// `profile`의 값으로 연결 테스트를 작업 스레드에서 시작합니다.
    /// 테스트는 한 번에 하나만 돌립니다. 시작했으면 `true`를 돌려줍니다.
    fn start_test(
        &mut self,
        egui_ctx: &egui::Context,
        shared: &mut Shared,
        target: LlmTestTarget,
        profile: ApiProfile,
    ) -> bool {
        if self.test.is_some() {
            return false;
        }
        self.last_test = None;
        let repaint_ctx = egui_ctx.clone();
        match ConnectionTest::start(profile, move || repaint_ctx.request_repaint()) {
            Ok(handle) => {
                self.test = Some(LlmTest { target, handle });
                true
            }
            Err(error) => {
                tracing::error!("could not start the connection test thread: {error}");
                shared.toasts.error(t!("models.llm.test.not_started"));
                false
            }
        }
    }

    /// `target`의 지난 테스트 결과를 지웁니다.
    fn forget_test(&mut self, target: &LlmTestTarget) {
        if self
            .last_test
            .as_ref()
            .is_some_and(|outcome| &outcome.target == target)
        {
            self.last_test = None;
        }
    }
}

/// 방금 다 받은 로컬 모델이 있을 때 부릅니다. 지금 설정으로 다듬는 데 쓸 수 있는 것이 없으면
/// (아무것도 고르지 않았거나 고른 것을 쓸 수 없으면) 그 모델을 고릅니다.
pub(super) fn use_downloaded(cx: &mut PageContext<'_>, variant_id: &str) {
    choose_downloaded(&cx.shared.paths, &mut cx.config.postprocess, variant_id);
}

fn choose_downloaded(paths: &AppPaths, config: &mut PostProcessConfig, variant_id: &str) {
    let entries = available_entries(paths, config);
    if selection_ready(&entries) {
        return;
    }
    let key = LlmEntryKey::local(variant_id);
    if entries.iter().any(|entry| entry.key == key && entry.ready) {
        activate(config, &key);
    }
}

/// 로컬 모델의 파일을 지웁니다. 다듬는 데 쓰던 모델이었으면 준비된 첫 항목을 대신 고르거나 선택을 비웁니다.
/// 파일을 지우지 못했으면 설정을 건드리지 않고 오류를 돌려줍니다.
fn delete_local(
    paths: &AppPaths,
    config: &mut PostProcessConfig,
    variant_id: &str,
) -> std::io::Result<()> {
    if let Some((family, variant)) = find_variant(paths, variant_id) {
        delete_variant(&variant, &family.variants, &models_root(paths))?;
    }
    on_local_removed(paths, config, variant_id);
    Ok(())
}

/// 새 제공자를 설정에 넣고 그 항목의 키를 돌려줍니다.
/// 지금 설정으로 다듬는 데 쓸 수 있는 것이 없을 때만 새 제공자를 바로 고릅니다.
fn register(paths: &AppPaths, config: &mut PostProcessConfig, profile: ApiProfile) -> LlmEntryKey {
    let key = LlmEntryKey::provider(&profile.id);
    let usable = selection_ready(&available_entries(paths, config));
    config.api_profiles.push(profile);
    if !usable {
        activate(config, &key);
    }
    key
}

/// 로컬 모델 카드의 머리 줄: 모델 이름, 계열·크기·라이선스 주의, 짧은 설명
fn local_head(family: &LlmFamily, variant: &ModelVariant) -> CardHead {
    let mut badges = vec![
        Badge::plain(tr(&family.name_key)),
        Badge::plain(format_bytes(variant.total_bytes())),
    ];
    // 쓰임새가 제한되는 모델(비상업 라이선스 등)은 카드를 펼치지 않아도 알 수 있게 합니다.
    if let Some(note) = license_note(family) {
        badges.push(Badge::warning(note));
    }
    if variant.recommended {
        badges.push(Badge::accent(t!("models.badge.recommended")));
    }
    CardHead {
        name: variant_label(variant),
        badges,
        description: tr_opt(&variant.description_key),
    }
}

/// 라이선스 때문에 쓰임새가 제한되는 계열이면 그 주의 문구 (예: "비상업적 용도 전용").
///
/// 계열 설명 키 옆의 `license_note` 번역이 있을 때만 나옵니다. 어느 계열이 해당하는지는 화면 코드가
/// 아니라 번역 파일이 정합니다.
fn license_note(family: &LlmFamily) -> Option<String> {
    let key = family.description_key.strip_suffix(".description")?;
    tr_opt(&format!("{key}.license_note"))
}

/// 모델 계열의 소개 페이지(모델 카드)로 가는 링크
fn model_card_link(ui: &mut Ui, family: &LlmFamily) {
    let label = t!("models.llm.info.model_card");
    ui.hyperlink_to(&*label, family.homepage);
}

/// 지금 설정이 가리키는 것을 쓸 수 없는 이유를 화면 문구로 바꿉니다.
fn issue_text(paths: &AppPaths, config: &PostProcessConfig, issue: &SelectionIssue) -> String {
    match issue {
        SelectionIssue::NotInstalled { variant_id } => {
            let model = find_variant(paths, variant_id).map_or_else(
                || variant_id.clone(),
                |(_, variant)| variant_label(&variant),
            );
            t!("models.llm.issue.not_installed", model = model)
        }
        SelectionIssue::UnknownModel { variant_id } if variant_id.is_empty() => {
            t!("models.llm.issue.no_model")
        }
        SelectionIssue::UnknownModel { variant_id } => {
            t!("models.llm.issue.unknown_model", id = variant_id)
        }
        SelectionIssue::NoProvider => t!("models.available.issue.no_provider"),
        SelectionIssue::ProviderIncomplete { profile_id } => {
            let name = config
                .api_profiles
                .iter()
                .find(|profile| &profile.id == profile_id)
                .map_or_else(|| profile_id.clone(), profile_display_name);
            t!("models.available.issue.provider_incomplete", name = name)
        }
        SelectionIssue::UnknownProcessor { id } => {
            t!("models.llm.issue.unknown_processor", id = id)
        }
    }
    .into_owned()
}

/// `target`의 연결 테스트 진행 상황과 마지막 결과를 보여 줍니다.
fn test_status(
    ui: &mut Ui,
    running: Option<&LlmTest>,
    last: Option<&LlmTestOutcome>,
    target: &LlmTestTarget,
) {
    if let Some(test) = running.filter(|test| &test.target == target) {
        let elapsed = format::seconds_text(test.handle.elapsed());
        parts::progress_line(ui, &t!("models.test.requesting", elapsed = elapsed));
        // 이 PC에서 도는 서버는 첫 요청에서 모델을 올리느라 오래 걸리기도 합니다.
        let local_server = profiles::find_preset(Category::Llm, &test.handle.profile().provider)
            .is_some_and(|preset| preset.runs_locally && !preset.is_mock(Category::Llm));
        if local_server {
            ui.label(text::caption(ui, t!("models.llm.test.local_hint")));
        }
    }

    let Some(outcome) = last.filter(|outcome| &outcome.target == target) else {
        return;
    };
    ui.add_space(6.0);
    match &outcome.result {
        Ok(reply) => {
            let mut banner = Banner::new(Tone::Success).line(t!(
                "models.llm.test.timing",
                elapsed = format::elapsed_text(reply.elapsed)
            ));
            let model = reply.model.as_deref().map(str::trim).unwrap_or_default();
            if !model.is_empty() {
                banner = banner.line(t!("models.llm.test.model", model = model));
            }
            let reply_text = shorten(reply.text.trim(), MAX_REPLY_CHARS);
            let detail = if reply_text.is_empty() {
                t!("models.llm.test.empty_reply")
            } else {
                t!("models.test.transcript", text = reply_text)
            };
            banner
                .title(&t!("models.test.success_title"))
                .line(detail)
                .show(ui);
        }
        Err(LlmError::Cancelled) => {
            Banner::new(Tone::Info)
                .line(t!("models.test.cancelled"))
                .show(ui);
        }
        Err(error) => {
            Banner::new(Tone::Error)
                .line(t!("models.test.failed", message = error_text::llm(error)))
                .show(ui);
        }
    }
}

/// 글이 `max_chars` 글자보다 길면 잘라 끝에 말줄임표를 붙입니다.
fn shorten(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sulsul_llm::presets::profile_from_preset;
    use sulsul_llm::{CLOUD_PROCESSOR_ID, LOCAL_PROCESSOR_ID};

    use super::*;

    const SMALL: &str = "qwen3_5-2b-q4_k_m";
    const OTHER: &str = "gemma4-e2b-q4_k_m";

    struct Fixture {
        _dir: tempfile::TempDir,
        paths: AppPaths,
    }

    impl Fixture {
        fn new() -> Self {
            rust_i18n::set_locale("ko");
            let dir = tempfile::tempdir().unwrap();
            let paths = AppPaths::under(dir.path());
            Self { _dir: dir, paths }
        }

        /// 카탈로그 모델의 파일을 빈 내용으로 만들어 설치된 것처럼 꾸밉니다.
        fn install(&self, variant_id: &str) {
            let (_, variant) = find_variant(&self.paths, variant_id).unwrap();
            for file in &variant.files {
                fs::create_dir_all(file.path.parent().unwrap()).unwrap();
                fs::write(&file.path, b"gguf").unwrap();
            }
        }
    }

    fn profile(id: &str) -> ApiProfile {
        let mut profile = profile_from_preset("openai", id.into(), format!("이름 {id}"));
        profile.api_key = "sk-test".into();
        profile
    }

    #[test]
    fn downloaded_model_is_chosen_only_when_nothing_usable_is_chosen() {
        let fixture = Fixture::new();
        let mut config = PostProcessConfig::default();

        // 파일이 아직 없으면(내려받기가 끝나지 않았으면) 고르지 않습니다.
        choose_downloaded(&fixture.paths, &mut config, SMALL);
        assert_eq!(config.processor, "");

        // 아무것도 고르지 않았으면 방금 받은 모델을 고릅니다.
        fixture.install(SMALL);
        choose_downloaded(&fixture.paths, &mut config, SMALL);
        assert_eq!(config.processor, LOCAL_PROCESSOR_ID);
        assert_eq!(chosen_local_model(&config), SMALL);
        assert!(
            !config.enabled,
            "모델을 골라도 다듬기 스위치는 건드리지 않습니다"
        );

        // 이미 쓸 수 있는 모델을 골라 두었으면 다른 모델을 받아도 바꾸지 않습니다.
        fixture.install(OTHER);
        choose_downloaded(&fixture.paths, &mut config, OTHER);
        assert_eq!(chosen_local_model(&config), SMALL);
    }

    #[test]
    fn deleting_the_chosen_model_moves_the_selection() {
        let fixture = Fixture::new();
        let mut config = PostProcessConfig::default();
        fixture.install(SMALL);
        fixture.install(OTHER);
        choose_downloaded(&fixture.paths, &mut config, SMALL);
        let file_of = |variant_id: &str| {
            let (_, variant) = find_variant(&fixture.paths, variant_id).unwrap();
            variant.primary_path().unwrap().to_owned()
        };

        // 쓰지 않는 모델을 지우면 파일만 없어지고 고른 모델은 그대로입니다.
        delete_local(&fixture.paths, &mut config, OTHER).unwrap();
        assert!(!file_of(OTHER).exists());
        assert!(file_of(SMALL).exists());
        assert_eq!(chosen_local_model(&config), SMALL);

        // 고른 모델을 지우면 남은 항목이 없으므로 선택이 비워집니다.
        delete_local(&fixture.paths, &mut config, SMALL).unwrap();
        assert!(!file_of(SMALL).exists());
        assert_eq!(config.processor, "");
        assert!(available_entries(&fixture.paths, &config).is_empty());
        assert!(
            models_root(&fixture.paths).parent().unwrap().is_dir(),
            "모델 폴더는 남아야 합니다"
        );
    }

    #[test]
    fn registered_provider_is_chosen_only_when_nothing_usable_is_chosen() {
        let fixture = Fixture::new();
        let mut config = PostProcessConfig::default();

        let first = register(&fixture.paths, &mut config, profile("openai"));
        assert_eq!(first, LlmEntryKey::provider("openai"));
        assert_eq!(config.processor, CLOUD_PROCESSOR_ID);
        assert_eq!(config.api_profile, "openai");

        // 이미 쓸 수 있는 제공자를 골라 두었으면 새 제공자를 등록해도 바꾸지 않습니다.
        register(&fixture.paths, &mut config, profile("openai-2"));
        assert_eq!(config.api_profile, "openai");
        assert_eq!(config.api_profiles.len(), 2);
    }

    #[test]
    fn every_selection_issue_has_its_own_wording() {
        let fixture = Fixture::new();
        let mut config = PostProcessConfig::default();
        config.api_profiles.push(profile("work"));
        let issues = [
            SelectionIssue::NotInstalled {
                variant_id: SMALL.into(),
            },
            SelectionIssue::UnknownModel {
                variant_id: String::new(),
            },
            SelectionIssue::UnknownModel {
                variant_id: "no-such-model".into(),
            },
            SelectionIssue::NoProvider,
            SelectionIssue::ProviderIncomplete {
                profile_id: "work".into(),
            },
            SelectionIssue::UnknownProcessor {
                id: "no_such_processor".into(),
            },
        ];
        let texts: Vec<String> = issues
            .iter()
            .map(|issue| issue_text(&fixture.paths, &config, issue))
            .collect();
        for (issue, text) in issues.iter().zip(&texts) {
            assert!(
                !text.starts_with("models.") && !text.contains("%{"),
                "{issue:?}: 문구가 덜 채워졌습니다 ({text})"
            );
        }
        let mut unique = texts.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), texts.len(), "{texts:#?}");

        // 모델과 제공자는 ID가 아니라 화면에 보이는 이름으로 알립니다.
        let (_, variant) = find_variant(&fixture.paths, SMALL).unwrap();
        assert!(texts[0].contains(&variant_label(&variant)), "{}", texts[0]);
        assert!(texts[4].contains("이름 work"), "{}", texts[4]);
    }

    /// 비상업 라이선스인 계열은 카드 머리 줄과 내려받기 카드에 주의 배지가 붙어야 합니다.
    #[test]
    fn restricted_license_is_flagged_on_the_card() {
        let fixture = Fixture::new();
        let families = families(&fixture.paths);
        let family = |id: &str| families.iter().find(|family| family.id == id).unwrap();

        let exaone = family("exaone3_5");
        assert!(
            exaone.license.contains("NC"),
            "EXAONE 3.5의 라이선스가 바뀌었으면 주의 문구도 다시 확인하세요: {}",
            exaone.license
        );
        let note = license_note(exaone).expect("비상업 라이선스 주의 문구가 없습니다");
        assert!(note.contains("비상업"), "{note}");
        // 설명에서도 드러나야 합니다.
        assert!(tr(&exaone.description_key).contains("비상업"));
        for variant in &exaone.variants {
            assert!(
                tr(&variant.description_key).contains("비상업"),
                "{}: 설명에 비상업 라이선스가 드러나지 않습니다",
                variant.id
            );
        }

        // 제한이 없는 계열에는 주의 배지를 붙이지 않습니다.
        assert_eq!(license_note(family("qwen3_5")), None);
        assert_eq!(license_note(family("gemma4")), None);
    }

    #[test]
    fn long_replies_are_shortened() {
        assert_eq!(shorten("OK", 5), "OK");
        assert_eq!(shorten("다섯 글자", 5), "다섯 글자");
        assert_eq!(shorten("여섯 글자다", 5), "여섯 글자…");
        assert_eq!(shorten("", 5), "");
    }
}
