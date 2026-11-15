//! 단축키 표시와 입력란.
//!
//! - [`keycaps`]: 단축키를 키캡 모양으로 나란히 보여 줍니다.
//! - [`hotkey_field`]: 지금 단축키를 보여 주다가, 누르면 새 단축키를 입력받는 칸입니다.
//!
//! 키 이름은 [`hotkey_key_parts`]로 바꿉니다 (설정 값 `Ctrl+Shift+Space` → "Ctrl", "Shift", "스페이스").

use std::sync::Arc;
use std::time::Duration;

use egui::{
    Color32, CornerRadius, CursorIcon, Galley, Key, Pos2, Rect, Response, Role, Sense, Stroke,
    StrokeKind, Ui, Vec2, ViewportId, WidgetInfo, pos2, vec2,
};
use rust_i18n::t;

use super::text;
use crate::hotkey::{Captured, KeyCapture};
use crate::i18n::hotkey_key_parts;
use crate::icons::{self, Icon};
use crate::theme::{self, Palette, radius, size};

/// 키캡 하나의 높이 (아래쪽 턱 포함)
const CAP_HEIGHT: f32 = 22.0;
/// 키캡 아래쪽 턱의 두께. 키가 솟아 보이게 합니다.
const CAP_LIP: f32 = 2.0;
/// 키캡 안쪽 좌우 여백
const CAP_PADDING_X: f32 = 7.0;
/// 키캡 최소 너비 (한 글자짜리 키도 네모나게)
const CAP_MIN_WIDTH: f32 = 22.0;
/// 키캡과 `+` 사이 간격
const JOIN_GAP: f32 = 4.0;
/// 키캡이 여러 줄로 넘어갔을 때 줄 사이 간격
const LINE_GAP: f32 = 4.0;

/// 입력란 안쪽 여백
const FIELD_PADDING: Vec2 = vec2(10.0, 5.0);
/// 입력란의 내용과 오른쪽 끝 표시(연필 아이콘, 취소 안내) 사이 간격
const TRAILING_GAP: f32 = 8.0;
/// 입력받는 동안 누르고 있는 키 미리보기를 새로 그리는 간격
const PREVIEW_REPAINT: Duration = Duration::from_millis(50);

/// 키캡 색
#[derive(Clone, Copy)]
enum CapTone {
    /// 지금 등록된 단축키
    Plain,
    /// 입력받는 중에 누르고 있는 키
    Accent,
}

/// 키캡 여러 개를 `+`로 이어 놓은 배치. 좌표는 왼쪽 위를 원점으로 합니다.
struct Caps {
    caps: Vec<(Rect, Arc<Galley>)>,
    /// `+` 글자의 왼쪽 위 자리
    joins: Vec<Pos2>,
    plus: Arc<Galley>,
    size: Vec2,
}

impl Caps {
    /// `parts`를 한 줄에 놓되, `max_width`를 넘으면 다음 줄로 넘깁니다.
    fn layout(ui: &Ui, parts: &[String], max_width: f32) -> Self {
        let painter = ui.painter();
        let plus = painter.layout_no_wrap(
            "+".to_owned(),
            theme::regular(size::CAPTION),
            Color32::PLACEHOLDER,
        );
        // `+`는 키캡의 윗면(턱을 뺀 부분) 가운데에 맞춥니다.
        let plus_offset_y = (CAP_HEIGHT - CAP_LIP - plus.size().y) / 2.0;

        let mut caps = Vec::with_capacity(parts.len());
        let mut joins = Vec::new();
        let (mut x, mut y, mut width) = (0.0_f32, 0.0_f32, 0.0_f32);
        for (index, part) in parts.iter().enumerate() {
            let galley = painter.layout_no_wrap(
                part.clone(),
                theme::semibold(size::CAPTION),
                Color32::PLACEHOLDER,
            );
            let cap_width = (galley.size().x + 2.0 * CAP_PADDING_X)
                .ceil()
                .max(CAP_MIN_WIDTH);
            let last = index + 1 == parts.len();
            // 뒤에 붙는 `+`까지 들어가야 같은 줄에 둡니다 (`+`만 다음 줄로 떨어지지 않게).
            let needed = if last {
                cap_width
            } else {
                cap_width + JOIN_GAP + plus.size().x
            };
            if x > 0.0 && x + needed > max_width {
                x = 0.0;
                y += CAP_HEIGHT + LINE_GAP;
            }
            caps.push((
                Rect::from_min_size(pos2(x, y), vec2(cap_width, CAP_HEIGHT)),
                galley,
            ));
            x += needed;
            width = width.max(x);
            if !last {
                joins.push(pos2(x - plus.size().x, y + plus_offset_y));
                x += JOIN_GAP;
            }
        }
        Self {
            caps,
            joins,
            plus,
            size: vec2(width, y + CAP_HEIGHT),
        }
    }

