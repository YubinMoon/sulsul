//! "사용 가능 엔진" 탭: 지금 바로 쓸 수 있는 로컬 모델과 외부 제공자의 목록.
//!
//! 항목마다 펼침 카드 하나를 두고, 스위치로 받아쓰기에 쓸 항목 하나를 고릅니다.
//! 카드를 펼치면 그 자리에서 설정을 고치고, 테스트하고, 제거할 수 있습니다.

use std::path::Path;

use egui::{Align, Ui};
use rust_i18n::t;
use sulsul_core::settings::SettingField;
use sulsul_core::stt::{
    EngineContext, LANGUAGE_KEY, LanguageSupport, ModelVariant, delete_variant, format_bytes,
};
use sulsul_stt_cloud::ENGINE_ID as CLOUD_ENGINE_ID;

use super::entries::{
    Entry, EntryKey, EntryKind, MODEL_PATH_KEY, activate, activate_first_ready, available_entries,
    clear_custom_path, entry_languages, entry_test_config, profile_display_name, provider_label,
    remove_profile, selection_issue, variant_label,
};
use super::jobs::TestTarget;
use super::parts::{self, TestClick};
use super::{ModelsPage, SttTab};
use crate::accel::{AccelEnv, EngineAccel};
use crate::i18n::{tr, tr_opt};
use crate::icons::Icon;
use crate::pages::SECTION_GAP;
use crate::session::PageContext;
use crate::theme::Palette;
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card;
use crate::widgets::confirm::{Answer, Confirm};
use crate::widgets::controls;
use crate::widgets::expander::Expander;
use crate::widgets::language::language_setting_row;
use crate::widgets::row::{self, Row};
use crate::widgets::setting_field::setting_field;
use crate::widgets::text;

/// 카드 사이 간격
const CARD_GAP: f32 = 4.0;
/// 펼친 카드 안에서 묶음 사이 간격
const BODY_GAP: f32 = 12.0;
/// 카드로 화면을 옮길 때 카드 위에 남기는 여백
const REVEAL_MARGIN: f32 = 12.0;

/// 사용자에게 확인을 받고 있는 제거
pub(super) enum PendingConfirm {
    /// 내려받은 모델 파일을 지웁니다.
    DeleteVariant {
        engine_id: String,
        variant: ModelVariant,
        label: String,
    },
    /// 직접 지정한 모델 경로를 설정에서 비웁니다 (파일은 남깁니다).
    ClearCustom { engine_id: String },
    /// 등록한 제공자를 지웁니다.
    DeleteProfile { id: String, name: String },
}

impl PendingConfirm {
    /// 제거하려는 목록 항목
    fn key(&self) -> EntryKey {
        match self {
            Self::DeleteVariant {
                engine_id, variant, ..
            } => EntryKey::variant(engine_id, &variant.id),
            Self::ClearCustom { engine_id } => EntryKey::Custom {
                engine_id: engine_id.clone(),
            },
            Self::DeleteProfile { id, .. } => EntryKey::provider(id),
        }
    }
}

/// 카드 머리 줄의 배지
struct Badge {
    text: String,
    tone: BadgeTone,
}

#[derive(Clone, Copy)]
enum BadgeTone {
    Plain,
    /// 추천 모델
    Accent,
    /// 지금은 쓸 수 없는 상태
    Warning,
}

impl Badge {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: BadgeTone::Plain,
        }
    }

    fn accent(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: BadgeTone::Accent,
        }
    }

    fn warning(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: BadgeTone::Warning,
        }
    }

    fn show(&self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        match self.tone {
            BadgeTone::Plain => controls::badge(ui, &self.text),
            BadgeTone::Accent => {
                controls::tone_badge(ui, &self.text, palette.accent_soft, palette.accent_text)
            }
            BadgeTone::Warning => {
                controls::tone_badge(ui, &self.text, palette.warning_soft, palette.warning_text)
            }
        };
    }
}

