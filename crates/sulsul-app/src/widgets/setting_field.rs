//! 설정 스키마([`SettingField`])만 보고 입력 화면을 그리는 범용 렌더러.
//!
//! 엔진·후처리기는 자기 설정 항목을 스키마로 선언하기만 하면 됩니다.
//! 새 엔진을 추가해도 이 파일은 고칠 필요가 없습니다.

use egui::{DragValue, TextEdit, Ui};
use rust_i18n::t;
use sulsul_core::settings::{PathKind, SettingField, SettingKind};
use sulsul_core::toml::{Table, Value};

use super::controls::{self, ComboItem};
use super::row::{self, Row};
use crate::error_text;
use crate::i18n::tr;
use crate::icons::Icon;

/// 이 범위 이하의 정수는 슬라이더로, 더 넓으면 숫자 입력으로 보여 줍니다.
const SLIDER_MAX_SPAN: i64 = 200;
/// 슬라이더 옆 숫자 칸의 너비
const SLIDER_VALUE_WIDTH: f32 = 48.0;

/// 스키마의 모든 항목을 줄 단위로 그립니다. 값이 바뀌면 `table`에 쓰고 `true`를 돌려줍니다.
///
/// `id_salt`는 위젯 ID가 겹치지 않게 하는 값입니다(예: 엔진 ID).
pub fn setting_fields(
    ui: &mut Ui,
    id_salt: &str,
    schema: &[SettingField],
    table: &mut Table,
    parent: &eframe::Frame,
) -> bool {
    let mut changed = false;
    for (index, field) in schema.iter().enumerate() {
        if index > 0 {
            row::separator(ui);
        }
        changed |= setting_field(ui, id_salt, field, table, parent);
    }
    changed
}

/// 항목 하나를 그립니다.
pub fn setting_field(
    ui: &mut Ui,
    id_salt: &str,
    field: &SettingField,
    table: &mut Table,
    parent: &eframe::Frame,
) -> bool {
    let label = tr(&field.label_key);
    let help = field.help_key.as_deref().map(tr);
    // 저장된 값이 스키마와 맞지 않으면 기본값을 보여 주고 그 사실을 알립니다.
    let stored = table.get(&field.key);
    let problem = stored.and_then(|value| field.check(value).err());
    let warning = problem.map(|problem| {
        let issue = sulsul_core::settings::SettingIssue {
            key: field.key.clone(),
            problem,
        };
        t!(
            "settings.field.invalid_value",
            issue = error_text::setting_issue(&issue)
        )
        .into_owned()
    });
    let current = match stored {
        Some(value) if warning.is_none() => value.clone(),
        _ => field.default.clone(),
    };
    let widget_id = (id_salt, field.key.as_str());

    let row = Row::new(&label)
        .help_opt(help.as_deref())
        .warning_opt(warning.as_deref());

    let new_value = match &field.kind {
        SettingKind::Bool => {
            let mut value = current.as_bool().unwrap_or_default();
            row.show(ui, |ui| controls::toggle(ui, &mut value, &label));
            Value::Boolean(value)
        }
        SettingKind::Integer { min, max } => {
            let mut value = current.as_integer().unwrap_or(*min);
            row.show(ui, |ui| {
                if max.saturating_sub(*min) <= SLIDER_MAX_SPAN {
                    // 슬라이더와 숫자 칸이 순서대로 놓이도록 너비를 잡아 줍니다.
                    let spacing = ui.spacing();
                    let width = spacing.slider_width + spacing.item_spacing.x + SLIDER_VALUE_WIDTH;
                    row::left_to_right(ui, width, |ui| {
                        controls::slider(ui, &mut value, *min..=*max);
                        ui.spacing_mut().interact_size.x = SLIDER_VALUE_WIDTH;
                        ui.add(DragValue::new(&mut value).range(*min..=*max));
                    });
                } else {
                    let speed = ((*max - *min) as f64 / 400.0).max(1.0);
                    ui.add(DragValue::new(&mut value).range(*min..=*max).speed(speed));
                }
            });
            Value::Integer(value)
        }
        SettingKind::Text { multiline: false } => {
            let mut value = current.as_str().unwrap_or_default().to_owned();
            row.show(ui, |ui| {
                ui.add(
                    TextEdit::singleline(&mut value)
                        .id_salt(widget_id)
                        .desired_width(row::CONTROL_WIDTH),
                );
            });
            Value::String(value)
        }
        SettingKind::Text { multiline: true } => {
            let mut value = current.as_str().unwrap_or_default().to_owned();
            row.show_below(ui, |ui| {
                ui.add(
                    TextEdit::multiline(&mut value)
                        .id_salt(widget_id)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
            });
            Value::String(value)
        }
        SettingKind::Secret => {
            let mut value = current.as_str().unwrap_or_default().to_owned();
            let visible_id = ui.make_persistent_id((widget_id, "visible"));
            let mut visible = ui.data(|data| data.get_temp::<bool>(visible_id).unwrap_or(false));
            row.show(ui, |ui| {
                let (icon, tooltip) = if visible {
                    (Icon::EyeOff, t!("settings.field.hide_secret"))
                } else {
                    (Icon::Eye, t!("settings.field.show_secret"))
                };
                if controls::icon_button(ui, icon, &tooltip).clicked() {
                    visible = !visible;
                }
                ui.add(
                    TextEdit::singleline(&mut value)
                        .id_salt(widget_id)
                        .password(!visible)
                        .desired_width(row::CONTROL_WIDTH - 44.0),
                );
            });
            ui.data_mut(|data| data.insert_temp(visible_id, visible));
            Value::String(value)
        }
        SettingKind::Path { kind } => {
            let mut value = current.as_str().unwrap_or_default().to_owned();
            row.show_below(ui, |ui| {
                // 버튼을 오른쪽 끝에 먼저 놓고, 남은 너비를 입력 칸이 씁니다.
                let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
                let layout = egui::Layout::right_to_left(egui::Align::Center);
                ui.allocate_ui_with_layout(size, layout, |ui| {
                    let browse_label = t!("settings.field.browse");
                    if controls::icon_text_button(ui, Icon::FolderOpen, &browse_label).clicked() {
                        let dialog = rfd::FileDialog::new().set_parent(parent);
                        let picked = match kind {
                            PathKind::File => dialog.pick_file(),
                            PathKind::Directory => dialog.pick_folder(),
                        };
                        if let Some(path) = picked {
                            value = path.to_string_lossy().into_owned();
                        }
                    }
                    ui.add(
                        TextEdit::singleline(&mut value)
                            .id_salt(widget_id)
                            .desired_width(ui.available_width()),
                    );
                });
            });
            Value::String(value)
        }
        SettingKind::Choice { options } => {
            let mut value = current.as_str().unwrap_or_default().to_owned();
            let items: Vec<ComboItem<String>> = options
                .iter()
                .map(|option| {
                    let label = option
                        .label_key
                        .as_deref()
                        .map_or_else(|| option.value.clone(), tr);
                    let mut item = ComboItem::new(option.value.clone(), label);
                    if let Some(detail) = &option.detail {
                        item = item.detail(detail);
                    }
                    item
                })
                .collect();
            row.show(ui, |ui| {
                let fallback = value.clone();
                controls::combo(
                    ui,
                    widget_id,
                    row::CONTROL_WIDTH,
                    &mut value,
                    &items,
                    &fallback,
                );
            });
            Value::String(value)
        }
    };

    if new_value == current {
        return false;
    }
    table.insert(field.key.clone(), new_value);
    true
}
