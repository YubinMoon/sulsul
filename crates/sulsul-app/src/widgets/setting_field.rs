//! 설정 스키마([`SettingField`])만 보고 입력 화면을 그리는 범용 렌더러.
//!
//! 엔진·후처리기는 자기 설정 항목을 스키마로 선언하기만 하면 됩니다.
//! 새 엔진을 추가해도 이 파일은 고칠 필요가 없습니다.

use egui::{DragValue, TextEdit, Ui};
use rust_i18n::t;
use sulsul_core::settings::{ChoiceOption, PathKind, SettingField, SettingKind};
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
    // 이 빌드에서 쓸 수 없는 항목은 조작을 잠그고, 값 문제보다 그 이유를 먼저 알립니다.
    let unavailable = field.unavailable_key.as_deref().map(tr);
    let enabled = unavailable.is_none();
    // 저장된 값이 지금은 고를 수 없는 선택지여도 값은 바꾸지 않습니다. 장치를 다시 꽂는 등
    // 상황이 바뀌면 그대로 쓰여야 하기 때문입니다. 대신 같은 자리에 경고로 알립니다.
    let stale_choice = match &field.kind {
        SettingKind::Choice { options } => unavailable_choice_warning(options, &current),
        _ => None,
    };
    let warning = unavailable.or(warning).or(stale_choice);
    let widget_id = (id_salt, field.key.as_str());

    let row = Row::new(&label)
        .help_opt(help.as_deref())
        .warning_opt(warning.as_deref());

    let new_value = match &field.kind {
        SettingKind::Bool => {
            // 잠긴 켜기/끄기는 실제로 적용되는 상태(꺼짐)로 보여 줍니다.
            let mut value = enabled && current.as_bool().unwrap_or_default();
            row.show(ui, |ui| {
                lock(ui, enabled);
                controls::toggle(ui, &mut value, &label)
            });
            Value::Boolean(value)
        }
        SettingKind::Integer { min, max } => {
            let mut value = current.as_integer().unwrap_or(*min);
            row.show(ui, |ui| {
                lock(ui, enabled);
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
                lock(ui, enabled);
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
                lock(ui, enabled);
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
                lock(ui, enabled);
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
                lock(ui, enabled);
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
                    let mut item = ComboItem::new(option.value.clone(), choice_label(option));
                    if let Some(detail) = &option.detail {
                        item = item.detail(detail);
                    }
                    if let Some(reason_key) = &option.unavailable_key {
                        item = item.unavailable(tr(reason_key));
                    }
                    item
                })
                .collect();
            row.show(ui, |ui| {
                lock(ui, enabled);
                let fallback = value.clone();
                // 좁은 창에서는 조작 영역이 기본 너비보다 좁아집니다.
                let width = row::CONTROL_WIDTH.min(ui.available_width());
                controls::combo(ui, widget_id, width, &mut value, &items, &fallback);
            });
            Value::String(value)
        }
    };

    // 잠긴 항목은 보여 주는 값이 저장된 값과 달라도 쓰지 않습니다.
    if !enabled || new_value == current {
        return false;
    }
    table.insert(field.key.clone(), new_value);
    true
}

/// 쓸 수 없는 항목이면 이 영역의 조작 위젯을 잠급니다.
fn lock(ui: &mut Ui, enabled: bool) {
    if !enabled {
        ui.disable();
    }
}

/// 선택지를 화면에 보일 이름. 번역 키가 없으면 값을 그대로 보여 줍니다.
fn choice_label(option: &ChoiceOption) -> String {
    option
        .label_key
        .as_deref()
        .map_or_else(|| option.value.clone(), tr)
}

/// `current`가 지금은 고를 수 없는 선택지면, 그 사실과 이유를 알리는 문구를 돌려줍니다.
fn unavailable_choice_warning(options: &[ChoiceOption], current: &Value) -> Option<String> {
    let current = current.as_str()?;
    let option = options.iter().find(|option| option.value == current)?;
    let reason = tr(option.unavailable_key.as_deref()?);
    Some(
        t!(
            "settings.field.choice_unavailable",
            option = choice_label(option),
            reason = reason
        )
        .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device_options() -> Vec<ChoiceOption> {
        vec![
            ChoiceOption::new("auto"),
            ChoiceOption::new("npu")
                .with_label("engine.sense_voice_local.settings.device_npu")
                .unavailable("engine.sense_voice_local.settings.device_npu_missing"),
            ChoiceOption::new("gpu").with_detail("NVIDIA GeForce RTX 3060"),
        ]
    }

    #[test]
    fn stale_choice_is_reported_with_its_reason() {
        rust_i18n::set_locale("ko");
        let options = device_options();
        let warning = unavailable_choice_warning(&options, &Value::String("npu".into())).unwrap();
        // 어떤 값이 왜 안 되는지 둘 다 들어 있어야 합니다.
        assert!(warning.contains("NPU"), "{warning}");
        assert!(warning.contains("찾지 못했습니다"), "{warning}");
    }

    #[test]
    fn usable_or_unknown_choice_has_no_warning() {
        let options = device_options();
        for value in ["auto", "gpu", "tpu"] {
            let value = Value::String(value.into());
            assert_eq!(unavailable_choice_warning(&options, &value), None);
        }
        assert_eq!(
            unavailable_choice_warning(&options, &Value::Boolean(true)),
            None
        );
    }
}
