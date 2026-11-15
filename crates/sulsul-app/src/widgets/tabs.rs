//! 밑줄 탭: 화면을 큰 갈래로 나누는 탭 줄.
//!
//! 문구 아래에 줄 전체를 가로지르는 얇은 선이 있고, 고른 탭은 문구와 그 아래 선이 포인트 색이 됩니다.
//! 한 화면 안에서 더 작은 갈래를 나눌 때는 [`super::controls::segmented`]를 씁니다.

use egui::{
    Color32, CornerRadius, Rect, Role, Sense, Stroke, StrokeKind, Ui, WidgetInfo, pos2, vec2,
};

use crate::theme::{self, Palette, radius, size};

/// 탭 줄 높이 (문구와 밑줄 포함)
const HEIGHT: f32 = 38.0;
/// 탭 문구 사이 간격
const GAP: f32 = 26.0;
/// 고른 탭의 밑줄 두께
const INDICATOR: f32 = 2.0;
/// 누를 수 있는 영역이 문구보다 좌우로 넓은 정도
const HIT_PADDING: f32 = 6.0;

/// 여러 탭 중 하나를 고릅니다. 값이 바뀌면 `true`를 돌려줍니다.
///
/// 줄은 남은 너비를 모두 차지하고, 탭은 왼쪽부터 놓입니다. 첫 탭의 문구는 줄의 왼쪽 끝에 맞습니다.
pub fn underline_tabs<T: PartialEq + Clone>(
    ui: &mut Ui,
    current: &mut T,
    options: &[(T, String)],
) -> bool {
    let palette = Palette::of(ui);
    let enabled = ui.is_enabled();
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, label)| {
            ui.painter().layout_no_wrap(
                label.clone(),
                theme::semibold(size::SECTION),
                Color32::PLACEHOLDER,
            )
        })
        .collect();
    let (rect, outer) = ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover());

    // 먼저 클릭을 처리하고, 바뀐 값으로 그립니다.
    let mut changed = false;
    let mut tabs = Vec::with_capacity(options.len());
    let mut x = rect.left();
    for (index, ((value, label), galley)) in options.iter().zip(&galleys).enumerate() {
        let text_width = galley.size().x;
        // 문구가 차지하는 가로 범위. 밑줄도 이 범위에 긋습니다.
        let text_span =
            Rect::from_min_max(pos2(x, rect.top()), pos2(x + text_width, rect.bottom()));
        x += text_width + GAP;
        let hit = text_span.expand2(vec2(HIT_PADDING, 0.0));
        let response = ui.interact(hit, outer.id.with(index), Sense::click());
        if response.clicked() && current != value {
            *current = value.clone();
            changed = true;
        }
        let selected = current == value;
        response.widget_info(|| WidgetInfo::selected(Role::Tab, enabled, selected, label));
        tabs.push((text_span, response));
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        // 줄 전체 아래의 기본 선
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, palette.border),
        );
        for (((value, _), galley), (text_span, response)) in options.iter().zip(galleys).zip(&tabs)
        {
            let selected = current == value;
            let hovered = enabled && (response.hovered() || response.has_focus());
            let color = if !enabled {
                palette.text_faint
            } else if selected {
                palette.accent_text
            } else if hovered {
                palette.text
            } else {
                palette.text_muted
            };
            // 문구는 밑줄을 뺀 높이의 가운데에 둡니다.
            let text_area = Rect::from_min_max(
                text_span.min,
                pos2(text_span.right(), text_span.bottom() - INDICATOR),
            );
            let text_rect = Rect::from_center_size(text_area.center(), galley.size());
            painter.galley(text_rect.min, galley, color);

            // 고른 탭의 밑줄은 기본 선 위에 포인트 색으로 덧그립니다.
            let shown = ui.ctx().animate_bool_responsive(response.id, selected);
            if shown > 0.0 {
                let line = Rect::from_min_max(
                    pos2(text_span.left(), text_span.bottom() - INDICATOR),
                    text_span.max,
                );
                painter.rect_filled(
                    line,
                    CornerRadius::same(1),
                    theme::mix(palette.border, palette.accent, shown),
                );
            }
            if response.has_focus() {
                painter.rect_stroke(
                    text_rect.expand2(vec2(HIT_PADDING, 4.0)),
                    CornerRadius::same(radius::CONTROL),
                    Stroke::new(1.5, palette.accent_text),
                    StrokeKind::Outside,
                );
            }
        }
    }
    changed
}