    fn paint(&self, ui: &Ui, origin: Pos2, tone: CapTone) {
        let palette = Palette::of(ui);
        let (face, edge, text_color) = match tone {
            CapTone::Plain => (palette.control, palette.border, palette.text),
            CapTone::Accent => (
                palette.accent_soft,
                theme::mix(palette.accent_soft, palette.accent, 0.45),
                palette.accent_text,
            ),
        };
        let corner = CornerRadius::same(radius::CONTROL - 2);
        let painter = ui.painter();
        for (rect, galley) in &self.caps {
            let rect = rect.translate(origin.to_vec2());
            // 바닥(턱)을 먼저 깔고 그 위에 윗면을 올립니다.
            painter.rect_filled(rect, corner, edge);
            let top = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - CAP_LIP));
            painter.rect(
                top,
                corner,
                face,
                Stroke::new(1.0, edge),
                StrokeKind::Inside,
            );
            painter.galley(
                top.center() - galley.size() / 2.0,
                galley.clone(),
                text_color,
            );
        }
        for join in &self.joins {
            painter.galley(
                origin + join.to_vec2(),
                self.plus.clone(),
                palette.text_faint,
            );
        }
    }
}

/// 단축키(`"RightCtrl"`, `"Ctrl+Shift+Space"`)를 키캡 모양으로 나란히 보여 줍니다.
///
/// 한 줄에 다 들어가지 않으면 키캡 단위로 줄을 바꿉니다. 단축키가 비어 있으면 "지정 안 됨"을 보여 줍니다.
pub fn keycaps(ui: &mut Ui, key: &str) -> Response {
    let parts = hotkey_key_parts(key);
    if parts.is_empty() {
        return ui.label(text::caption(ui, t!("hotkey.unset")));
    }
    let caps = Caps::layout(ui, &parts, ui.max_rect().width());
    let (rect, response) = ui.allocate_exact_size(caps.size, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, parts.join(" + ")));
    if ui.is_rect_visible(rect) {
        caps.paint(ui, rect.min, CapTone::Plain);
    }
    response
}

/// 입력란 왼쪽에 보일 내용
enum Content {
    Caps(Caps, CapTone),
    Text(Arc<Galley>, Color32),
}

impl Content {
    fn size(&self) -> Vec2 {
        match self {
            Self::Caps(caps, _) => caps.size,
            Self::Text(galley, _) => galley.size(),
        }
    }
}

