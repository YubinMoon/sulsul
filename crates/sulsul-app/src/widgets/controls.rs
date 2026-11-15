//! 기본 조작 위젯: 스위치, 구간 선택, 강조 버튼, 배지, 콤보.

use egui::{
    Button, Color32, ComboBox, CornerRadius, Response, RichText, Role, Sense, Slider,
    SliderClamping, Stroke, StrokeKind, Ui, WidgetInfo, vec2,
};

use crate::icons::{self, Icon};
use crate::theme::{self, Palette, radius, size};

/// 켜고 끄는 스위치. `label`은 화면 낭독기용 이름입니다.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(SWITCH_SIZE, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let enabled = ui.is_enabled();
    let selected = *on;
    response.widget_info(|| WidgetInfo::selected(Role::Switch, enabled, selected, label));
    paint_switch(ui, rect, &response, selected);
    response
}

/// 여럿 중 하나만 켜 두는 스위치. 꺼진 것을 눌러 켰으면 `true`를 돌려줍니다.
///
/// 켜진 스위치는 눌러도 꺼지지 않습니다(항상 하나는 켜져 있어야 하는 목록용). 다른 스위치를
/// 끄는 일은 부르는 쪽에서 `selected` 값을 바꿔서 합니다. `label`은 화면 낭독기용 이름입니다.
pub fn choice_toggle(ui: &mut Ui, selected: bool, label: &str) -> bool {
    let (rect, mut response) = ui.allocate_exact_size(SWITCH_SIZE, Sense::click());
    let turned_on = response.clicked() && !selected;
    if turned_on {
        response.mark_changed();
    }
    let enabled = ui.is_enabled();
    let shown = selected || turned_on;
    response.widget_info(|| WidgetInfo::selected(Role::RadioButton, enabled, shown, label));
    paint_switch(ui, rect, &response, shown);
    turned_on
}

/// 스위치 크기
const SWITCH_SIZE: egui::Vec2 = vec2(38.0, 22.0);

/// 스위치를 그립니다. `on`은 지금 보여 줄 상태입니다.
fn paint_switch(ui: &Ui, rect: egui::Rect, response: &Response, on: bool) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let palette = Palette::of(ui);
    let t = ui.ctx().animate_bool_responsive(response.id, on);
    let off_color = if response.hovered() {
        palette.border_strong
    } else {
        palette.border
    };
    let on_color = if response.hovered() {
        palette.accent_hover
    } else {
        palette.accent
    };
    let track = theme::mix(off_color, on_color, t);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(radius::PILL), track);
    let knob_radius = rect.height() / 2.0 - 3.0;
    let x = egui::lerp(
        (rect.left() + 3.0 + knob_radius)..=(rect.right() - 3.0 - knob_radius),
        t,
    );
    painter.circle_filled(egui::pos2(x, rect.center().y), knob_radius, Color32::WHITE);
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(2.0),
            CornerRadius::same(radius::PILL),
            Stroke::new(1.5, palette.accent_text),
            StrokeKind::Outside,
        );
    }
}

