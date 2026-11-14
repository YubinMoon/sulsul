//! 측정값 표시: 작은 이름표와 큰 값.

use egui::{RichText, Ui};

use super::text;
use crate::theme::{self, Palette, size};

pub struct Stat {
    pub label: String,
    pub value: String,
    /// 값 아래 작은 보조 설명
    pub note: Option<String>,
    /// 값을 경고색으로 표시
    pub warn: bool,
}

impl Stat {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            note: None,
            warn: false,
        }
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn warn(mut self, warn: bool) -> Self {
        self.warn = warn;
        self
    }
}

/// 측정값들을 같은 너비의 칸에 나란히 놓습니다.
pub fn stats(ui: &mut Ui, stats: &[Stat]) {
    if stats.is_empty() {
        return;
    }
    ui.columns(stats.len(), |columns| {
        for (column, stat) in columns.iter_mut().zip(stats) {
            let palette = Palette::of(column);
            column.spacing_mut().item_spacing.y = 2.0;
            column.label(text::caption(column, &stat.label));
            column.label(
                RichText::new(&stat.value)
                    .font(theme::semibold(size::SUBTITLE))
                    .color(if stat.warn {
                        palette.warning_text
                    } else {
                        palette.text
                    }),
            );
            if let Some(note) = &stat.note {
                column.label(
                    RichText::new(note)
                        .size(size::CAPTION)
                        .color(palette.text_faint),
                );
            }
        }
    });
}
