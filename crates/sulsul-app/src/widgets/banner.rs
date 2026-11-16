//! 배너: 화면 위쪽에 띄우는 알림 상자.

use egui::{Align, Color32, Frame, Layout, Margin, RichText, Stroke, Ui};

use super::text;
use crate::icons::{self, Icon};
use crate::theme::{self, Palette, radius, size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Success,
    Warning,
    Error,
}

struct ToneColors {
    fill: Color32,
    border: Color32,
    icon: Color32,
}

impl Tone {
    fn icon(self) -> Icon {
        match self {
            Self::Info => Icon::Info,
            Self::Success => Icon::CircleCheck,
            Self::Warning => Icon::TriangleAlert,
            Self::Error => Icon::CircleAlert,
        }
    }

    fn colors(self, palette: &Palette) -> ToneColors {
        let (fill, strong, icon) = match self {
            Self::Info => (palette.accent_soft, palette.accent, palette.accent_text),
            Self::Success => (palette.success_soft, palette.success, palette.success_text),
            Self::Warning => (palette.warning_soft, palette.warning, palette.warning_text),
            Self::Error => (palette.danger_soft, palette.danger, palette.danger_text),
        };
        ToneColors {
            fill,
            border: theme::mix(fill, strong, 0.45),
            icon,
        }
    }
}

/// 배너 아래쪽에 버튼 등을 그리는 함수
type Actions<'a> = Box<dyn FnOnce(&mut Ui) + 'a>;

pub struct Banner<'a> {
    tone: Tone,
    subtle: bool,
    title: Option<&'a str>,
    lines: Vec<String>,
}

impl<'a> Banner<'a> {
    pub fn new(tone: Tone) -> Self {
        Self {
            tone,
            subtle: false,
            title: None,
            lines: Vec::new(),
        }
    }

    /// 준비 상태는 중립적인 카드로 알리고, 상태 색은 아이콘에만 씁니다.
    pub fn subtle(mut self) -> Self {
        self.subtle = true;
        self
    }

    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    /// 본문 한 단락을 더합니다.
    pub fn line(mut self, line: impl Into<String>) -> Self {
        self.lines.push(line.into());
        self
    }

    pub fn lines(mut self, lines: impl IntoIterator<Item = String>) -> Self {
        self.lines.extend(lines);
        self
    }

    pub fn show(self, ui: &mut Ui) {
        self.render(ui, None);
    }

    /// 본문 아래에 버튼 등을 함께 놓습니다.
    pub fn show_with(self, ui: &mut Ui, add_actions: impl FnOnce(&mut Ui)) {
        self.render(ui, Some(Box::new(add_actions)));
    }

    fn render(self, ui: &mut Ui, add_actions: Option<Actions<'_>>) {
        let palette = Palette::of(ui);
        let mut colors = self.tone.colors(&palette);
        if self.subtle {
            colors.fill = palette.card;
            colors.border = palette.card_border;
        }
        Frame::new()
            .fill(colors.fill)
            .stroke(Stroke::new(1.0, colors.border))
            .corner_radius(radius::CARD)
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.label(
                        icons::text(self.tone.icon())
                            .size(size::SUBTITLE)
                            .color(colors.icon),
                    );
                    ui.vertical(|ui| {
                        ui.set_max_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 4.0;
                        if let Some(title) = self.title {
                            ui.add(
                                egui::Label::new(text::strong(title).color(palette.text)).wrap(),
                            );
                        }
                        for line in &self.lines {
                            let color = if self.subtle && self.title.is_some() {
                                palette.text_muted
                            } else {
                                palette.text
                            };
                            ui.add(egui::Label::new(RichText::new(line).color(color)).wrap());
                        }
                        if let Some(add_actions) = add_actions {
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(add_actions);
                        }
                    });
                });
            });
    }
}