/// 몇 개 중 하나를 고르는 구간 선택. 값이 바뀌면 `true`를 돌려줍니다.
///
/// 놓이는 방향(왼쪽→오른쪽, 오른쪽→왼쪽)과 상관없이 항목 순서가 유지되도록
/// 전체 크기를 먼저 잡고 직접 그립니다.
pub fn segmented<T: PartialEq + Clone>(
    ui: &mut Ui,
    current: &mut T,
    options: &[(T, String)],
) -> bool {
    const INSET: f32 = 3.0;
    const GAP: f32 = 2.0;
    const PADDING_X: f32 = 12.0;
    const SEGMENT_HEIGHT: f32 = 26.0;

    let palette = Palette::of(ui);
    let enabled = ui.is_enabled();
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, label)| {
            ui.painter().layout_no_wrap(
                label.clone(),
                theme::regular(size::BODY),
                Color32::PLACEHOLDER,
            )
        })
        .collect();
    let widths: Vec<f32> = galleys
        .iter()
        .map(|galley| (galley.size().x + 2.0 * PADDING_X).ceil())
        .collect();
    let gaps = GAP * options.len().saturating_sub(1) as f32;
    let total = vec2(
        widths.iter().sum::<f32>() + gaps + 2.0 * INSET,
        SEGMENT_HEIGHT + 2.0 * INSET,
    );
    let (rect, outer) = ui.allocate_exact_size(total, Sense::hover());

    // 먼저 클릭을 처리하고, 바뀐 값으로 그립니다.
    let mut changed = false;
    let mut segments = Vec::with_capacity(options.len());
    let mut x = rect.left() + INSET;
    for (index, ((value, label), width)) in options.iter().zip(&widths).enumerate() {
        let segment = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + INSET),
            vec2(*width, SEGMENT_HEIGHT),
        );
        x += width + GAP;
        let response = ui.interact(segment, outer.id.with(index), Sense::click());
        if response.clicked() && current != value {
            *current = value.clone();
            changed = true;
        }
        let selected = current == value;
        response.widget_info(|| WidgetInfo::selected(Role::RadioButton, enabled, selected, label));
        segments.push((segment, response));
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::same(radius::CONTROL),
            palette.field,
            Stroke::new(1.0, palette.border),
            StrokeKind::Inside,
        );
        let inner_radius = CornerRadius::same(radius::CONTROL - 3);
        for (((value, _), galley), (segment, response)) in
            options.iter().zip(galleys).zip(&segments)
        {
            let selected = current == value;
            let hovered = enabled && (response.hovered() || response.has_focus());
            if selected {
                painter.rect_filled(*segment, inner_radius, palette.accent_soft);
            } else if hovered {
                painter.rect_filled(*segment, inner_radius, palette.control);
            }
            let color = if !enabled {
                palette.text_faint
            } else if selected {
                palette.accent_text
            } else if hovered {
                palette.text
            } else {
                palette.text_muted
            };
            let position = segment.center() - galley.size() / 2.0;
            painter.galley(position, galley, color);
        }
    }
    changed
}

/// 색을 입힌 버튼 (호버·눌림 색 포함)
fn tinted_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    text: &str,
    fill: Color32,
    hover: Color32,
    text_color: Color32,
) -> Response {
    ui.scope(|ui| {
        let visuals = &mut ui.visuals_mut().widgets;
        visuals.inactive.weak_bg_fill = fill;
        visuals.inactive.bg_stroke = Stroke::new(1.0, fill);
        visuals.hovered.weak_bg_fill = hover;
        visuals.hovered.bg_stroke = Stroke::new(1.0, hover);
        visuals.active.weak_bg_fill = fill;
        visuals.active.bg_stroke = Stroke::new(1.0, fill);
        for state in [
            &mut visuals.inactive,
            &mut visuals.hovered,
            &mut visuals.active,
        ] {
            state.fg_stroke.color = text_color;
        }
        let font = theme::semibold(size::BODY);
        match icon {
            Some(icon) => {
                let response = ui.add(Button::new(icons::with_text_in(icon, text, font)));
                name_without_icon(&response, text);
                response
            }
            None => ui.add(Button::new(RichText::new(text).font(font))),
        }
    })
    .inner
}

/// 화면의 주된 동작 버튼 (포인트 색)
pub fn primary_button(ui: &mut Ui, text: impl AsRef<str>) -> Response {
    let palette = Palette::of(ui);
    tinted_button(
        ui,
        None,
        text.as_ref(),
        palette.accent,
        palette.accent_hover,
        palette.on_accent,
    )
}

/// 아이콘이 붙은 주된 동작 버튼
pub fn primary_icon_button(ui: &mut Ui, icon: Icon, text: impl AsRef<str>) -> Response {
    let palette = Palette::of(ui);
    tinted_button(
        ui,
        Some(icon),
        text.as_ref(),
        palette.accent,
        palette.accent_hover,
        palette.on_accent,
    )
}

/// 되돌리기 어려운 동작 버튼 (빨간색)
pub fn danger_button(ui: &mut Ui, text: impl AsRef<str>) -> Response {
    let palette = Palette::of(ui);
    tinted_button(
        ui,
        None,
        text.as_ref(),
        palette.danger,
        palette.danger_hover,
        palette.on_accent,
    )
}

