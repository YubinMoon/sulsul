//! 타일: 테두리가 있는 작은 상자. 카드 안에서 정보를 묶어 보여 줄 때 씁니다.

use egui::{Frame, Margin, Stroke, Ui};

use crate::theme::{Palette, radius};

/// 타일 모서리 반지름
const CORNER: u8 = radius::CONTROL + 2;
/// 타일 안쪽 여백
const MARGIN: Margin = Margin {
    left: 14,
    right: 14,
    top: 10,
    bottom: 10,
};

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