/// 단축키 입력란: 지금 단축키를 키캡으로 보여 주고, 누르면 새 단축키를 입력받습니다.
///
/// 입력받는 동안에는 포인트 색 테두리로 강조하고, 지금 누르고 있는 키를 미리 보여 줍니다.
/// 입력란을 다시 누르거나 밖을 누르면 그만둡니다. Esc는 키보드 후크가 받아
/// [`Captured::Cancelled`]로 알려 주며, 후크가 없을 때는 화면이 받은 Esc로 그만둡니다.
/// 새 키가 정해지면 `key`를 바꾸고 `changed()`인 반응을 돌려줍니다.
///
/// - `id_salt`: 같은 화면의 다른 입력란과 겹치지 않는 값
/// - `label`: 화면 낭독기용 이름 (줄 이름)
pub fn hotkey_field(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    width: f32,
    label: &str,
    key: &mut String,
    capture: &KeyCapture,
) -> Response {
    let palette = Palette::of(ui);
    let enabled = ui.is_enabled();
    // 이 입력란이 시작한 입력받기인지 기억합니다. 창구는 하나라서, 다른 곳에서 시작한 입력받기의
    // 결과를 가로채지 않으려는 것입니다.
    let state_id = ui.make_persistent_id(id_salt);
    let mut capturing = ui.data(|data| data.get_temp::<bool>(state_id).unwrap_or(false));
    let mut changed = false;

    if capturing && !capture.is_active() {
        // 입력받기가 끝났습니다 (새 키, Esc, 또는 앱이 그만두게 함).
        capturing = false;
        if let Some(captured) = capture.take_result()
            && enabled
        {
            changed = apply_captured(key, captured);
        }
    }
    if capturing && !enabled {
        // 입력받는 중에 편집이 잠겼습니다.
        capture.cancel();
        capturing = false;
    }

    // 오른쪽 끝: 평소에는 고칠 수 있다는 표시, 입력받는 중에는 그만두는 방법
    let painter = ui.painter();
    let (trailing, trailing_color) = if capturing {
        let hint = painter.layout_no_wrap(
            t!("hotkey.capture.cancel_hint").into_owned(),
            theme::regular(size::CAPTION),
            Color32::PLACEHOLDER,
        );
        (hint, palette.text_muted)
    } else {
        let icon = painter.layout_no_wrap(
            Icon::Pencil.unicode().to_string(),
            icons::font(size::BODY),
            Color32::PLACEHOLDER,
        );
        (icon, palette.text_faint)
    };
    let content_width = (width - 2.0 * FIELD_PADDING.x - trailing.size().x - TRAILING_GAP).max(0.0);

    let shown_key = if capturing {
        capture.preview().unwrap_or_default()
    } else {
        key.clone()
    };
    let parts = hotkey_key_parts(&shown_key);
    let content = if !parts.is_empty() {
        let tone = if capturing {
            CapTone::Accent
        } else {
            CapTone::Plain
        };
        Content::Caps(Caps::layout(ui, &parts, content_width), tone)
    } else if capturing {
        Content::Text(
            painter.layout(
                t!("hotkey.capture.prompt").into_owned(),
                theme::regular(size::BODY),
                Color32::PLACEHOLDER,
                content_width,
            ),
            palette.accent_text,
        )
    } else {
        Content::Text(
            painter.layout(
                t!("hotkey.unset").into_owned(),
                theme::regular(size::BODY),
                Color32::PLACEHOLDER,
                content_width,
            ),
            palette.text_faint,
        )
    };

    let height = (content.size().y + 2.0 * FIELD_PADDING.y).max(ui.spacing().interact_size.y);
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    // 이번 프레임은 누르기 전 상태로 그립니다.
    let shown_capturing = capturing;

    if response.clicked() {
        if capturing {
            capture.cancel();
        } else {
            capture.start();
        }
        capturing = !capturing;
        ui.ctx().request_repaint();
    } else if capturing
        && (response.clicked_elsewhere() || ui.input(|input| input.key_pressed(Key::Escape)))
    {
        capture.cancel();
        capturing = false;
        ui.ctx().request_repaint();
    }
    if capturing {
        ui.ctx().request_repaint_after(PREVIEW_REPAINT);
        CaptureWatch::mark_seen(ui, capture);
    }
    ui.data_mut(|data| data.insert_temp(state_id, capturing));

    let spoken = if shown_capturing {
        t!("hotkey.capture.prompt").into_owned()
    } else if parts.is_empty() {
        t!("hotkey.unset").into_owned()
    } else {
        parts.join(" + ")
    };
    response
        .widget_info(|| WidgetInfo::labeled(Role::Button, enabled, format!("{label}: {spoken}")));

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let highlighted = enabled && (response.hovered() || response.has_focus());
        let (fill, stroke) = if shown_capturing {
            (
                theme::mix(palette.field, palette.accent_soft, 0.5),
                Stroke::new(1.5, palette.accent),
            )
        } else if highlighted {
            (palette.field, Stroke::new(1.0, palette.border_strong))
        } else {
            (palette.field, Stroke::new(1.0, palette.border))
        };
        painter.rect(
            rect,
            CornerRadius::same(radius::CONTROL),
            fill,
            stroke,
            StrokeKind::Inside,
        );

        let content_pos = pos2(
            rect.left() + FIELD_PADDING.x,
            rect.center().y - content.size().y / 2.0,
        );
        match &content {
            Content::Caps(caps, tone) => caps.paint(ui, content_pos, *tone),
            Content::Text(galley, color) => painter.galley(content_pos, galley.clone(), *color),
        }

        let trailing_pos = pos2(
            rect.right() - FIELD_PADDING.x - trailing.size().x,
            rect.center().y - trailing.size().y / 2.0,
        );
        let trailing_color = if highlighted && !shown_capturing {
            palette.text_muted
        } else {
            trailing_color
        };
        painter.galley(trailing_pos, trailing, trailing_color);

        if response.has_focus() {
            painter.rect_stroke(
                rect.expand(2.0),
                CornerRadius::same(radius::CONTROL + 2),
                Stroke::new(1.5, palette.accent_text),
                StrokeKind::Outside,
            );
        }
    }

    if changed {
        response.mark_changed();
    }
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if shown_capturing {
        response
    } else {
        response.on_hover_text(t!("hotkey.capture.start"))
    }
}