/// 아이콘이 붙은 보통 버튼
pub fn icon_text_button(ui: &mut Ui, icon: Icon, text: impl AsRef<str>) -> Response {
    let text = text.as_ref();
    let response = ui.button(icons::with_text(ui, icon, text));
    name_without_icon(&response, text);
    response
}

/// 화면 낭독기가 읽는 이름에서 아이콘 문자를 뺍니다.
fn name_without_icon(response: &Response, text: &str) {
    response
        .ctx
        .accesskit_node_builder(response.id, |node| node.set_label(text.to_owned()));
}

/// 아이콘만 있는 작은 버튼. `label`은 툴팁이자 화면 낭독기용 이름입니다.
pub fn icon_button(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    let palette = Palette::of(ui);
    let size = vec2(30.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() || response.has_focus() {
            painter.rect_filled(
                rect,
                CornerRadius::same(radius::CONTROL),
                palette.control_hover,
            );
        }
        let color = if response.hovered() {
            palette.text
        } else {
            palette.text_muted
        };
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon.unicode(),
            icons::font(16.0),
            if enabled { color } else { palette.text_faint },
        );
    }
    response.on_hover_text(label)
}

/// 작은 알약 모양 표시 (예: "준비 중")
pub fn badge(ui: &mut Ui, text: &str) -> Response {
    let palette = Palette::of(ui);
    tone_badge(ui, text, palette.control, palette.text_muted)
}

pub fn tone_badge(ui: &mut Ui, text: &str, fill: Color32, text_color: Color32) -> Response {
    // 줄 높이에 맞춰 늘어나지 않도록 크기를 직접 정해 그립니다.
    let padding = vec2(8.0, 2.0);
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        theme::regular(size::CAPTION - 1.0),
        text_color,
    );
    let (rect, response) = ui.allocate_exact_size(galley.size() + 2.0 * padding, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(radius::PILL), fill);
        painter.galley(rect.min + padding, galley, text_color);
    }
    response
}

/// 정수 슬라이더 (숫자 칸 없이 막대만). 숫자 칸은 옆에 따로 둡니다.
pub fn slider(ui: &mut Ui, value: &mut i64, range: std::ops::RangeInclusive<i64>) -> Response {
    let palette = Palette::of(ui);
    ui.scope(|ui| {
        // 손잡이 크기는 줄 높이를 따라가므로 이 위젯에서만 줄 높이를 줄입니다.
        ui.spacing_mut().interact_size.y = 18.0;
        let visuals = ui.visuals_mut();
        visuals.selection.bg_fill = palette.accent;
        for state in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
        ] {
            state.bg_fill = Color32::WHITE;
            state.fg_stroke = Stroke::new(1.5, palette.accent);
        }
        // 평소 상태의 채움색은 막대(레일)에도 쓰이므로 카드 위에서 보이는 색으로 둡니다.
        visuals.widgets.inactive.bg_fill = palette.border_strong;
        ui.add(
            Slider::new(value, range)
                .clamping(SliderClamping::Always)
                .show_value(false),
        )
    })
    .inner
}

/// 콤보 상자 오른쪽의 펼침 표시
fn combo_chevron(ui: &Ui, rect: egui::Rect, visuals: &egui::style::WidgetVisuals, is_open: bool) {
    let icon = if is_open {
        Icon::ChevronUp
    } else {
        Icon::ChevronDown
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon.unicode(),
        icons::font(15.0),
        visuals.fg_stroke.color.gamma_multiply(0.75),
    );
}

/// 콤보 상자의 한 항목
pub struct ComboItem<T> {
    pub value: T,
    pub label: String,
    /// 번역하지 않는 부가 정보 (흐리게 표시)
    pub detail: Option<String>,
    /// 지금은 고를 수 없는 항목이면 그 이유. 항목을 꺼진 상태로 보여 주고 아래에 이유를 적습니다.
    pub unavailable: Option<String>,
}

impl<T> ComboItem<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            detail: None,
            unavailable: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// 고를 수 없는 항목으로 표시합니다. `reason`은 사용자에게 보일 이유입니다.
    pub fn unavailable(mut self, reason: impl Into<String>) -> Self {
        self.unavailable = Some(reason.into());
        self
    }

    /// 이름과 부가 정보는 그대로 두고 값만 바꿉니다 (예: `String` → `Option<String>`).
    pub fn map_value<U>(self, map: impl FnOnce(T) -> U) -> ComboItem<U> {
        ComboItem {
            value: map(self.value),
            label: self.label,
            detail: self.detail,
            unavailable: self.unavailable,
        }
    }
}

