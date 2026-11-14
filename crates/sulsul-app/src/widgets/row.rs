//! 설정 한 줄: 왼쪽에 이름과 도움말, 오른쪽에 조작 위젯.

use egui::{Align, Layout, Sense, Stroke, TextStyle, Ui, vec2};

use super::{controls, text};
use crate::theme::Palette;

/// 오른쪽 조작 영역의 기본 너비
pub const CONTROL_WIDTH: f32 = 260.0;

pub struct Row<'a> {
    label: &'a str,
    help: Option<&'a str>,
    badge: Option<&'a str>,
    warning: Option<&'a str>,
}

impl<'a> Row<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            help: None,
            badge: None,
            warning: None,
        }
    }

    pub fn help(mut self, help: &'a str) -> Self {
        self.help = Some(help);
        self
    }

    pub fn help_opt(mut self, help: Option<&'a str>) -> Self {
        self.help = help;
        self
    }

    /// 이름 옆에 작은 배지를 붙입니다 (예: "준비 중").
    pub fn badge_opt(mut self, badge: Option<&'a str>) -> Self {
        self.badge = badge;
        self
    }

    /// 도움말 아래에 경고 문구를 보여 줍니다.
    pub fn warning_opt(mut self, warning: Option<&'a str>) -> Self {
        self.warning = warning;
        self
    }

    fn label_ui(&self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 3.0;
        match self.badge {
            Some(badge) => {
                ui.scope(|ui| {
                    // 줄 높이를 글자 높이에 맞춰 배지가 붙어도 줄이 커지지 않게 합니다.
                    ui.spacing_mut().interact_size.y = ui.text_style_height(&TextStyle::Body);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(self.label);
                        controls::badge(ui, badge);
                    });
                });
            }
            None => {
                ui.label(self.label);
            }
        }
        if let Some(help) = self.help {
            ui.label(text::caption(ui, help));
        }
        if let Some(warning) = self.warning {
            ui.label(text::caption(ui, warning).color(Palette::of(ui).warning_text));
        }
    }

    /// 조작 위젯을 오른쪽에 놓습니다.
    ///
    /// 조작 위젯은 오른쪽 끝에서부터 놓이고, 이름의 첫 줄과 세로 가운데가 맞습니다.
    pub fn show<R>(self, ui: &mut Ui, add_control: impl FnOnce(&mut Ui) -> R) -> R {
        let total = ui.available_width();
        let gap = 16.0;
        let control_width = CONTROL_WIDTH.min(total * 0.5);
        let label_width = (total - control_width - gap).max(80.0);
        let control_height = ui.spacing().interact_size.y;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(
                vec2(label_width, 0.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(label_width);
                    let offset = first_line_offset(ui);
                    ui.add_space(offset);
                    self.label_ui(ui);
                    if self.help.is_some() || self.warning.is_some() {
                        // 도움말이 구분선에 붙지 않게 아래에도 조금 띄웁니다.
                        ui.add_space(offset / 2.0);
                    }
                },
            );
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), control_height),
                Layout::right_to_left(Align::Center),
                add_control,
            )
            .inner
        })
        .inner
    }

    /// 조작 위젯을 이름 아래에 전체 너비로 놓습니다 (여러 줄 입력 등).
    pub fn show_below<R>(self, ui: &mut Ui, add_control: impl FnOnce(&mut Ui) -> R) -> R {
        ui.vertical(|ui| {
            ui.scope(|ui| self.label_ui(ui));
            ui.add_space(2.0);
            add_control(ui)
        })
        .inner
    }
}

/// 조작 위젯 높이의 줄에서 본문 첫 줄을 세로 가운데에 맞추려면 위에 둘 여백
pub fn first_line_offset(ui: &Ui) -> f32 {
    let line_height = ui.text_style_height(&TextStyle::Body);
    ((ui.spacing().interact_size.y - line_height) / 2.0).max(0.0)
}

/// 오른쪽 정렬 칸 안에서 위젯 여러 개를 왼쪽에서 오른쪽 순서로 놓습니다.
///
/// 오른쪽 정렬 칸에서는 묶음 위젯(슬라이더와 숫자 등)의 순서가 뒤집히므로,
/// 너비를 알 수 있을 때 이 함수로 감쌉니다.
pub fn left_to_right<R>(ui: &mut Ui, width: f32, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        vec2(width, height),
        Layout::left_to_right(Align::Center),
        add_contents,
    )
    .inner
}

/// 줄 사이 구분선
pub fn separator(ui: &mut Ui) {
    let palette = Palette::of(ui);
    ui.add_space(3.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, palette.card_border),
    );
    ui.add_space(3.0);
}
