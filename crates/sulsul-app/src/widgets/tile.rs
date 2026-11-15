//! 타일: 테두리가 있는 작은 상자. 목록에서 하나를 고르거나 정보를 묶어 보여 줄 때 씁니다.

use egui::{
    CursorIcon, Frame, InnerResponse, Margin, Role, Sense, Stroke, Ui, UiBuilder, WidgetInfo,
};

use crate::theme::{self, Palette, radius};

/// 타일 모서리 반지름
const CORNER: u8 = radius::CONTROL + 2;
/// 타일 안쪽 여백
const MARGIN: Margin = Margin {
    left: 14,
    right: 14,
    top: 10,
    bottom: 10,
};

/// 통째로 누를 수 있는 타일. 여러 개 중 하나를 고르는 데 씁니다.
///
/// 고른 타일(`selected`)은 포인트 색 테두리와 옅은 배경으로 표시합니다. 타일 안의 버튼을
/// 누르면 그 버튼만 눌리고 타일은 눌리지 않습니다. `label`은 화면 낭독기용 이름입니다.
pub fn selectable<R>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    selected: bool,
    label: &str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let palette = Palette::of(ui);
    let enabled = ui.is_enabled();
    let builder = UiBuilder::new().id_salt(id_salt).sense(Sense::click());
    let scoped = ui.scope_builder(builder, |ui| {
        // 앞 프레임에서 그린 영역으로 판단한 반응입니다.
        let response = ui.response();
        let hovered = enabled && (response.hovered() || response.has_focus());
        let (fill, stroke) = if selected {
            (
                theme::mix(palette.card, palette.accent_soft, 0.6),
                palette.accent,
            )
        } else if hovered {
            (
                theme::mix(palette.card, palette.control, 0.6),
                palette.border_strong,
            )
        } else {
            (palette.card, palette.border)
        };
        Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CORNER)
            .inner_margin(MARGIN)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                add_contents(ui)
            })
            .inner
    });
    let response = scoped.response.on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::selected(Role::RadioButton, enabled, selected, label));
    InnerResponse::new(scoped.inner, response)
}

/// 누를 수 없는 정보 타일. 카드보다 한 단계 어두운 배경에 내용을 묶습니다.
pub fn panel<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = Palette::of(ui);
    Frame::new()
        .fill(palette.field)
        .stroke(Stroke::new(1.0, palette.card_border))
        .corner_radius(CORNER)
        .inner_margin(MARGIN)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}