/// 입력받는 중인 입력란이 화면에서 사라지면 입력받기를 대신 그만두는 감시자.
///
/// 입력란은 그려질 때만 코드가 돌아서, 입력받는 중에 다른 화면으로 넘어가면 스스로 그만둘 수 없습니다.
/// 그대로 두면 키 입력이 계속 막히고, 다른 화면에서 누른 키가 새 단축키로 등록됩니다.
/// 그래서 패스가 끝날 때마다 입력란이 그려졌는지 보고, 그려지지 않았으면 입력받기를 그만둡니다.
#[derive(Default)]
struct CaptureWatch {
    /// 입력받는 중인 창구와, 그 입력란이 있는 창
    watching: Option<(KeyCapture, ViewportId)>,
    /// 이번 패스에 그 입력란이 그려졌는지
    seen: bool,
}

impl CaptureWatch {
    /// 입력받는 중인 입력란이 이번 패스에 그려졌다고 알립니다.
    fn mark_seen(ui: &Ui, capture: &KeyCapture) {
        let viewport = ui.ctx().viewport_id();
        // 처음 부를 때 한 번만 등록됩니다.
        let handle = ui.ctx().plugin_or_default::<Self>();
        let mut watch = handle.lock();
        watch.watching = Some((capture.clone(), viewport));
        watch.seen = true;
    }
}

impl egui::Plugin for CaptureWatch {
    fn debug_name(&self) -> &'static str {
        "hotkey_capture_watch"
    }

    fn on_end_pass(&mut self, ui: &mut Ui) {
        let Some((_, viewport)) = &self.watching else {
            return;
        };
        // 다른 창을 그리는 패스에는 입력란이 없는 것이 당연합니다.
        if *viewport != ui.ctx().viewport_id() {
            return;
        }
        if std::mem::take(&mut self.seen) {
            return;
        }
        if let Some((capture, _)) = self.watching.take() {
            capture.cancel();
        }
    }
}

