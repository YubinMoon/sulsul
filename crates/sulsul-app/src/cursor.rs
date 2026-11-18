//! 입력 중인 커서 ([`Cursor`] 구현).
//!
//! Windows에서 글을 입력받는 곳이 있는지는 맨 앞 창을 보고 이 순서로 판단합니다.
//! 1. 맨 앞 앱이 이 앱보다 높은 권한(관리자 권한 등)으로 돌거나 권한을 알 수 없으면, 키를 보내도 Windows가
//!    말없이 막으므로(UIPI) 입력할 곳이 없다고 봅니다.
//! 2. 시스템 캐럿이 있으면(`GetGUIThreadInfo`) 입력 중입니다. 메모장·워드 같은 Win32 입력란이 캐럿을 만듭니다.
//! 3. 터미널 창(Windows Terminal, 콘솔, mintty 등)은 늘 입력을 받습니다.
//! 4. 나머지는 UI Automation으로 키보드 포커스를 받은 요소가 고칠 수 있는 입력란인지 봅니다. 브라우저와
//!    Electron 앱(VS Code, Slack 등)은 캐럿을 만들지 않아 이 방법으로 알아봅니다.
//!
//! UI Automation은 다른 앱에 묻는 것이라 그 앱이 멈춰 있으면 오래 걸릴 수 있습니다. 그래서 전용 스레드에서
//! 시간 제한을 두고 묻고, 답이 늦으면 입력할 곳이 없다고 봅니다.
//!
//! 붙여넣기 키는 `SendInput`으로 보냅니다. 보통은 Ctrl+V이고, Ctrl+V가 붙여넣기가 아닌 터미널(mintty, PuTTY)에는
//! Shift+Insert를 보냅니다. 사용자가 Shift·Alt·Win을 누르고 있으면 다른 단축키가 되므로 뗄 때까지 잠깐 기다립니다.
//! 다른 OS는 아직 지원하지 않습니다(입력할 곳이 없다고 답함).

use crate::dictation::cursor::{Cursor, CursorError};

/// 시스템의 입력 중인 커서
#[derive(Default)]
pub struct SystemCursor {
    /// UI Automation에 묻는 스레드. 처음 물을 때 띄우고, 답이 없으면 버리고 다시 띄웁니다.
    #[cfg(windows)]
    focus: Option<win32::FocusProbe>,
}

#[cfg(not(windows))]
impl Cursor for SystemCursor {
    fn is_typing(&mut self) -> bool {
        false
    }

    fn send_paste(&mut self) -> Result<(), CursorError> {
        Err(CursorError::Unsupported)
    }
}