/// 카드 머리 줄에 보일 내용: 이름, 배지, 짧은 설명
struct EntryHead {
    name: String,
    badges: Vec<Badge>,
    description: Option<String>,
}

impl EntryHead {
    fn of(engine_name: &str, engine_description: Option<String>, entry: &Entry) -> Self {
        match &entry.kind {
            EntryKind::Variant(variant) => {
                let mut badges = vec![
                    Badge::plain(engine_name),
                    Badge::plain(format_bytes(variant.total_bytes())),
                ];
                if variant.recommended {
                    badges.push(Badge::accent(t!("models.badge.recommended")));
                }
                Self {
                    name: variant_label(variant),
                    badges,
                    description: tr_opt(&variant.description_key),
                }
            }
            EntryKind::Custom { .. } => {
                let mut badges = vec![Badge::plain(engine_name)];
                if !entry.ready {
                    badges.push(Badge::warning(t!("models.available.custom.missing_badge")));
                }
                Self {
                    name: t!("models.available.custom.name").into_owned(),
                    badges,
                    description: Some(t!("models.available.custom.description").into_owned()),
                }
            }
            EntryKind::Engine => Self {
                name: engine_name.to_owned(),
                badges: vec![Badge::plain(t!("models.available.engine.no_model_badge"))],
                description: engine_description,
            },
            EntryKind::Provider(profile) => {
                let mut badges = vec![Badge::plain(provider_label(&profile.provider))];
                if !entry.ready {
                    badges.push(Badge::warning(t!(
                        "models.available.provider.incomplete_badge"
                    )));
                }
                let model = profile.model.trim();
                Self {
                    name: profile_display_name(profile),
                    badges,
                    description: (!model.is_empty()).then(|| model.to_owned()),
                }
            }
        }
    }

    fn show(&self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 3.0;
        row::title_line(ui, |ui| {
            ui.label(text::strong(&self.name));
            for badge in &self.badges {
                badge.show(ui);
            }
        });
        if let Some(description) = &self.description {
            ui.label(text::caption(ui, description));
        }
    }
}

impl ModelsPage {
    pub(super) fn available_tab(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let engine_ctx = EngineContext {
            paths: cx.shared.paths.clone(),
        };
        let entries = available_entries(&cx.shared.registry, &engine_ctx, &cx.config.stt);

        // 지금 설정이 가리키는 모델·제공자를 쓸 수 없으면 목록 위에서 먼저 알립니다.
        let issue = selection_issue(&cx.shared.registry, &engine_ctx, &cx.config.stt, &entries);
        if let Some(issue) = issue {
            Banner::new(Tone::Warning).line(issue.text()).show(ui);
            ui.add_space(8.0);
        }

        if entries.is_empty() {
            self.expanded = None;
            self.reveal = None;
            self.empty_state(ui);
            return;
        }
        // 펼쳐 둔 항목이 목록에서 사라졌으면 펼침 상태도 지웁니다.
        if let Some(key) = &self.expanded
            && entries.iter().all(|entry| &entry.key != key)
        {
            self.expanded = None;
        }

        let mut turn_on = None;
        let mut previous_local = None;
        for entry in &entries {
            let local = entry.key.is_local();
            if previous_local != Some(local) {
                if previous_local.is_some() {
                    ui.add_space(SECTION_GAP - CARD_GAP);
                }
                previous_local = Some(local);
                let title = if local {
                    t!("models.available.group.local")
                } else {
                    t!("models.available.group.providers")
                };
                parts::group_title(ui, &title);
            }
            if self.entry_card(ui, cx, entry) {
                turn_on = Some(entry.key.clone());
            }
            ui.add_space(CARD_GAP);
        }
        // 보여 주려던 카드가 목록에 없었으면 그만둡니다. 나중에 생겼을 때 갑자기 화면이 옮겨지지 않게 합니다.
        self.reveal = None;

        // 스위치를 켠 항목으로 바꿉니다. 다른 항목의 스위치는 다음 프레임부터 꺼진 모습이 됩니다.
        if let Some(key) = turn_on
            && !cx.locked
        {
            activate(&cx.shared.registry, &mut cx.config.stt, &key);
        }
    }