/// 입력받은 결과를 단축키 값에 반영합니다. 값이 바뀌었으면 `true`.
fn apply_captured(key: &mut String, captured: Captured) -> bool {
    match captured {
        Captured::Key(new_key) if !new_key.trim().is_empty() && *key != new_key => {
            *key = new_key;
            true
        }
        // 같은 키를 다시 눌렀거나 Esc로 그만뒀으면 원래 값 그대로입니다.
        Captured::Key(_) | Captured::Cancelled => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_key_replaces_the_hotkey() {
        let mut key = "RightCtrl".to_owned();
        assert!(apply_captured(
            &mut key,
            Captured::Key("Ctrl+Shift+Space".into())
        ));
        assert_eq!(key, "Ctrl+Shift+Space");
    }

    #[test]
    fn cancel_or_same_key_keeps_the_hotkey() {
        let mut key = "RightCtrl".to_owned();
        assert!(!apply_captured(&mut key, Captured::Cancelled));
        assert!(!apply_captured(&mut key, Captured::Key("RightCtrl".into())));
        assert!(!apply_captured(&mut key, Captured::Key("  ".into())));
        assert_eq!(key, "RightCtrl");
    }

    /// 화면 없이 위젯을 한 프레임 그려 봅니다.
    fn run_frame(run: impl FnMut(&mut Ui)) {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), run);
        // 화면에 올리지 않을 글꼴 텍스처는 버립니다 (그냥 떨어뜨리면 egui가 패닉합니다).
        output.textures_delta.clear();
    }

    #[test]
    fn long_combination_wraps_within_the_width() {
        rust_i18n::set_locale("ko");
        run_frame(|ui| {
            let parts = hotkey_key_parts("Ctrl+Alt+Shift+Win+PrintScreen");
            let narrow = Caps::layout(ui, &parts, 150.0);
            assert!(
                narrow.size.x <= 150.0,
                "키캡 줄이 주어진 너비를 넘습니다: {}",
                narrow.size.x
            );
            assert!(
                narrow.size.y > CAP_HEIGHT,
                "긴 조합은 여러 줄로 넘어가야 합니다"
            );
            // `+`는 앞 키캡과 같은 줄에 남습니다.
            for (join, (cap, _)) in narrow.joins.iter().zip(&narrow.caps) {
                assert!(
                    join.y >= cap.top() && join.y <= cap.bottom(),
                    "`+`가 앞 키캡과 다른 줄에 놓였습니다"
                );
            }

            let wide = Caps::layout(ui, &parts, 2000.0);
            assert_eq!(wide.size.y, CAP_HEIGHT, "넓으면 한 줄에 놓여야 합니다");
        });
    }

    /// 입력란만 있는 화면을 한 프레임 그리고, 입력란의 자리와 값이 바뀌었는지를 돌려줍니다.
    fn field_frame(
        ctx: &egui::Context,
        capture: &KeyCapture,
        events: Vec<egui::Event>,
        key: &mut String,
    ) -> (Rect, bool) {
        let input = egui::RawInput {
            events,
            ..egui::RawInput::default()
        };
        let mut shown = (Rect::NOTHING, false);
        let mut output = ctx.run_ui(input, |ui| {
            let response = hotkey_field(ui, "test", 240.0, "녹음 단축키", key, capture);
            shown = (response.rect, response.changed());
        });
        output.textures_delta.clear();
        shown
    }

    /// 한 자리를 눌렀다 떼는 입력
    fn click(pos: Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    #[test]
    fn field_follows_capture_from_start_to_result() {
        rust_i18n::set_locale("ko");
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let capture = KeyCapture::default();
        let mut key = "RightCtrl".to_owned();
        let frame =
            |events: Vec<egui::Event>, key: &mut String| field_frame(&ctx, &capture, events, key);

        let (field_rect, _) = frame(Vec::new(), &mut key);
        assert!(!capture.is_active());

        // 입력란을 누르면 입력받기를 시작합니다.
        let inside = field_rect.center();
        frame(vec![egui::Event::PointerMoved(inside)], &mut key);
        frame(click(inside), &mut key);
        assert!(
            capture.is_active(),
            "입력란을 누르면 입력받기를 시작해야 합니다"
        );

        // 후크가 새 키를 알려 주면 설정 값이 바뀝니다.
        capture.finish(Captured::Key("Ctrl+Shift+Space".into()));
        let (_, changed) = frame(Vec::new(), &mut key);
        assert_eq!(key, "Ctrl+Shift+Space");
        assert!(changed, "값이 바뀐 프레임에는 changed()여야 합니다");

        // 다시 시작한 뒤 입력란 밖을 누르면 그만둡니다.
        frame(click(inside), &mut key);
        assert!(capture.is_active());
        let outside = field_rect.right_bottom() + vec2(40.0, 40.0);
        frame(vec![egui::Event::PointerMoved(outside)], &mut key);
        frame(click(outside), &mut key);
        assert!(
            !capture.is_active(),
            "밖을 누르면 입력받기를 그만둬야 합니다"
        );
        assert_eq!(key, "Ctrl+Shift+Space");

        // Esc로 그만둔 결과는 값을 바꾸지 않습니다.
        frame(click(inside), &mut key);
        assert!(capture.is_active());
        capture.finish(Captured::Cancelled);
        let (_, changed) = frame(Vec::new(), &mut key);
        assert_eq!(key, "Ctrl+Shift+Space");
        assert!(!changed, "그만둔 뒤에는 값이 바뀌지 않아야 합니다");
    }

    /// 입력받는 중에 다른 화면으로 넘어가면(입력란이 더 그려지지 않으면) 입력받기가 남지 않아야 합니다.
    #[test]
    fn capture_stops_when_the_field_is_no_longer_drawn() {
        rust_i18n::set_locale("ko");
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let capture = KeyCapture::default();
        let mut key = "RightCtrl".to_owned();
        // 입력란이 없는 다른 화면을 한 프레임 그립니다.
        let other_page = || {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.label("다른 화면");
            });
            output.textures_delta.clear();
        };

        let (field_rect, _) = field_frame(&ctx, &capture, Vec::new(), &mut key);
        let inside = field_rect.center();
        field_frame(&ctx, &capture, click(inside), &mut key);
        assert!(capture.is_active());

        // 입력란이 계속 그려지는 동안에는 입력받기를 이어 갑니다.
        for _ in 0..3 {
            field_frame(&ctx, &capture, Vec::new(), &mut key);
        }
        assert!(
            capture.is_active(),
            "입력란이 보이는 동안에는 입력받기를 그만두면 안 됩니다"
        );

        other_page();
        other_page();
        assert!(
            !capture.is_active(),
            "입력란이 사라지면 입력받기를 그만둬야 합니다"
        );

        // 돌아오면 평소 모습이고, 다시 눌러 시작할 수 있습니다.
        let (_, changed) = field_frame(&ctx, &capture, Vec::new(), &mut key);
        assert!(!changed);
        assert_eq!(key, "RightCtrl");
        assert!(!capture.is_active());
        field_frame(&ctx, &capture, click(inside), &mut key);
        assert!(
            capture.is_active(),
            "돌아온 뒤 한 번 누르면 다시 입력받기를 시작해야 합니다"
        );
    }
}
