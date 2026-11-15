//! 알림 영역(트레이) 아이콘과 메뉴("술술 열기", "끝내기").
//!
//! 아이콘은 그림 파일 없이 코드로 그립니다. 받아쓰기 상태에 따라 색과 모양이 바뀌고(녹음 중: 위험 색,
//! 받아쓰는 중: 경고 색과 점 세 개), 툴팁에도 상태가 보입니다.
//! Windows는 `tray-icon`으로 만들고 화면 스레드(winit 메시지 루프)에서 돌립니다. 다른 OS는 아직 지원하지 않습니다.

use std::sync::mpsc::{self, Receiver};

use egui::Color32;
use rust_i18n::t;

use crate::session::DictationState;
use crate::theme::Palette;

/// 트레이에서 고른 일
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    /// 창을 엽니다 (메뉴 또는 아이콘 클릭).
    Open,
    /// 앱을 끝냅니다.
    Quit,
}

#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    #[error("the tray icon is not supported on this platform")]
    /// 지원하지 않는 OS에서만 만듭니다.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
    #[error("could not create the tray icon: {0}")]
    Create(String),
}

pub struct Tray {
    commands: Receiver<TrayCommand>,
    /// 지금 보이는 아이콘의 상태
    shown: DictationState,
    /// 메뉴·툴팁을 만든 화면 언어
    locale: String,
    #[cfg(windows)]
    native: windows::NativeTray,
}

impl Tray {
    /// 트레이 아이콘을 만듭니다. 메뉴나 아이콘을 누르면 `wake`가 불리고(화면을 다시 그리게 함),
    /// [`Tray::commands`]로 받습니다. 화면 스레드에서 불러야 합니다.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Result<Self, TrayError> {
        let (tx, commands) = mpsc::channel();
        let state = DictationState::Idle;
        #[cfg(windows)]
        {
            let native = windows::NativeTray::new(state, tx, wake)?;
            Ok(Self {
                commands,
                shown: state,
                locale: rust_i18n::locale().to_string(),
                native,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (tx, wake, commands, state);
            Err(TrayError::Unsupported)
        }
    }

    /// 쌓인 명령을 가져갑니다.
    pub fn commands(&self) -> Vec<TrayCommand> {
        self.commands.try_iter().collect()
    }

    /// 받아쓰기 상태와 화면 언어를 아이콘·툴팁·메뉴에 반영합니다. 바뀐 것이 없으면 아무것도 하지 않습니다.
    pub fn update(&mut self, state: DictationState) {
        let locale = rust_i18n::locale().to_string();
        if self.locale != locale {
            self.locale = locale;
            #[cfg(windows)]
            self.native.refresh_texts(state);
        } else if self.shown != state {
            #[cfg(windows)]
            self.native.set_state(state);
        }
        self.shown = state;
    }
}

/// 상태별 툴팁
fn tooltip(state: DictationState) -> String {
    match state {
        DictationState::Recording => t!("tray.tooltip_recording"),
        DictationState::Transcribing => t!("tray.tooltip_transcribing"),
        DictationState::Idle | DictationState::WaitingPaste => t!("tray.tooltip"),
    }
    .into_owned()
}

/// 아이콘 RGBA (가로·세로 `size` 픽셀)
fn icon_rgba(size: u32, state: DictationState) -> Vec<u8> {
    // 트레이는 작업 표시줄 위에 있고 작업 표시줄은 대개 어두우므로 다크 테마 색을 씁니다.
    let palette = Palette::for_dark_mode(true);
    let (fill, glyph) = match state {
        DictationState::Recording => (palette.danger, Glyph::Bars),
        DictationState::Transcribing => (palette.warning, Glyph::Dots),
        DictationState::Idle | DictationState::WaitingPaste => (palette.accent, Glyph::Bars),
    };
    draw_icon(size, fill, palette.on_accent, glyph)
}

#[derive(Clone, Copy)]
enum Glyph {
    /// 소리 막대 네 개 (앱 로고와 같은 모양)
    Bars,
    /// 점 세 개 (처리 중)
    Dots,
}

/// 둥근 사각형 바탕에 `glyph`를 그립니다. 가장자리는 거리 함수로 부드럽게 칠합니다.
fn draw_icon(size: u32, fill: Color32, ink: Color32, glyph: Glyph) -> Vec<u8> {
    let s = size as f32;
    let center = s / 2.0;
    let half = s / 2.0 - s * 0.03;
    let radius = s * 0.24;

    // 막대: (가로 위치 비율, 높이 비율)
    const BARS: [(f32, f32); 4] = [(-0.27, 0.28), (-0.09, 0.56), (0.09, 0.40), (0.27, 0.22)];
    let bar_radius = (s * 0.055).max(0.75);
    let dot_radius = (s * 0.075).max(0.9);

    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5 - center;
            let py = y as f32 + 0.5 - center;
            let base = coverage(rounded_rect_distance(px, py, half, radius));
            let mark = match glyph {
                Glyph::Bars => BARS
                    .iter()
                    .map(|(offset, height)| {
                        let half_height = (s * height / 2.0 - bar_radius).max(0.0);
                        capsule_distance(px - offset * s, py, half_height, bar_radius)
                    })
                    .fold(f32::MAX, f32::min),
                Glyph::Dots => [-0.22, 0.0, 0.22]
                    .iter()
                    .map(|offset| (px - offset * s).hypot(py) - dot_radius)
                    .fold(f32::MAX, f32::min),
            };
            let ink_alpha = coverage(mark) * base;
            let mix = |a: u8, b: u8| -> f32 {
                f32::from(a) * (1.0 - ink_alpha) + f32::from(b) * ink_alpha
            };
            rgba.extend_from_slice(&[
                mix(fill.r(), ink.r()).round() as u8,
                mix(fill.g(), ink.g()).round() as u8,
                mix(fill.b(), ink.b()).round() as u8,
                (base * 255.0).round() as u8,
            ]);
        }
    }
    rgba
}