    /// 쓸 수 있는 항목이 하나도 없을 때의 안내
    fn empty_state(&mut self, ui: &mut Ui) {
        card::card(ui, |ui| {
            ui.label(text::strong(t!("models.available.empty.title")));
            ui.label(text::muted(ui, t!("models.available.empty.body")));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                if controls::primary_icon_button(
                    ui,
                    Icon::Download,
                    t!("models.available.empty.go_download"),
                )
                .clicked()
                {
                    self.tab = SttTab::Download;
                }
                if controls::icon_text_button(
                    ui,
                    Icon::Cloud,
                    t!("models.available.empty.go_providers"),
                )
                .clicked()
                {
                    self.tab = SttTab::Providers;
                }
            });
        });
    }

    /// 항목 카드 하나를 그립니다. 스위치를 눌러 켰으면 `true`를 돌려줍니다.
    fn entry_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, entry: &Entry) -> bool {
        let descriptor = cx.shared.registry.descriptor(entry.key.engine_id());
        let engine_name = descriptor.map_or_else(
            || entry.key.engine_id().to_owned(),
            |descriptor| tr(&descriptor.name_key),
        );
        let engine_description = descriptor.and_then(|d| tr_opt(&d.description_key));
        let head = EntryHead::of(&engine_name, engine_description, entry);
        let languages = entry_languages(&cx.shared.registry, &cx.config.stt, &entry.key);
        let languages = languages.as_ref();

        let open = self.expanded.as_ref() == Some(&entry.key);
        let shown_on = entry.shown_on();
        // 설정이 잠겨 있거나 파일이 없는 항목은 켤 수 없습니다.
        let can_turn_on = !cx.locked && entry.selectable();
        let toggle_label = t!("models.available.use", name = head.name);
        let mut turned_on = false;

        let shown = Expander::new(&head.name, open).selected(shown_on).show(
            ui,
            ("available_entry", &entry.key),
            |ui| head.show(ui),
            |ui| {
                ui.add_enabled_ui(can_turn_on, |ui| {
                    turned_on = controls::choice_toggle(ui, shown_on, &toggle_label);
                });
            },
            |ui| self.entry_body(ui, cx, entry, &engine_name, languages),
        );

        if shown.toggled {
            self.expanded = (!open).then(|| entry.key.clone());
            self.reveal_api_key = false;
        }
        // 다른 탭에서 "이 항목 보기"로 넘어왔으면 화면을 이 카드로 옮깁니다.
        if self.reveal.as_ref() == Some(&entry.key) {
            self.reveal = None;
            ui.scroll_to_rect(shown.response.rect.expand(REVEAL_MARGIN), Some(Align::Min));
        }
        turned_on
    }

    /// 펼친 카드의 내용. `languages`는 이 항목이 인식할 수 있는 언어입니다.
    fn entry_body(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        entry: &Entry,
        engine_name: &str,
        languages: Option<&LanguageSupport>,
    ) {
        match &entry.kind {
            EntryKind::Provider(_) => self.provider_body(ui, cx, entry, languages),
            EntryKind::Variant(_) | EntryKind::Custom { .. } | EntryKind::Engine => {
                self.local_body(ui, cx, entry, engine_name, languages);
            }
        }
        self.entry_actions(ui, cx, entry, engine_name);
    }

    /// 로컬 항목: 직접 지정한 경로, 엔진 설정, 예상 장치
    fn local_body(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        entry: &Entry,
        engine_name: &str,
        languages: Option<&LanguageSupport>,
    ) {
        let palette = Palette::of(ui);
        let engine_id = entry.key.engine_id();

        if let EntryKind::Custom { path } = &entry.kind {
            parts::sub_title(ui, &t!("models.available.custom.path_title"));
            ui.add(text::path_label(ui, Path::new(path)));
            if !entry.ready {
                ui.label(
                    text::caption(ui, t!("models.available.custom.missing"))
                        .color(palette.warning_text),
                );
            }
            ui.label(text::caption(
                ui,
                t!(
                    "models.available.custom.replaced_note",
                    engine = engine_name
                ),
            ));
            ui.add_space(BODY_GAP);
        }

        // 엔진 설정. 모델을 고르는 값은 이 목록의 스위치가 정하므로 빼고 보여 줍니다.
        let model_key = cx
            .shared
            .registry
            .model_setting_key(engine_id)
            .ok()
            .flatten();
        let fields: Vec<SettingField> = cx
            .shared
            .registry
            .descriptor(engine_id)
            .map(|descriptor| {
                descriptor
                    .settings
                    .iter()
                    .filter(|field| {
                        Some(field.key.as_str()) != model_key && field.key != MODEL_PATH_KEY
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !fields.is_empty() {
            parts::sub_title(ui, &t!("models.available.settings.title"));
            engine_settings(ui, cx, engine_id, &fields, languages);
            ui.add_space(4.0);
            ui.label(text::caption(
                ui,
                t!("models.available.settings.shared", engine = engine_name),
            ));
            ui.add_space(BODY_GAP);
        }

        // 예상 장치. 지금 설정과 찾은 장치로 미리 본 값이고, 실제 장치는 테스트 결과에 나옵니다.
        let accel = EngineAccel::of(
            &cx.shared.registry,
            &cx.config.stt,
            engine_id,
            AccelEnv::current(),
        );
        if let Some(accel) = accel {
            parts::sub_title(ui, &t!("models.accel.expected"));
            parts::accel_panel(ui, &accel, &accel.note(&cx.shared.registry));
            ui.add_space(4.0);
            ui.label(text::caption(ui, t!("models.accel.footnote")));
            ui.add_space(BODY_GAP);
        }
    }

    /// 외부 제공자: 이름·주소·API 키·모델 편집과 클라우드 공통 설정
    fn provider_body(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        entry: &Entry,
        languages: Option<&LanguageSupport>,
    ) {
        let EntryKey::Provider { profile_id } = &entry.key else {
            return;
        };
        let Some(index) = cx
            .config
            .stt
            .api_profiles
            .iter()
            .position(|profile| &profile.id == profile_id)
        else {
            return;
        };

        ui.add_enabled_ui(!cx.locked, |ui| {
            let profile = &mut cx.config.stt.api_profiles[index];
            // 공급자는 등록할 때 정해지고 여기서는 바꾸지 않습니다.
            Row::new(&t!("models.providers.field.provider")).show(ui, |ui| {
                controls::badge(ui, &provider_label(&profile.provider));
            });
            row::separator(ui);
            parts::profile_fields(ui, profile_id, profile, &mut self.reveal_api_key);
        });
        ui.add_space(BODY_GAP);

        let shared_fields = cx
            .shared
            .registry
            .descriptor(CLOUD_ENGINE_ID)
            .map(|descriptor| descriptor.settings.clone())
            .unwrap_or_default();
        if !shared_fields.is_empty() {
            parts::sub_title(ui, &t!("models.providers.shared.title"));
            engine_settings(ui, cx, CLOUD_ENGINE_ID, &shared_fields, languages);
            ui.add_space(4.0);
            ui.label(text::caption(ui, t!("models.providers.shared.note")));
            ui.add_space(BODY_GAP);
        }
    }

    /// 테스트·제거 버튼 줄과, 그 아래의 테스트 진행 상황·결과
    fn entry_actions(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        entry: &Entry,
        engine_name: &str,
    ) {
        let target = TestTarget::Entry(entry.key.clone());
        // 모델 파일이 필요 없는 엔진은 지울 것이 없습니다.
        let removable = !matches!(entry.kind, EntryKind::Engine);
        // 같은 엔진의 모델은 파일을 함께 쓰기도 하므로, 그 엔진을 내려받는 동안에는 지우지 않습니다.
        let downloading = entry.key.is_local()
            && self
                .download
                .as_ref()
                .is_some_and(|download| download.engine_id == entry.key.engine_id());
        let remove_blocked = if cx.locked {
            Some(t!("models.available.remove_blocked.locked"))
        } else if self.is_testing(&target) {
            Some(t!("models.available.remove_blocked.testing"))
        } else if downloading {
            Some(t!("models.available.remove_blocked.downloading"))
        } else {
            None
        };

        let test_label = t!("models.test.button");
        let mut test_click = None;
        let mut remove = false;
        parts::action_row(
            ui,
            |ui| {
                test_click = parts::test_button(ui, &test_label, self.test.as_ref(), &target);
            },
            |ui| {
                if !removable {
                    return;
                }
                ui.add_enabled_ui(remove_blocked.is_none(), |ui| {
                    let button = controls::danger_button(ui, t!("models.available.remove"));
                    remove = match &remove_blocked {
                        Some(reason) => {
                            button.on_disabled_hover_text(reason.as_ref());
                            false
                        }
                        None => button.clicked(),
                    };
                });
            },
        );
        parts::test_progress(ui, self.test.as_ref(), &target);
        parts::test_outcome(ui, self.last_test.as_ref(), &target);

        match test_click {
            Some(TestClick::Start) => {
                let stt = entry_test_config(&cx.shared.registry, &cx.config.stt, &entry.key);
                if let Some(stt) = stt {
                    let engine_id = entry.key.engine_id().to_owned();
                    self.start_test(cx.shared, ui.ctx(), target, stt, engine_id);
                }
            }
            Some(TestClick::Cancel) => self.cancel_test(),
            None => {}
        }
        if remove {
            self.confirm = match &entry.kind {
                EntryKind::Variant(variant) => Some(PendingConfirm::DeleteVariant {
                    engine_id: entry.key.engine_id().to_owned(),
                    variant: variant.clone(),
                    label: format!("{engine_name} · {}", variant_label(variant)),
                }),
                EntryKind::Custom { .. } => Some(PendingConfirm::ClearCustom {
                    engine_id: entry.key.engine_id().to_owned(),
                }),
                EntryKind::Provider(profile) => Some(PendingConfirm::DeleteProfile {
                    id: profile.id.clone(),
                    name: profile_display_name(profile),
                }),
                EntryKind::Engine => None,
            };
        }
    }

    /// 제거 확인 창을 띄우고, 확인을 받으면 제거합니다.
    pub(super) fn confirm_modal(&mut self, ui: &Ui, cx: &mut PageContext<'_>) {
        let Some(pending) = self.confirm.take() else {
            return;
        };
        let (id, title, body) = match &pending {
            PendingConfirm::DeleteVariant { label, .. } => (
                "confirm_delete_model_variant",
                t!("models.remove.variant.title"),
                t!("models.remove.variant.body", model = label),
            ),
            PendingConfirm::ClearCustom { .. } => (
                "confirm_clear_custom_model",
                t!("models.remove.custom.title"),
                t!("models.remove.custom.body"),
            ),
            PendingConfirm::DeleteProfile { name, .. } => (
                "confirm_delete_provider",
                t!("models.remove.provider.title"),
                t!("models.remove.provider.body", name = name),
            ),
        };
        let answer = Confirm {
            id,
            title: &title,
            body: &body,
            confirm_label: &t!("models.available.remove"),
            cancel_label: &t!("common.cancel"),
            danger: true,
        }
        .show(ui);
        match answer {
            Some(Answer::Confirm) => self.remove(cx, pending),
            Some(Answer::Cancel) => {}
            None => self.confirm = Some(pending),
        }
    }

    /// 항목을 제거합니다. 받아쓰기에 쓰던 항목이었으면 남은 항목 중 첫 번째를 대신 켭니다.
    fn remove(&mut self, cx: &mut PageContext<'_>, pending: PendingConfirm) {
        if cx.locked {
            return;
        }
        let engine_ctx = EngineContext {
            paths: cx.shared.paths.clone(),
        };
        let key = pending.key();
        let was_selected = available_entries(&cx.shared.registry, &engine_ctx, &cx.config.stt)
            .iter()
            .any(|entry| entry.key == key && entry.selected);

        match pending {
            PendingConfirm::DeleteVariant {
                engine_id,
                variant,
                label,
            } => {
                let catalog = cx
                    .shared
                    .registry
                    .model_variants(&engine_ctx, &engine_id)
                    .unwrap_or_default();
                let deleted = delete_variant(&variant, &catalog, &cx.shared.paths.models_dir());
                if let Err(error) = deleted {
                    tracing::warn!("could not delete model {engine_id}/{}: {error}", variant.id);
                    cx.shared.toasts.error(t!(
                        "models.remove.variant.failed",
                        message = error.to_string()
                    ));
                    return;
                }
                cx.shared
                    .toasts
                    .info(t!("models.remove.variant.done", model = label));
            }
            PendingConfirm::ClearCustom { engine_id } => {
                clear_custom_path(&mut cx.config.stt, &engine_id);
                cx.shared.toasts.info(t!("models.remove.custom.done"));
            }
            PendingConfirm::DeleteProfile { id, name } => {
                remove_profile(&mut cx.config.stt, &id);
                cx.shared
                    .toasts
                    .info(t!("models.remove.provider.done", name = name));
            }
        }

        if self.expanded.as_ref() == Some(&key) {
            self.expanded = None;
        }
        self.forget_test(&TestTarget::Entry(key));
        if was_selected {
            activate_first_ready(&cx.shared.registry, &engine_ctx, &mut cx.config.stt);
        }
    }

    /// 다른 탭에서 이 탭으로 넘어와 `key` 카드를 펼쳐 보여 줍니다.
    pub(super) fn show_entry(&mut self, key: EntryKey) {
        self.tab = SttTab::Available;
        self.reveal_api_key = false;
        self.expanded = Some(key.clone());
        self.reveal = Some(key);
    }
}

/// 엔진 설정 항목을 그리고, 값이 바뀌면 `[stt.engines.<엔진 ID>]`에 씁니다.
///
/// 인식 언어는 `languages`(이 카드의 모델이 인식할 수 있는 언어) 안에서 고릅니다. 한 언어만
/// 인식하는 모델의 카드에서는 고정으로 보여 주고, 엔진이 함께 쓰는 언어 값은 바꾸지 않습니다.
///
/// 보기만 해서는 설정에 빈 표가 생기지 않도록, 바뀐 값이 있을 때만 표를 넣습니다.
fn engine_settings(
    ui: &mut Ui,
    cx: &mut PageContext<'_>,
    engine_id: &str,
    fields: &[SettingField],
    languages: Option<&LanguageSupport>,
) {
    // 표에 값이 없으면 실제로 쓰이는 언어(기본값이나 예전 전역 설정에서 이어받은 값)를 보여 줍니다.
    let language = cx
        .shared
        .registry
        .resolve_settings(&cx.config.stt, engine_id)
        .map(|settings| settings.str(LANGUAGE_KEY).to_owned())
        .unwrap_or_default();
    ui.add_enabled_ui(!cx.locked, |ui| {
        let mut table = cx
            .config
            .stt
            .engines
            .get(engine_id)
            .cloned()
            .unwrap_or_default();
        let mut changed = false;
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                row::separator(ui);
            }
            changed |= match languages {
                Some(support) if field.key == LANGUAGE_KEY => {
                    let help = field.help_key.as_deref().map(tr).unwrap_or_default();
                    language_setting_row(
                        ui,
                        (engine_id, LANGUAGE_KEY),
                        field,
                        &help,
                        support,
                        &mut table,
                        &language,
                    )
                }
                _ => setting_field(ui, engine_id, field, &mut table, cx.frame),
            };
        }
        if changed {
            cx.config.stt.engines.insert(engine_id.to_owned(), table);
        }
    });
}
