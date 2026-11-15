//! 설정 한 줄: 왼쪽에 이름과 도움말, 오른쪽에 조작 위젯.

use egui::{Align, Layout, Sense, Stroke, TextStyle, Ui, vec2};

use super::text;
use crate::theme::Palette;

/// 오른쪽 조작 영역의 기본 너비
pub const CONTROL_WIDTH: f32 = 260.0;

pub struct Row<'a> {
    label: &'a str,
    help: Option<&'a str>,
    warning: Option<&'a str>,
}

impl<'a> Row<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            help: None,
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

    /// 도움말 아래에 경고 문구를 보여 줍니다.
    pub fn warning_opt(mut self, warning: Option<&'a str>) -> Self {
        self.warning = warning;
        self
    }

    fn label_ui(&self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.label(self.label);
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

/// 왼쪽에 내용, 오른쪽 끝에 버튼 묶음을 놓는 줄.
///
/// 버튼을 먼저 오른쪽 끝에 놓고 남은 너비만 내용에 주므로, 내용이 길어도 버튼과 겹치지
/// 않고 줄을 바꿉니다. 줄 높이는 내용과 버튼 중 큰 쪽에 맞고, 내용의 첫 줄은 버튼과
/// 세로 가운데가 맞습니다. `beside`가 `false`면 버튼을 내용 아래 줄에 오른쪽 정렬로 놓습니다.
///
/// `add_actions`는 오른쪽에서 왼쪽 순서로 놓입니다(먼저 더한 것이 가장 오른쪽).
///
/// 남은 영역 전체를 차지하는 `ui.with_layout(right_to_left(Align::Center))`는 쓰지 않습니다.
/// 영역의 높이가 내용보다 크면 버튼이 그 가운데로 내려가 빈 공간이 생기기 때문입니다.
pub fn with_actions<R>(
    ui: &mut Ui,
    beside: bool,
    add_actions: impl FnOnce(&mut Ui),
    add_content: impl FnOnce(&mut Ui) -> R,
) -> R {
    const CONTENT_GAP: f32 = 12.0;
    const ACTION_GAP: f32 = 6.0;
    let width = ui.available_width();
    if beside {
        ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::right_to_left(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = CONTENT_GAP;
            // 오른쪽에서 왼쪽으로 놓는 칸 안이므로 버튼 높이의 줄에 오른쪽 끝부터 놓입니다.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ACTION_GAP;
                add_actions(ui);
            });
            let remaining = ui.available_width().max(0.0);
            ui.allocate_ui_with_layout(vec2(remaining, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(remaining);
                ui.add_space(first_line_offset(ui));
                add_content(ui)
            })
            .inner
        })
        .inner
    } else {
        ui.vertical(|ui| {
            let result = add_content(ui);
            ui.allocate_ui_with_layout(
                vec2(width, ui.spacing().interact_size.y),
                Layout::right_to_left(Align::Center).with_main_wrap(true),
                |ui| {
                    ui.spacing_mut().item_spacing.x = ACTION_GAP;
                    add_actions(ui);
                },
            );
            result
        })
        .inner
    }
}

/// 이름과 배지를 한 줄에 놓습니다. 너비가 모자라면 다음 줄로 넘깁니다.
pub fn title_line<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        // 줄 높이를 글자 높이에 맞춰 배지가 붙어도 줄이 커지지 않게 합니다.
        ui.spacing_mut().interact_size.y = ui.text_style_height(&TextStyle::Body);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            add_contents(ui)
        })
        .inner
    })
    .inner
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
