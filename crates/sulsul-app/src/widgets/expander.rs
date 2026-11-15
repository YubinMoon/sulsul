//! 펼침 카드: 머리 줄을 누르면 아래에 세부 내용이 펼쳐지는 카드.
//!
//! 목록의 항목마다 카드 하나를 두고, 항목의 세부 설정을 그 자리에서 보거나 고칠 때 씁니다.

use egui::{CursorIcon, Frame, Margin, Response, Role, Sense, Stroke, Ui, UiBuilder, WidgetInfo};
use rust_i18n::t;

use super::{controls, row};
use crate::icons::Icon;
use crate::theme::{self, Palette, radius};

/// 카드 안쪽 여백
const MARGIN: Margin = Margin {
    left: 16,
    right: 16,
    top: 12,
    bottom: 12,
};
/// 머리 줄과 펼친 내용 사이 간격 (구분선 위아래)
const BODY_GAP: f32 = 8.0;

pub struct Expander<'a> {
    label: &'a str,
    open: bool,
    selected: bool,
}

/// [`Expander::show`]의 결과
pub struct ExpanderResponse {
    /// 머리 줄의 빈 곳이나 펼치기 버튼을 눌렀는지. 부르는 쪽에서 펼침 상태를 바꿉니다.
    pub toggled: bool,
    /// 카드 전체의 반응 (화면을 이 카드로 옮길 때 등에 씁니다)
    pub response: Response,
}

impl<'a> Expander<'a> {
    /// `label`은 화면 낭독기용 이름, `open`은 지금 펼쳐져 있는지입니다.
    pub fn new(label: &'a str, open: bool) -> Self {
        Self {
            label,
            open,
            selected: false,
        }
    }

    /// 목록에서 고른 항목이면 포인트 색 테두리로 표시합니다.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// 카드를 그립니다.
    ///
    /// - `add_title`: 머리 줄 왼쪽 내용 (이름, 배지, 짧은 설명). 남은 너비 안에서 줄을 바꿉니다.
    /// - `add_trailing`: 머리 줄 오른쪽 끝에 놓을 조작 위젯 (스위치 등). 펼치기 버튼은 그 왼쪽에 붙습니다.
    /// - `add_body`: 펼쳤을 때만 그리는 세부 내용.
    pub fn show(
        self,
        ui: &mut Ui,
        id_salt: impl egui::AsIdSalt,
        add_title: impl FnOnce(&mut Ui),
        add_trailing: impl FnOnce(&mut Ui),
        add_body: impl FnOnce(&mut Ui),
    ) -> ExpanderResponse {
        let palette = Palette::of(ui);
        let enabled = ui.is_enabled();
        let Self {
            label,
            open,
            selected,
        } = self;
        let builder = UiBuilder::new().id_salt(id_salt).sense(Sense::hover());
        let scoped = ui.scope_builder(builder, |ui| {
            // 앞 프레임에서 그린 영역으로 판단한 반응입니다.
            let hovered = enabled && ui.response().hovered();
            let (fill, stroke) = if selected {
                (
                    theme::mix(palette.card, palette.accent_soft, 0.35),
                    palette.accent,
                )
            } else if hovered {
                (palette.card, palette.border_strong)
            } else {
                (palette.card, palette.card_border)
            };
            Frame::new()
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke))
                .corner_radius(radius::CARD)
                .inner_margin(MARGIN)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut toggled = false;
                    // 머리 줄: 빈 곳을 누르면 펼치거나 접습니다. 안의 스위치·버튼은 그것만 눌립니다.
                    let header = ui.scope_builder(
                        UiBuilder::new().id_salt("header").sense(Sense::click()),
                        |ui| {
                            row::with_actions(
                                ui,
                                true,
                                |ui| {
                                    add_trailing(ui);
                                    let (icon, tooltip) = if open {
                                        (Icon::ChevronUp, t!("common.collapse"))
                                    } else {
                                        (Icon::ChevronDown, t!("common.expand"))
                                    };
                                    let button = controls::icon_button(ui, icon, &tooltip);
                                    button.ctx.accesskit_node_builder(button.id, |node| {
                                        node.set_expanded(open);
                                    });
                                    toggled |= button.clicked();
                                },
                                add_title,
                            );
                        },
                    );
                    let header = header.response.on_hover_cursor(CursorIcon::PointingHand);
                    header.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
                    toggled |= header.clicked();

                    if open {
                        ui.add_space(BODY_GAP - 3.0);
                        row::separator(ui);
                        ui.add_space(BODY_GAP - 3.0);
                        add_body(ui);
                    }
                    toggled
                })
                .inner
        });
        ExpanderResponse {
            toggled: scoped.inner,
            response: scoped.response,
        }
    }
}
