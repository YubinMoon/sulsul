//! 입력 음량 미터 (-60 ~ 0 dBFS).

use egui::{Align2, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};

use crate::format::{METER_FLOOR_DBFS, dbfs, meter_position};
use crate::theme::{self, Palette, radius, size};

/// 막대가 내려가는 속도 (dB/초). 올라갈 때는 바로 따라갑니다.
const RELEASE_DB_PER_SEC: f32 = 36.0;
/// 최근 최댓값을 붙잡아 두는 시간(초)
const PEAK_HOLD_SECS: f32 = 1.2;
/// 붙잡아 둔 뒤 최댓값 표시가 내려가는 속도 (dB/초)
const PEAK_FALL_DB_PER_SEC: f32 = 24.0;
/// 이 값보다 크면 경고색으로 표시합니다 (너무 큰 입력).
const HOT_DBFS: f32 = -6.0;

/// 미터의 움직임 상태. 값은 모두 dBFS입니다.
#[derive(Debug, Clone, Copy)]
pub struct MeterState {
    /// 부드럽게 움직이는 현재 막대 값
    level: f32,
    /// 최근 최댓값
    peak: f32,
    peak_age: f32,
}

impl Default for MeterState {
    fn default() -> Self {
        Self {
            level: METER_FLOOR_DBFS,
            peak: METER_FLOOR_DBFS,
            peak_age: 0.0,
        }
    }
}

impl MeterState {
    /// 새 입력 음량(선형 0.0 ~ 1.0)과 지난 시간(초)으로 상태를 갱신합니다.
    pub fn update(&mut self, linear_level: f32, dt: f32) {
        let target = dbfs(linear_level).clamp(METER_FLOOR_DBFS, 0.0);
        let dt = dt.clamp(0.0, 0.25);

        self.level = if target >= self.level {
            target
        } else {
            (self.level - RELEASE_DB_PER_SEC * dt).max(target)
        };

        if target >= self.peak {
            self.peak = target;
            self.peak_age = 0.0;
        } else {
            self.peak_age += dt;
            if self.peak_age > PEAK_HOLD_SECS {
                self.peak = (self.peak - PEAK_FALL_DB_PER_SEC * dt).max(self.level);
            }
        }
    }

    /// 화면에 보이는 현재 값 (dBFS)
    pub fn level_dbfs(&self) -> f32 {
        self.level
    }

    /// 최근 최댓값 (dBFS)
    pub fn peak_dbfs(&self) -> f32 {
        self.peak
    }
}

/// 미터를 그립니다. `active`가 아니면 빈 막대만 보여 줍니다.
pub fn level_meter(ui: &mut Ui, state: &MeterState, active: bool) {
    let palette = Palette::of(ui);
    let bar_height = 10.0;
    let label_height = 16.0;
    let width = ui.available_width();
    let (rect, _) =
        ui.allocate_exact_size(vec2(width, bar_height + 4.0 + label_height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let bar = Rect::from_min_size(rect.min, vec2(width, bar_height));
    let rounding = CornerRadius::same(radius::PILL);
    painter.rect_filled(bar, rounding, palette.field);
    painter.rect_stroke(
        bar,
        rounding,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );

    if active {
        let position = meter_position(state.level);
        if position > 0.0 {
            let fill_width = (bar.width() * position).max(bar_height);
            let fill = Rect::from_min_size(bar.min, vec2(fill_width, bar_height));
            let color = if state.level > HOT_DBFS {
                palette.warning
            } else {
                palette.accent
            };
            painter.rect_filled(fill, rounding, color);
        }
        let peak_position = meter_position(state.peak);
        if peak_position > 0.0 {
            let x = (bar.left() + bar.width() * peak_position)
                .clamp(bar.left() + 2.0, bar.right() - 2.0);
            painter.line_segment(
                [pos2(x, bar.top() + 1.0), pos2(x, bar.bottom() - 1.0)],
                Stroke::new(
                    2.0,
                    if state.peak > HOT_DBFS {
                        palette.warning_text
                    } else {
                        palette.accent_text
                    },
                ),
            );
        }
    }

    // 눈금: 12 dB 간격
    let font = theme::regular(size::CAPTION - 1.0);
    let label_y = bar.bottom() + 4.0;
    let steps = (-METER_FLOOR_DBFS / 12.0).round() as i32;
    for step in 0..=steps {
        let value = METER_FLOOR_DBFS + 12.0 * step as f32;
        let x = bar.left() + bar.width() * meter_position(value);
        let anchor = if step == 0 {
            Align2::LEFT_TOP
        } else if step == steps {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        painter.text(
            pos2(x, label_y),
            anchor,
            format!("{value:.0}"),
            font.clone(),
            palette.text_faint,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rises_immediately_and_falls_slowly() {
        let mut meter = MeterState::default();
        meter.update(1.0, 0.016);
        assert_eq!(meter.level_dbfs(), 0.0);

        meter.update(0.0, 0.5);
        assert!(meter.level_dbfs() < 0.0, "내려가기 시작");
        assert!(
            meter.level_dbfs() > METER_FLOOR_DBFS,
            "한 번에 바닥까지 가지 않음"
        );
    }

    #[test]
    fn peak_is_held_then_falls() {
        let mut meter = MeterState::default();
        meter.update(0.5, 0.016);
        let peak = meter.peak_dbfs();

        for _ in 0..10 {
            meter.update(0.0, 0.1);
        }
        assert_eq!(meter.peak_dbfs(), peak, "붙잡아 두는 동안은 그대로");

        for _ in 0..40 {
            meter.update(0.0, 0.1);
        }
        assert!(meter.peak_dbfs() < peak);
        assert!(meter.peak_dbfs() >= meter.level_dbfs());
    }

    #[test]
    fn silence_stays_at_floor() {
        let mut meter = MeterState::default();
        meter.update(0.0, 0.016);
        assert_eq!(meter.level_dbfs(), METER_FLOOR_DBFS);
        assert_eq!(meter.peak_dbfs(), METER_FLOOR_DBFS);
    }
}
