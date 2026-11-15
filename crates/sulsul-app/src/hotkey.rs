//! 전역 단축키.
//!
//! - [`HotkeySpec`]: 설정 파일의 단축키 값(`"RightCtrl"`, `"Ctrl+Shift+Space"`)을 읽고 씁니다.
//! - [`GlobalHotkey`]: 어느 앱을 쓰고 있든 단축키를 받아 [`HotkeyEvent`]로 알립니다.
//!   Windows는 저수준 키보드 후크로 구현하고, 다른 OS는 아직 지원하지 않습니다.
//! - [`KeyCapture`]: 설정 화면이 새 단축키를 입력받는 창구입니다. 입력받는 동안에는 녹음 단축키 대신
//!   다음에 누른 키(조합)를 받고, 그 키는 다른 앱으로 전달되지 않습니다.
//!
//! 키를 읽고 판단하는 규칙은 OS와 무관한 [`machine`]에 있고, OS별 코드는 키를 넘겨주고
//! 판단대로 키를 막거나 통과시키기만 합니다.

mod keys;
mod machine;
mod spec;
#[cfg(windows)]
mod windows;

use std::sync::{Arc, Mutex, MutexGuard};

use sulsul_core::config::HotkeyMode;

pub use keys::Key;
pub use machine::HotkeyEvent;
pub use spec::HotkeySpec;

/// 단축키 입력받기 결과
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// 설정 파일 형식의 키 (`"RightCtrl"`, `"F13"`, `"Ctrl+Shift+Space"`)
    Key(String),
    /// Esc를 눌러 그만둠
    Cancelled,
}

/// 설정 화면에서 새 단축키를 입력받는 창구. 복제해도 같은 상태를 봅니다.
///
/// 화면은 [`start`](Self::start)로 입력받기를 시작하고 매 프레임 [`take_result`](Self::take_result)를
/// 확인합니다. 입력받는 동안에는 녹음 단축키가 동작하지 않고, 누른 키는 다른 곳으로 전달되지 않습니다.
#[derive(Debug, Clone, Default)]
pub struct KeyCapture {
    state: Arc<Mutex<CaptureState>>,
}

#[derive(Debug, Default)]
struct CaptureState {
    active: bool,
    /// 지금 누르고 있는 키 (설정 파일 형식, 예: `"Ctrl+Shift"`)
    preview: Option<String>,
    result: Option<Captured>,
}

impl KeyCapture {
    /// 다음 단축키 입력을 받기 시작합니다. 전에 받아 두고 가져가지 않은 결과는 버립니다.
    pub fn start(&self) {
        let mut state = self.lock();
        state.active = true;
        state.preview = None;
        state.result = None;
    }

    /// 입력받기를 그만둡니다 (결과 없음).
    pub fn cancel(&self) {
        let mut state = self.lock();
        state.active = false;
        state.preview = None;
        state.result = None;
    }

    /// 지금 입력받는 중인지
    pub fn is_active(&self) -> bool {
        self.lock().active
    }

    /// 입력받는 중에 지금 누르고 있는 키 (화면 미리보기용)
    pub fn preview(&self) -> Option<String> {
        self.lock().preview.clone()
    }

    /// 받은 결과. 한 번 돌려주면 비웁니다.
    pub fn take_result(&self) -> Option<Captured> {
        self.lock().result.take()
    }

    /// 키보드 후크가 누르고 있는 키를 알립니다.
    pub(crate) fn set_preview(&self, preview: Option<String>) {
        let mut state = self.lock();
        if state.active {
            state.preview = preview;
        }
    }

    /// 키보드 후크가 입력받기를 끝냅니다.
    pub(crate) fn finish(&self, result: Captured) {
        let mut state = self.lock();
        if state.active {
            state.active = false;
            state.preview = None;
            state.result = Some(result);
        }
    }

    fn lock(&self) -> MutexGuard<'_, CaptureState> {
        // 잠금을 쥔 채 패닉이 나도 상태는 단순한 값이라 그대로 씁니다.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 녹음 단축키와 동작 방식
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub spec: HotkeySpec,
    pub mode: HotkeyMode,
}

/// 전역 단축키를 쓸 수 없는 이유
#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error("global hotkeys are not supported on this platform")]
    /// 지원하지 않는 OS에서만 만듭니다.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
    #[error("could not install the keyboard hook: {0}")]
    Install(std::io::Error),
}

/// 전역 단축키. 만들면 바로 키를 읽기 시작하고, 버리면 그만둡니다.
pub struct GlobalHotkey {
    #[cfg(windows)]
    hook: windows::Hook,
}

impl GlobalHotkey {
    /// 키 읽기를 시작합니다. `on_event`는 키보드 후크 스레드에서 불리므로 오래 걸리면 안 됩니다
    /// (채널에 보내기만 하세요). 시작할 때는 단축키가 없으니 [`set_binding`](Self::set_binding)으로 정합니다.
    #[cfg(windows)]
    pub fn start(
        capture: KeyCapture,
        on_event: impl Fn(HotkeyEvent) + Send + 'static,
    ) -> Result<Self, HotkeyError> {
        let hook = windows::Hook::start(capture, Box::new(on_event))?;
        Ok(Self { hook })
    }

    #[cfg(not(windows))]
    pub fn start(
        _capture: KeyCapture,
        _on_event: impl Fn(HotkeyEvent) + Send + 'static,
    ) -> Result<Self, HotkeyError> {
        Err(HotkeyError::Unsupported)
    }

    /// 녹음 단축키를 바꿉니다. `None`이면 녹음 단축키 없이 붙여넣기 감지와 입력받기만 합니다.
    pub fn set_binding(&self, binding: Option<Binding>) {
        #[cfg(windows)]
        self.hook.set_binding(binding);
        #[cfg(not(windows))]
        let _ = binding;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_delivers_result_once() {
        let capture = KeyCapture::default();
        let ui_side = capture.clone();
        assert!(!ui_side.is_active());

        ui_side.start();
        assert!(ui_side.is_active());
        capture.set_preview(Some("Ctrl+Shift".into()));
        assert_eq!(ui_side.preview().as_deref(), Some("Ctrl+Shift"));

        capture.finish(Captured::Key("Ctrl+Shift+Space".into()));
        assert!(!ui_side.is_active());
        assert_eq!(ui_side.preview(), None);
        assert_eq!(
            ui_side.take_result(),
            Some(Captured::Key("Ctrl+Shift+Space".into()))
        );
        assert_eq!(
            ui_side.take_result(),
            None,
            "결과는 한 번만 돌려줘야 합니다"
        );
    }

    #[test]
    fn finish_is_ignored_when_not_active() {
        let capture = KeyCapture::default();
        capture.finish(Captured::Cancelled);
        assert_eq!(capture.take_result(), None);

        capture.start();
        capture.cancel();
        capture.finish(Captured::Key("F13".into()));
        assert_eq!(
            capture.take_result(),
            None,
            "그만둔 뒤에 온 결과는 버려야 합니다"
        );
    }
}
