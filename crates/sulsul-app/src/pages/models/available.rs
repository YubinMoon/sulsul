//! 음성 인식 갈래의 "사용 가능 모델" 탭: 지금 바로 쓸 수 있는 로컬 모델과 외부 제공자의 목록.
//!
//! 항목마다 펼침 카드 하나를 두고, 스위치로 받아쓰기에 쓸 항목 하나를 고릅니다.
//! 카드를 펼치면 그 자리에서 설정을 고치고, 테스트하고, 제거할 수 있습니다.
//! 목록 아래에는 모든 모델이 함께 쓰는 "인식 정확도 높이기" 묶음(용어 힌트)이 있습니다.

use std::path::Path;

use egui::{TextEdit, Ui};
use rust_i18n::t;
use sulsul_core::settings::SettingField;
use sulsul_core::stt::{
    EngineContext, LANGUAGE_KEY, LanguageSupport, ModelVariant, delete_variant, format_bytes,
};
use sulsul_stt_cloud::ENGINE_ID as CLOUD_ENGINE_ID;

use super::entries::{
    Entry, EntryKey, EntryKind, MODEL_PATH_KEY, SelectionIssue, activate, activate_first_ready,
    available_entries, clear_custom_path, edit_engine_table, entry_languages, entry_test_config,
    model_path_field, remove_profile, selection_issue, variant_label,
};
use super::jobs::{DownloadTarget, TestTarget};
use super::parts::{self, BODY_GAP, Badge, CARD_GAP, CardHead, ListGroup, TestClick};
use super::profiles::profile_display_name;
use super::{Category, ModelsPage, Tab};
use crate::accel::{AccelEnv, EngineAccel};
use crate::i18n::{tr, tr_opt};
use crate::icons::Icon;
use crate::pages::SECTION_GAP;
use crate::session::PageContext;
use crate::theme::Palette;
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card::Section;
use crate::widgets::confirm::Answer;
use crate::widgets::language::language_setting_row;
use crate::widgets::row::{self, Row};
use crate::widgets::setting_field::setting_field;
use crate::widgets::text;

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