/// 거리(픽셀, 안쪽이 음수)에서 칠할 비율
fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

/// 가운데가 원점이고 반너비가 `half`, 모서리 반지름이 `radius`인 정사각형까지의 거리
fn rounded_rect_distance(x: f32, y: f32, half: f32, radius: f32) -> f32 {
    let qx = x.abs() - (half - radius);
    let qy = y.abs() - (half - radius);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

/// 세로 선분(가운데 원점, 반길이 `half_height`)을 반지름 `radius`로 두껍게 한 모양까지의 거리
fn capsule_distance(x: f32, y: f32, half_height: f32, radius: f32) -> f32 {
    let dy = (y.abs() - half_height).max(0.0);
    x.hypot(dy) - radius
}

#[cfg(windows)]
mod windows {
    use std::sync::mpsc::Sender;

    use rust_i18n::t;
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};

    use super::{TrayCommand, TrayError, icon_rgba, tooltip};
    use crate::session::DictationState;

    pub struct NativeTray {
        icon: TrayIcon,
        open: MenuItem,
        quit: MenuItem,
        size: u32,
    }

    impl NativeTray {
        pub fn new(
            state: DictationState,
            commands: Sender<TrayCommand>,
            wake: impl Fn() + Send + Sync + 'static,
        ) -> Result<Self, TrayError> {
            // SAFETY: 인자만 받는 함수입니다. 알림 영역의 작은 아이콘 크기(화면 배율 반영)입니다.
            let size = u32::try_from(unsafe { GetSystemMetrics(SM_CXSMICON) })
                .ok()
                .filter(|size| (16..=256).contains(size))
                .unwrap_or(32);

            let open = MenuItem::new(t!("tray.open"), true, None);
            let quit = MenuItem::new(t!("tray.quit"), true, None);
            let menu = Menu::new();
            menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit])
                .map_err(|error| TrayError::Create(error.to_string()))?;

            let icon = TrayIconBuilder::new()
                .with_icon(icon(size, state)?)
                .with_tooltip(tooltip(state))
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()
                .map_err(|error| TrayError::Create(error.to_string()))?;

            let wake = std::sync::Arc::new(wake);
            let open_id: MenuId = open.id().clone();
            let quit_id: MenuId = quit.id().clone();
            let menu_commands = commands.clone();
            let menu_wake = std::sync::Arc::clone(&wake);
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let command = if event.id == open_id {
                    TrayCommand::Open
                } else if event.id == quit_id {
                    TrayCommand::Quit
                } else {
                    return;
                };
                let _ = menu_commands.send(command);
                menu_wake();
            }));
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                let open = matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }
                );
                if open {
                    let _ = commands.send(TrayCommand::Open);
                    wake();
                }
            }));

            Ok(Self {
                icon,
                open,
                quit,
                size,
            })
        }

        pub fn set_state(&self, state: DictationState) {
            match icon(self.size, state) {
                Ok(icon) => {
                    if let Err(error) = self.icon.set_icon(Some(icon)) {
                        tracing::debug!("could not update the tray icon: {error}");
                    }
                }
                Err(error) => tracing::debug!("could not draw the tray icon: {error}"),
            }
            if let Err(error) = self.icon.set_tooltip(Some(tooltip(state))) {
                tracing::debug!("could not update the tray tooltip: {error}");
            }
        }

        /// 화면 언어가 바뀌었을 때 메뉴와 툴팁 문구를 다시 씁니다.
        pub fn refresh_texts(&self, state: DictationState) {
            self.open.set_text(t!("tray.open"));
            self.quit.set_text(t!("tray.quit"));
            self.set_state(state);
        }
    }

    fn icon(size: u32, state: DictationState) -> Result<Icon, TrayError> {
        Icon::from_rgba(icon_rgba(size, state), size, size)
            .map_err(|error| TrayError::Create(error.to_string()))
    }

    impl Drop for NativeTray {
        fn drop(&mut self) {
            // 앱이 끝난 뒤 남은 처리기가 닫힌 채널에 보내지 않게 풀어 둡니다.
            MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
            TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(rgba: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn icon_has_transparent_corners_and_opaque_body() {
        for size in [16, 20, 24, 32] {
            let rgba = icon_rgba(size, DictationState::Idle);
            assert_eq!(rgba.len(), (size * size * 4) as usize);
            assert_eq!(pixel(&rgba, size, 0, 0)[3], 0, "{size}: 모서리는 투명");
            assert_eq!(
                pixel(&rgba, size, size / 2, 1)[3],
                255,
                "{size}: 바탕은 불투명"
            );
        }
    }

    #[test]
    fn icon_color_follows_the_state() {
        let palette = Palette::for_dark_mode(true);
        let edge = |state| {
            let rgba = icon_rgba(32, state);
            let [r, g, b, _] = pixel(&rgba, 32, 16, 2);
            Color32::from_rgb(r, g, b)
        };
        assert_eq!(edge(DictationState::Idle), palette.accent);
        assert_eq!(edge(DictationState::Recording), palette.danger);
        assert_eq!(edge(DictationState::Transcribing), palette.warning);
        assert_ne!(
            icon_rgba(32, DictationState::Recording),
            icon_rgba(32, DictationState::Transcribing),
            "녹음 중과 받아쓰는 중은 모양도 다름"
        );
    }

    #[test]
    fn tooltip_shows_the_state() {
        rust_i18n::set_locale("ko");
        assert_ne!(
            tooltip(DictationState::Recording),
            tooltip(DictationState::Idle)
        );
        assert_ne!(
            tooltip(DictationState::Transcribing),
            tooltip(DictationState::Idle)
        );
    }
}