/// 값 하나를 고르는 콤보 상자. 값이 바뀌면 `true`를 돌려줍니다.
///
/// `current`가 목록에 없으면 `fallback_label`을 선택된 글로 보여 줍니다.
///
/// 고를 수 없는 항목([`ComboItem::unavailable`])은 누를 수 없고, 이유가 함께 보입니다.
/// `current`가 그런 항목이어도 값은 바꾸지 않고, 닫힌 상태에서 경고색으로 알립니다.
pub fn combo<T: PartialEq + Clone>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    width: f32,
    current: &mut T,
    items: &[ComboItem<T>],
    fallback_label: &str,
) -> bool {
    let palette = Palette::of(ui);
    let selected = items.iter().find(|item| item.value == *current);
    let selected_label = selected.map_or(fallback_label, |item| item.label.as_str());
    let selected_unavailable = selected.is_some_and(|item| item.unavailable.is_some());
    let selected_text = if selected_unavailable {
        // 고를 수 없는 값이 선택되어 있으면 경고 아이콘과 경고색으로 보여 줍니다.
        let mut job = icons::with_text_in(
            Icon::TriangleAlert,
            selected_label,
            theme::regular(size::BODY),
        );
        for section in &mut job.sections {
            section.format.color = palette.warning_text;
        }
        egui::WidgetText::from(job)
    } else {
        egui::WidgetText::from(selected_label)
    };
    let mut changed = false;
    ComboBox::from_id_salt(id_salt)
        .width(width)
        .selected_text(selected_text)
        .truncate()
        .icon(combo_chevron)
        .show_ui(ui, |ui| {
            // 목록의 처음 너비(= 버튼 너비). 이유 문구는 이 너비 안에서 줄을 바꿉니다.
            let reason_width = ui.available_width();
            for item in items {
                let is_selected = item.value == *current;
                let enabled = item.unavailable.is_none();
                let label_color = match (enabled, is_selected) {
                    (false, _) => palette.text_faint,
                    (true, true) => palette.accent_text,
                    (true, false) => palette.text,
                };
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    &item.label,
                    0.0,
                    egui::TextFormat::simple(theme::regular(size::BODY), label_color),
                );
                if let Some(detail) = &item.detail {
                    job.append(
                        detail,
                        10.0,
                        egui::TextFormat::simple(theme::regular(size::CAPTION), palette.text_faint),
                    );
                }
                let response = ui.add_enabled(enabled, Button::selectable(is_selected, job));
                if let Some(reason) = &item.unavailable {
                    response.on_disabled_hover_text(reason);
                    combo_item_reason(ui, reason, reason_width);
                } else if response.clicked() && !is_selected {
                    *current = item.value.clone();
                    changed = true;
                }
            }
        });
    changed
}

/// 고를 수 없는 콤보 항목 아래에 적는 이유. 항목 글자와 왼쪽을 맞추고, 길면 줄을 바꿉니다.
fn combo_item_reason(ui: &mut Ui, reason: &str, width: f32) {
    let palette = Palette::of(ui);
    let padding = ui.spacing().button_padding.x;
    ui.allocate_ui_with_layout(
        vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_max_width(width);
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: padding as i8,
                    right: padding as i8,
                    top: 0,
                    bottom: 4,
                })
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(reason)
                                .size(size::CAPTION)
                                .color(palette.warning_text),
                        )
                        .wrap(),
                    );
                });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_the_value_keeps_everything_else() {
        let item = ComboItem::new("fr".to_owned(), "프랑스어")
            .detail("fr")
            .unavailable("쓸 수 없음")
            .map_value(Some);
        assert_eq!(item.value.as_deref(), Some("fr"));
        assert_eq!(item.label, "프랑스어");
        assert_eq!(item.detail.as_deref(), Some("fr"));
        assert_eq!(item.unavailable.as_deref(), Some("쓸 수 없음"));
    }
}