#[cfg(windows)]
mod win32 {
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::{Duration, Instant};

    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationValuePattern,
        UIA_CONTROLTYPE_ID, UIA_ComboBoxControlTypeId, UIA_DocumentControlTypeId,
        UIA_EditControlTypeId, UIA_PaneControlTypeId, UIA_ValuePatternId,
    };
    use windows::core::Interface as _;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
    use windows_sys::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
        TOKEN_QUERY, TokenIntegrityLevel,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, INPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
        MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput, VK_CONTROL, VK_INSERT, VK_LWIN, VK_MENU,
        VK_RWIN, VK_SHIFT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo,
        GetWindowThreadProcessId,
    };

    use super::{Cursor, CursorError, SystemCursor};
    use crate::hotkey::injected_key;

    /// UI Automation 스레드의 답을 기다리는 시간
    const FOCUS_TIMEOUT: Duration = Duration::from_millis(1500);
    /// UI Automation이 다른 앱의 답을 기다리는 시간 (ms)
    const AUTOMATION_TIMEOUT_MS: u32 = 1000;
    /// 사용자가 Shift·Alt·Win을 뗄 때까지 기다리는 시간과 확인 간격
    const MODIFIER_WAIT: Duration = Duration::from_secs(1);
    const MODIFIER_POLL: Duration = Duration::from_millis(10);
    /// 늘 입력을 받는 터미널 창의 클래스 이름
    const TERMINAL_CLASSES: &[&str] = &[
        // Windows Terminal
        "CASCADIA_HOSTING_WINDOW_CLASS",
        // 콘솔 (conhost)
        "ConsoleWindowClass",
        // Git Bash 등
        "mintty",
        "PuTTY",
        // ConEmu
        "VirtualConsoleClass",
        "org.wezfurlong.wezterm",
    ];
    /// Ctrl+V가 붙여넣기가 아니라 글자(^V)로 들어가는 터미널. Shift+Insert로 붙여넣습니다.
    const SHIFT_INSERT_CLASSES: &[&str] = &["mintty", "PuTTY"];
    /// Chromium 기반 앱(Chrome, Edge, Electron 앱)의 창 클래스
    const CHROMIUM_CLASS: &str = "Chrome_WidgetWin_1";
    /// Chromium이 접근성 정보를 아직 만들지 않았을 때 다시 묻는 횟수와 간격 (모두 0.8초)
    const CHROMIUM_RETRIES: u32 = 8;
    const CHROMIUM_RETRY_INTERVAL: Duration = Duration::from_millis(100);
    const VK_V: u16 = 0x56;

    /// 입력할 곳을 알아본 방법 (로그용)
    #[derive(Debug, Clone, Copy)]
    enum Found {
        Caret,
        Terminal,
        Automation,
    }

    /// 맨 앞 창
    struct Foreground {
        window: HWND,
        thread: u32,
        process: u32,
    }

    impl Foreground {
        fn now() -> Option<Self> {
            // SAFETY: 인자가 없는 함수입니다.
            let window = unsafe { GetForegroundWindow() };
            if window.is_null() {
                return None;
            }
            let mut process = 0;
            // SAFETY: 창 핸들은 방금 받은 것이고(그새 없어졌으면 0을 돌려줌), `process`는 쓸 수 있는 변수입니다.
            let thread = unsafe { GetWindowThreadProcessId(window, &mut process) };
            Some(Self {
                window,
                thread,
                process,
            })
        }

        fn class(&self) -> String {
            let mut buffer = [0u16; 128];
            // SAFETY: 버퍼 크기를 함께 넘깁니다. 창이 없어졌으면 0을 돌려줍니다.
            let len =
                unsafe { GetClassNameW(self.window, buffer.as_mut_ptr(), buffer.len() as i32) };
            String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0)])
        }

        /// 캐럿(깜박이는 커서)이 있는지
        fn has_caret(&self) -> bool {
            if self.thread == 0 {
                return false;
            }
            let mut info = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..GUITHREADINFO::default()
            };
            // SAFETY: `cbSize`를 채운 구조체를 넘깁니다.
            let ok = unsafe { GetGUIThreadInfo(self.thread, &mut info) } != 0;
            ok && !info.hwndCaret.is_null()
        }
    }

    impl Cursor for SystemCursor {
        /// UI Automation 스레드를 띄우고 한 번 물어 둡니다(답은 기다리지 않음). Chromium은 UI Automation이
        /// 물어 오면 그제야 접근성 정보를 만들기 시작하므로, 말하는 동안 준비를 끝내 두려는 것입니다.
        fn prepare(&mut self) {
            if let Some(probe) = self.probe() {
                probe.warm_up();
            }
        }

        fn is_typing(&mut self) -> bool {
            let started = Instant::now();
            let found = self.find_text_field();
            tracing::debug!(
                ?found,
                ms = started.elapsed().as_millis() as u64,
                "looked for a text cursor"
            );
            found.is_some()
        }

        fn send_paste(&mut self) -> Result<(), CursorError> {
            wait_for_modifier_release();
            let shift_insert = Foreground::now()
                .is_some_and(|fg| SHIFT_INSERT_CLASSES.contains(&fg.class().as_str()));
            let (modifier, key, key_flags) = if shift_insert {
                (VK_SHIFT, VK_INSERT, KEYEVENTF_EXTENDEDKEY)
            } else {
                (VK_CONTROL, VK_V, 0)
            };
            let inputs = [
                key_input(modifier, 0),
                key_input(key, key_flags),
                key_input(key, key_flags | KEYEVENTF_KEYUP),
                key_input(modifier, KEYEVENTF_KEYUP),
            ];
            // SAFETY: 채워진 INPUT 배열과 그 크기를 넘깁니다.
            let sent = unsafe {
                SendInput(
                    inputs.len() as u32,
                    inputs.as_ptr(),
                    std::mem::size_of::<INPUT>() as i32,
                )
            };
            if sent as usize == inputs.len() {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if sent > 0 {
                // 수정 키가 눌린 채로 남지 않게 뗌을 다시 보냅니다.
                let release = [key_input(modifier, KEYEVENTF_KEYUP)];
                // SAFETY: 위와 같습니다.
                unsafe { SendInput(1, release.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
            }
            Err(error.into())
        }
    }

    impl SystemCursor {
        fn find_text_field(&mut self) -> Option<Found> {
            let Some(foreground) = Foreground::now() else {
                tracing::debug!("no foreground window");
                return None;
            };
            if !can_send_keys(foreground.process) {
                tracing::info!(
                    "the foreground app may run with higher privileges; not typing into it"
                );
                return None;
            }
            if foreground.has_caret() {
                return Some(Found::Caret);
            }
            let class = foreground.class();
            if TERMINAL_CLASSES.contains(&class.as_str()) {
                return Some(Found::Terminal);
            }
            tracing::debug!(class, "foreground window has no caret");
            let chromium = class == CHROMIUM_CLASS;
            for attempt in 0..=CHROMIUM_RETRIES {
                let focus = self.ask_automation()?;
                // Chromium이 접근성 정보를 아직 만들지 않았으면 포커스가 창 전체(Pane)로만 보입니다.
                if !(chromium && focus.control == UIA_PaneControlTypeId) {
                    return focus.editable.then_some(Found::Automation);
                }
                if attempt < CHROMIUM_RETRIES {
                    std::thread::sleep(CHROMIUM_RETRY_INTERVAL);
                }
            }
            tracing::debug!("Chromium did not expose the focused element");
            None
        }

        /// UI Automation에 묻는 스레드. 없으면 띄웁니다.
        fn probe(&mut self) -> Option<&FocusProbe> {
            if self.focus.is_none() {
                match FocusProbe::start() {
                    Ok(probe) => self.focus = Some(probe),
                    Err(error) => {
                        tracing::warn!("could not start the UI Automation thread: {error}");
                        return None;
                    }
                }
            }
            self.focus.as_ref()
        }

        /// 키보드 포커스를 받은 요소. 답이 없으면 `None`.
        fn ask_automation(&mut self) -> Option<Focus> {
            let answer = self.probe()?.ask();
            if answer.is_none() {
                // 멈춘 앱에 묶였을 수 있으니 이 스레드는 버리고 다음에 새로 띄웁니다.
                tracing::warn!("UI Automation did not answer in time");
                self.focus = None;
            }
            answer
        }
    }

    /// 수정 키(Shift·Alt·Win)를 뗄 때까지 잠깐 기다립니다. 그래도 누르고 있으면 그냥 보냅니다.
    fn wait_for_modifier_release() {
        let deadline = Instant::now() + MODIFIER_WAIT;
        let held = || {
            [VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
                .into_iter()
                // SAFETY: 키 코드만 받는 함수입니다. 맨 위 비트가 서 있으면 눌려 있습니다.
                .any(|vk| unsafe { GetAsyncKeyState(i32::from(vk)) } < 0)
        };
        while held() {
            if Instant::now() >= deadline {
                tracing::debug!("modifier keys are still held; sending the paste keys anyway");
                return;
            }
            std::thread::sleep(MODIFIER_POLL);
        }
    }

    fn key_input(vk: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        // SAFETY: 키 코드만 받는 함수입니다.
        let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) };
        injected_key(vk, scan as u16, flags)
    }

    /// `process`에 키를 보낼 수 있는지 (그 앱의 무결성 수준이 이 앱보다 높지 않은지).
    /// 알 수 없으면 보내지 않습니다. 이 앱의 수준을 모르면 막지 않습니다.
    fn can_send_keys(process: u32) -> bool {
        // SAFETY: 이 프로세스를 가리키는 가짜 핸들이라 닫지 않습니다.
        let Some(ours) = integrity_level(unsafe { GetCurrentProcess() }) else {
            return true;
        };
        // SAFETY: 조회 권한만 요청합니다. 받은 핸들은 아래에서 닫습니다.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process) };
        if handle.is_null() {
            return false;
        }
        let theirs = integrity_level(handle);
        // SAFETY: 위에서 연 핸들입니다.
        unsafe { CloseHandle(handle) };
        theirs.is_some_and(|theirs| theirs <= ours)
    }

    /// 프로세스의 무결성 수준(낮음 0x1000, 보통 0x2000, 높음 0x3000 …)
    fn integrity_level(process: HANDLE) -> Option<u32> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: 열린 프로세스 핸들과 받을 변수를 넘깁니다. 받은 토큰은 아래에서 닫습니다.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        let mut size = 0;
        // SAFETY: 크기만 묻습니다 (버퍼가 없어 실패하고 필요한 크기를 알려 줌).
        unsafe {
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                std::ptr::null_mut(),
                0,
                &mut size,
            )
        };
        // TOKEN_MANDATORY_LABEL은 포인터를 담으므로 8바이트로 맞춘 버퍼에 받습니다.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
        // SAFETY: 크기만큼의 버퍼를 넘깁니다.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        } != 0;
        // SAFETY: 위에서 연 토큰입니다.
        unsafe { CloseHandle(token) };
        if !ok {
            return None;
        }
        // SAFETY: 성공하면 버퍼 앞에 TOKEN_MANDATORY_LABEL이 있고, 그 SID는 같은 버퍼 안을 가리킵니다.
        unsafe {
            let label = &*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
            let sid = label.Label.Sid;
            let count = *GetSidSubAuthorityCount(sid);
            if count == 0 {
                return None;
            }
            Some(*GetSidSubAuthority(sid, u32::from(count) - 1))
        }
    }

    /// 키보드 포커스를 받은 요소
    #[derive(Debug, Clone, Copy, Default)]
    struct Focus {
        control: UIA_CONTROLTYPE_ID,
        /// 고칠 수 있는 입력란인지
        editable: bool,
    }

    /// UI Automation에 묻는 전용 스레드. 버리면 스레드가 하던 일을 마치고 끝납니다.
    pub(super) struct FocusProbe {
        requests: Sender<Sender<Focus>>,
    }

    impl FocusProbe {
        fn start() -> std::io::Result<Self> {
            let (requests, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("sulsul-focus".into())
                .spawn(move || run_probe(rx))?;
            Ok(Self { requests })
        }

        /// 키보드 포커스를 받은 요소를 묻습니다. 답이 없거나 늦으면 `None`.
        fn ask(&self) -> Option<Focus> {
            let (reply, answer) = mpsc::channel();
            self.requests.send(reply).ok()?;
            answer.recv_timeout(FOCUS_TIMEOUT).ok()
        }

        /// 묻기만 하고 답은 기다리지 않습니다.
        fn warm_up(&self) {
            let (reply, _answer) = mpsc::channel();
            let _ = self.requests.send(reply);
        }
    }

    fn run_probe(requests: Receiver<Sender<Focus>>) {
        // SAFETY: 이 스레드에서 COM을 처음 초기화합니다(MTA). 아래에서 CoUninitialize로 짝을 맞춥니다.
        let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if init.is_err() {
            tracing::warn!("could not initialize COM for UI Automation: {init:?}");
            for reply in requests {
                let _ = reply.send(Focus::default());
            }
            return;
        }
        {
            let automation = create_automation();
            for reply in requests {
                let focus = automation.as_ref().map(focused).unwrap_or_default();
                let _ = reply.send(focus);
            }
        }
        // SAFETY: 위에서 초기화한 COM을 닫습니다. UI Automation 객체는 이미 버렸습니다.
        unsafe { CoUninitialize() };
    }

    fn create_automation() -> Option<IUIAutomation> {
        // SAFETY: COM을 초기화한 스레드에서 만듭니다.
        let automation: IUIAutomation =
            match unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) } {
                Ok(automation) => automation,
                Err(error) => {
                    tracing::warn!("could not start UI Automation: {error}");
                    return None;
                }
            };
        // 멈춘 앱에 물어도 오래 기다리지 않게 시간 제한을 둡니다.
        if let Ok(automation) = automation.cast::<IUIAutomation2>() {
            // SAFETY: 값만 바꾸는 호출입니다.
            unsafe {
                let _ = automation.SetConnectionTimeout(AUTOMATION_TIMEOUT_MS);
                let _ = automation.SetTransactionTimeout(AUTOMATION_TIMEOUT_MS);
            }
        }
        Some(automation)
    }

    fn focused(automation: &IUIAutomation) -> Focus {
        // SAFETY: COM을 초기화한 스레드에서 부릅니다. 요소와 패턴은 이 함수 안에서만 씁니다.
        unsafe {
            let element = match automation.GetFocusedElement() {
                Ok(element) => element,
                Err(error) => {
                    tracing::debug!("no focused element: {error}");
                    return Focus::default();
                }
            };
            let control = element.CurrentControlType().unwrap_or_default();
            let enabled = element
                .CurrentIsEnabled()
                .is_ok_and(|enabled| enabled.as_bool());
            let read_only = element
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .and_then(|value| value.CurrentIsReadOnly())
                .ok()
                .map(|read_only| read_only.as_bool());
            tracing::debug!(
                control_type = control.0,
                enabled,
                ?read_only,
                "focused element"
            );
            Focus {
                control,
                editable: enabled && editable(control, read_only),
            }
        }
    }

    /// UI Automation 요소의 종류와 (값 패턴이 있으면) 읽기 전용인지로 고칠 수 있는 입력란인지 정합니다.
    fn editable(control: UIA_CONTROLTYPE_ID, read_only: Option<bool>) -> bool {
        if control == UIA_EditControlTypeId {
            // 입력란은 값 패턴이 없어도(서식 있는 입력란 등) 입력란으로 봅니다.
            read_only != Some(true)
        } else if control == UIA_DocumentControlTypeId || control == UIA_ComboBoxControlTypeId {
            // 문서·콤보 상자는 읽기 전용이 아니라고 밝힐 때만입니다. 브라우저의 웹 페이지 자체도 문서입니다.
            read_only == Some(false)
        } else {
            false
        }
    }

    #[cfg(test)]
    mod tests {
        use windows::Win32::UI::Accessibility::UIA_ButtonControlTypeId;

        use super::*;

        #[test]
        fn edit_fields_are_editable_unless_read_only() {
            assert!(
                editable(UIA_EditControlTypeId, None),
                "값 패턴이 없는 입력란"
            );
            assert!(editable(UIA_EditControlTypeId, Some(false)));
            assert!(
                !editable(UIA_EditControlTypeId, Some(true)),
                "읽기 전용 입력란"
            );
        }

        #[test]
        fn documents_need_to_say_they_are_writable() {
            assert!(editable(UIA_DocumentControlTypeId, Some(false)));
            assert!(
                !editable(UIA_DocumentControlTypeId, None),
                "웹 페이지처럼 값 패턴이 없는 문서"
            );
            assert!(!editable(UIA_DocumentControlTypeId, Some(true)));
            assert!(editable(UIA_ComboBoxControlTypeId, Some(false)));
            assert!(!editable(UIA_ComboBoxControlTypeId, None));
        }

        #[test]
        fn other_controls_are_not_text_fields() {
            assert!(!editable(UIA_ButtonControlTypeId, Some(false)));
        }

        #[test]
        fn this_process_can_send_keys_to_itself() {
            assert!(can_send_keys(std::process::id()));
            assert!(
                integrity_level(unsafe { GetCurrentProcess() }).is_some(),
                "이 앱의 무결성 수준을 읽을 수 있어야 합니다"
            );
        }
    }
}
