//! 확인 대화상자 (앱 안에 띄우는 모달).

use egui::{Align, Frame, Id, Layout, Margin, Modal, Stroke, Ui};

use super::{controls, text};
use crate::theme::{Palette, radius};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Confirm,
    Cancel,
}

pub struct Confirm<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub confirm_label: &'a str,
    pub cancel_label: &'a str,
    /// 확인 버튼을 빨간색으로 표시 (되돌리기 어려운 동작)
    pub danger: bool,
}

impl Confirm<'_> {
    /// 대화상자를 그립니다. 사용자가 답했으면 그 답을 돌려줍니다.
    pub fn show(self, ui: &Ui) -> Option<Answer> {
        let palette = Palette::of(ui);
        let frame = Frame::new()
            .fill(palette.card)
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius::CARD)
            .shadow(ui.visuals().window_shadow)
            .inner_margin(Margin::same(22));

        let response = Modal::new(Id::unique(self.id))
            .frame(frame)
            .show(ui.ctx(), |ui| {
                ui.set_width(380.0);
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.label(text::section(self.title));
                ui.label(text::muted(ui, self.body));
                ui.add_space(6.0);
                let mut answer = None;
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let confirm = if self.danger {
                        controls::danger_button(ui, self.confirm_label)
                    } else {
                        controls::primary_button(ui, self.confirm_label)
                    };
                    if confirm.clicked() {
                        answer = Some(Answer::Confirm);
                    }
                    if ui.button(self.cancel_label).clicked() {
                        answer = Some(Answer::Cancel);
                    }
                });
                answer
            });

        if response.inner.is_some() {
            response.inner
        } else if response.should_close() {
            Some(Answer::Cancel)
        } else {
            None
        }
    }
}
