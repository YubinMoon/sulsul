//! 카드: 내용을 묶는 둥근 상자.

use egui::{Frame, Margin, Stroke, Ui};

use super::{controls, text};
use crate::icons::{self, Icon};
use crate::theme::{Palette, radius, size};

/// 카드 안쪽 여백
pub const PADDING: i8 = 18;

pub fn frame(ui: &Ui) -> Frame {
    let palette = Palette::of(ui);
    Frame::new()
        .fill(palette.card)
        .stroke(Stroke::new(1.0, palette.card_border))
        .corner_radius(radius::CARD)
        .inner_margin(Margin::same(PADDING))
}

/// 제목 없는 카드
pub fn card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    frame(ui)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// 아이콘과 제목이 있는 카드
pub struct Section<'a> {
    icon: Icon,
    title: &'a str,
    badge: Option<&'a str>,
}

impl<'a> Section<'a> {
    pub fn new(icon: Icon, title: &'a str) -> Self {
        Self {
            icon,
            title,
            badge: None,
        }
    }

    /// 제목 옆에 작은 배지를 붙입니다 (예: "준비 중").
    pub fn badge(mut self, badge: &'a str) -> Self {
        self.badge = Some(badge);
        self
    }

    pub fn show<R>(self, ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(
                    icons::text(self.icon)
                        .size(size::SECTION + 1.0)
                        .color(Palette::of(ui).accent_text),
                );
                ui.label(text::section(self.title));
                if let Some(badge) = self.badge {
                    controls::badge(ui, badge);
                }
            });
            ui.add_space(6.0);
            add_contents(ui)
        })
    }
}