/// 카드 머리 줄에 보일 내용. `languages`는 이 항목이 인식할 수 있는 언어입니다 (`entry_languages`).
fn entry_head(
    engine_name: &str,
    engine_description: Option<String>,
    entry: &Entry,
    languages: Option<&LanguageSupport>,
) -> CardHead {
    match &entry.kind {
        EntryKind::Variant(variant) => {
            let mut badges = vec![
                Badge::plain(engine_name),
                Badge::plain(format_bytes(variant.total_bytes())),
            ];
            // 한 언어만 인식하는 모델(영어 전용 등)은 카드를 펼치지 않아도 알 수 있게 합니다.
            if let Some(languages) = languages.filter(|languages| languages.single().is_some()) {
                badges.push(Badge::plain(parts::language_support_label(languages)));
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
        EntryKind::Custom { .. } => {
            let mut badges = vec![Badge::plain(engine_name)];
            if !entry.ready {
                badges.push(Badge::warning(t!("models.available.custom.missing_badge")));
            }
            CardHead {
                name: t!("models.available.custom.name").into_owned(),
                badges,
                description: Some(t!("models.available.custom.description").into_owned()),
            }
        }
        EntryKind::Engine => CardHead {
            name: engine_name.to_owned(),
            badges: vec![Badge::plain(t!("models.available.engine.no_model_badge"))],
            description: engine_description,
        },
        EntryKind::Provider(profile) => parts::provider_head(Category::Stt, profile, entry.ready),
    }
}

impl ModelsPage {
    pub(super) fn available_tab(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let engine_ctx = EngineContext {
            paths: cx.shared.paths.clone(),
        };
        let entries = available_entries(&cx.shared.registry, &engine_ctx, &cx.config.stt);

        // 빈 목록의 모델 준비 안내와 선택한 모델의 안내가 겹치지 않게 합니다.
        let issue = selection_issue(&cx.shared.registry, &engine_ctx, &cx.config.stt, &entries);
        if let Some(issue) = issue
            && !(entries.is_empty() && matches!(issue, SelectionIssue::NotInstalled { .. }))
        {
            match issue {
                SelectionIssue::NoProvider | SelectionIssue::ProviderIncomplete { .. } => {
                    Banner::new(Tone::Warning).line(issue.text()).show(ui);
                }
                _ => parts::model_notice(ui, &issue.text()),
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
        let mut previous_group = None;
        for entry in &entries {
            let group = if entry.key.is_local() {
                ListGroup::Local
            } else {
                ListGroup::Providers
            };
            parts::list_group(ui, &mut previous_group, group);
            if self.entry_card(ui, cx, entry) {
                turn_on = Some(entry.key.clone());
            }
            ui.add_space(CARD_GAP);
        }
        self.list.end();

        // 스위치를 켠 항목으로 바꿉니다. 다른 항목의 스위치는 다음 프레임부터 꺼진 모습이 됩니다.
        if let Some(key) = turn_on
            && !cx.locked
        {
            activate(&cx.shared.registry, &mut cx.config.stt, &key);
        }

        accuracy_section(ui, cx);
    }

    /// 항목 카드 하나를 그립니다. 스위치를 눌러 켰으면 `true`를 돌려줍니다.
    fn entry_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, entry: &Entry) -> bool {
        let descriptor = cx.shared.registry.descriptor(entry.key.engine_id());
        let engine_name = descriptor.map_or_else(
            || entry.key.engine_id().to_owned(),
            |descriptor| tr(&descriptor.name_key),
        );
        let engine_description = descriptor.and_then(|d| tr_opt(&d.description_key));
        let languages = entry_languages(&cx.shared.registry, &cx.config.stt, &entry.key);
        let languages = languages.as_ref();
        let head = entry_head(&engine_name, engine_description, entry, languages);

        let open = self.list.is_open(&entry.key);
        // 설정이 잠겨 있거나 파일이 없는 항목은 켤 수 없습니다.
        let can_turn_on = !cx.locked && entry.selectable();
        let outcome = parts::entry_card(
            ui,
            ("available_entry", &entry.key),
            &head,
            open,
            entry.shown_on(),
            can_turn_on,
            |ui| self.entry_body(ui, cx, entry, &engine_name, languages),
        );
        self.list.after_card(ui, &entry.key, &outcome);
        outcome.turned_on
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
                local_body(ui, cx, entry, languages);
            }
        }
        self.entry_actions(ui, cx, entry, engine_name);
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
            parts::provider_row(ui, Category::Stt, &profile.provider);
            row::separator(ui);
            parts::profile_fields(
                ui,
                Category::Stt,
                profile_id,
                profile,
                &mut self.list.reveal_api_key,
            );
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
            unknown_keys_warning(ui, cx, CLOUD_ENGINE_ID);
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
            && self.downloads.active().is_some_and(|download| {
                matches!(
                    &download.target,
                    DownloadTarget::Stt { engine_id, .. } if engine_id == entry.key.engine_id()
                )
            });
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
        let test_state = self.test_state(&target);
        let mut test_click = None;
        let mut remove = false;
        parts::action_row(
            ui,
            |ui| test_click = parts::test_button(ui, &test_label, test_state),
            |ui| {
                if removable {
                    remove = parts::remove_button(ui, remove_blocked.as_deref());
                }
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
        match parts::confirm_removal(ui, id, &title, &body) {
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
            // 지운 항목은 목록에서 사라지므로 따로 알리지 않습니다. 실패만 알립니다.
            PendingConfirm::DeleteVariant {
                engine_id, variant, ..
            } => {
                let catalog = cx
                    .shared
                    .registry
                    .model_variants(&engine_ctx, &engine_id)
                    .unwrap_or_default();
                let deleted = delete_variant(&variant, &catalog, &cx.shared.paths.models_dir());
                if let Err(error) = deleted {
                    tracing::warn!("could not delete model {engine_id}/{}: {error}", variant.id);
                    cx.shared.toasts.error(t!("models.remove.variant.failed"));
                    return;
                }
            }
            PendingConfirm::ClearCustom { engine_id } => {
                clear_custom_path(&mut cx.config.stt, &engine_id);
            }
            PendingConfirm::DeleteProfile { id, .. } => {
                remove_profile(&mut cx.config.stt, &id);
            }
        }

        self.list.forget(&key);
        self.forget_test(&TestTarget::Entry(key));
        if was_selected {
            activate_first_ready(&cx.shared.registry, &engine_ctx, &mut cx.config.stt);
        }
    }

    /// 다른 탭에서 이 탭으로 넘어와 `key` 카드를 펼쳐 보여 줍니다.
    pub(super) fn show_entry(&mut self, key: EntryKey) {
        self.tab = Tab::Available;
        self.list.show(key);
    }
}

/// 로컬 항목: 직접 지정한 경로, 엔진 설정, 예상 장치
fn local_body(
    ui: &mut Ui,
    cx: &mut PageContext<'_>,
    entry: &Entry,
    languages: Option<&LanguageSupport>,
) {
    let palette = Palette::of(ui);
    let engine_id = entry.key.engine_id();

    if let EntryKind::Custom { path } = &entry.kind {
        // 직접 지정한 경로는 여기서 고칩니다. 경로를 비우면 이 항목은 목록에서 빠집니다.
        match model_path_field(&cx.shared.registry, engine_id) {
            Some(field) => {
                engine_settings(ui, cx, engine_id, std::slice::from_ref(&field), None);
            }
            None => {
                ui.add(text::path_label(ui, Path::new(path)));
            }
        }
        if !entry.ready {
            ui.label(
                text::caption(ui, t!("models.available.custom.missing"))
                    .color(palette.warning_text),
            );
        }
        ui.label(text::caption(
            ui,
            t!("models.available.custom.replaced_note"),
        ));
        ui.add_space(BODY_GAP);
    }

    // 엔진 설정. 엔진이 스키마에 선언한 항목만 그립니다. 모델을 고르는 값은 이 목록의 스위치가
    // 정하고 직접 지정한 경로는 위에서 따로 고치므로 빼고 보여 줍니다.
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
    let has_unknown_keys = !cx
        .shared
        .registry
        .unknown_keys(&cx.config.stt, engine_id)
        .is_empty();
    if !fields.is_empty() || has_unknown_keys {
        parts::sub_title(ui, &t!("models.available.settings.title"));
        if !fields.is_empty() {
            engine_settings(ui, cx, engine_id, &fields, languages);
        }
        unknown_keys_warning(ui, cx, engine_id);
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

/// 엔진 설정 항목을 그리고, 값이 바뀌면 `[stt.engines.<엔진 ID>]`에 씁니다.
///
/// 인식 언어는 `languages`(이 카드의 모델이 인식할 수 있는 언어) 안에서 고릅니다. 한 언어만
/// 인식하는 모델의 카드에서는 고정으로 보여 주고, 엔진이 함께 쓰는 언어 값은 바꾸지 않습니다.
///
/// 보기만 해서는 설정에 빈 표가 생기지 않도록, 바뀐 값이 있을 때만 표를 넣습니다.
pub(super) fn engine_settings(
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
    let frame = cx.frame;
    ui.add_enabled_ui(!cx.locked, |ui| {
        edit_engine_table(&mut cx.config.stt, engine_id, |table| {
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
                            table,
                            &language,
                        )
                    }
                    _ => setting_field(ui, engine_id, field, table, frame),
                };
            }
            changed
        });
    });
}

/// 엔진 설정 표(`[stt.engines.<엔진 ID>]`)에 그 엔진이 모르는 키가 있으면 알립니다.
/// 오타일 수 있습니다. 더 이상 쓰지 않는 키는 알리지 않습니다.
fn unknown_keys_warning(ui: &mut Ui, cx: &PageContext<'_>, engine_id: &str) {
    let keys = cx.shared.registry.unknown_keys(&cx.config.stt, engine_id);
    if keys.is_empty() {
        return;
    }
    let palette = Palette::of(ui);
    ui.add_space(4.0);
    ui.label(
        text::caption(
            ui,
            t!(
                "models.available.settings.unknown_keys",
                keys = keys.join(", ")
            ),
        )
        .color(palette.warning_text),
    );
}

/// "인식 정확도 높이기" 묶음: 모든 모델이 함께 쓰는 용어 힌트(`stt.initial_prompt`).
///
/// 지금 고른 엔진이 용어 힌트를 쓰지 않으면 묶음을 통째로 숨깁니다. 쓸 수 없는 설정은 보여 주지 않습니다.
fn accuracy_section(ui: &mut Ui, cx: &mut PageContext<'_>) {
    let supports_prompt = cx
        .shared
        .registry
        .descriptor(&cx.config.stt.engine)
        .is_some_and(|descriptor| descriptor.capabilities.supports_prompt);
    if !supports_prompt {
        return;
    }
    ui.add_space(SECTION_GAP - CARD_GAP);
    Section::new(Icon::Target, &t!("models.accuracy.title"))
        .badge(&t!("models.accuracy.shared"))
        .show(ui, |ui| {
            ui.add_enabled_ui(!cx.locked, |ui| {
                Row::new(&t!("models.accuracy.prompt"))
                    .help(&t!("models.accuracy.prompt_help"))
                    .show_below(ui, |ui| {
                        ui.add(
                            TextEdit::multiline(&mut cx.config.stt.initial_prompt)
                                .id_salt("stt_initial_prompt")
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .hint_text(t!("models.accuracy.prompt_hint")),
                        );
                    });
            });
        });
}

#[cfg(test)]
mod tests {
    use sulsul_core::Config;
    use sulsul_core::stt::mock::MOCK_ENGINE_ID;

    use super::*;

    #[test]
    fn language_row_uses_the_engine_schema_field() {
        let registry = crate::engines::registry();
        for descriptor in registry.descriptors() {
            // 지금 등록된 엔진은 모두 언어를 고를 수 있습니다. 카드의 언어 줄은 그 항목으로 그립니다.
            let field = descriptor
                .settings
                .iter()
                .find(|field| field.key == LANGUAGE_KEY);
            assert!(
                field.is_some_and(|field| field.label_key == "stt.language.label"),
                "{}: 인식 언어 항목이 없습니다",
                descriptor.id
            );
        }
    }

    /// 용어 힌트 묶음은 엔진이 알려 준 값(`supports_prompt`)만 보고 보이거나 숨습니다.
    /// 화면 코드에 엔진 이름을 적어 두지 않으므로, 등록된 엔진의 값이 서로 다른지만 확인합니다.
    #[test]
    fn prompt_support_comes_from_the_engine_descriptor() {
        let registry = crate::engines::registry();
        let supports = |engine: &str| {
            let mut config = Config::default();
            config.stt.engine = engine.into();
            registry
                .descriptor(&config.stt.engine)
                .is_some_and(|descriptor| descriptor.capabilities.supports_prompt)
        };
        assert!(
            supports(sulsul_stt_whisper::ENGINE_ID),
            "Whisper는 용어 힌트를 씁니다"
        );
        assert!(
            !supports("no_such_engine"),
            "모르는 엔진이면 용어 힌트 묶음을 숨깁니다"
        );
        // 개발 빌드에만 있는 가짜 엔진은 등록 여부만 봅니다 (값은 엔진이 정합니다).
        let _ = supports(MOCK_ENGINE_ID);
    }
}
