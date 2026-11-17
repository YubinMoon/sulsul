//! Windows 전역 단축키: 저수준 키보드·마우스 후크.
//!
//! 후크는 전용 스레드에서 메시지 루프를 돌며 받습니다. 후크 함수가 오래 걸리면 Windows가 후크를
//! 말없이 떼어 버리므로, 후크 함수에서는 판단([`Machine`])과 채널 보내기만 합니다.
//! 판단에 따라 키를 막거나(1을 돌려줌) 다음 후크로 넘기고, 메뉴 막대·시작 메뉴가 열리지 않게
//! 쓰지 않는 키 코드(`0xE8`)를 가짜로 눌렀다 떼기도 합니다.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak, mpsc};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput, VK_ADD, VK_APPS, VK_BACK, VK_CAPITAL,
    VK_CONTROL, VK_DECIMAL, VK_DELETE, VK_DIVIDE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F24,
    VK_HANGUL, VK_HANJA, VK_HOME, VK_INSERT, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN,
    VK_MENU, VK_MULTIPLY, VK_NEXT, VK_NUMLOCK, VK_NUMPAD0, VK_NUMPAD9, VK_OEM_1, VK_OEM_2,
    VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD,
    VK_OEM_PLUS, VK_PACKET, VK_PAUSE, VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_SCROLL, VK_SHIFT, VK_SNAPSHOT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER,
    WM_XBUTTONDOWN,
};

use super::machine::{CaptureUpdate, KeyInput, KeyProbe, Machine, Mask};
use super::{Binding, HotkeyError, HotkeyEvent, Key, KeyCapture};

/// 우리가 넣은 가짜 키 표시 (`dwExtraInfo`). 후크가 자기가 넣은 입력을 다시 판단하지 않게 합니다.
const SIGNATURE: usize = 0x5355_4C53;
/// 메뉴 막대·시작 메뉴를 막는 데 쓰는 가짜 키 (Windows가 쓰지 않는 키 코드)
const MASK_VK: u16 = 0xE8;
/// 오른쪽 Ctrl·Alt의 스캔 코드. 한국어 키보드에서 이 키가 한자·한/영으로 들어올 때 알아보는 데 씁니다.
const SCAN_CTRL: u32 = 0x1D;
const SCAN_ALT: u32 = 0x38;
const SCAN_RIGHT_SHIFT: u32 = 0x36;
/// AltGr를 누르면 Windows가 함께 만드는 가짜 왼쪽 Ctrl의 스캔 코드
const SCAN_ALTGR_CTRL: u32 = 0x21D;

/// 창으로 직접 온 키도 egui보다 먼저 받습니다. 원격 입력처럼 저수준 후크를 거치지 않는 입력도
/// 수정 키의 방향을 보존하고, Space·Enter가 입력란 버튼을 다시 누르지 않게 합니다.
pub fn install_window_capture(options: &mut eframe::NativeOptions, capture: KeyCapture) {
    use winit::platform::windows::EventLoopBuilderExtWindows as _;

    options.event_loop_builder = Some(Box::new(move |builder| {
        let mut window = WindowCapture {
            input: capture.window_input,
        };
        builder.with_msg_hook(move |message| {
            // SAFETY: winit이 메시지를 전달하는 동안 유효한 MSG 포인터입니다.
            window.on_message(unsafe { &*(message as *const MSG) })
        });
    }));
}

struct WindowCapture {
    input: WindowInput,
}

/// 창 입력도 전역 후크와 같은 판단 상태·녹음 알림을 씁니다. 약한 참조라 후크를 끄면 함께 끊깁니다.
#[derive(Clone, Debug, Default)]
pub(super) struct WindowInput(Arc<Mutex<Weak<Context>>>);

impl WindowCapture {
    fn on_message(&mut self, message: &MSG) -> bool {
        if !matches!(
            message.message,
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP
        ) {
            return false;
        }
        let info = KBDLLHOOKSTRUCT {
            vkCode: message.wParam as u32,
            scanCode: ((message.lParam as usize >> 16) & 0xFF) as u32,
            flags: if message.lParam as usize & (1 << 24) != 0 {
                LLKHF_EXTENDED
            } else {
                0
            },
            ..KBDLLHOOKSTRUCT::default()
        };
        let context = self
            .input
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .upgrade();
        let Some(context) = context else {
            return false;
        };
        context.on_key_with_probe(message.message, &info, &WindowKeyState)
    }
}

/// 후크 스레드
pub struct Hook {
    machine: Arc<Mutex<Machine>>,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl Hook {
    pub fn start(
        capture: KeyCapture,
        on_event: Box<dyn Fn(HotkeyEvent) + Send + Sync>,
    ) -> Result<Self, HotkeyError> {
        let machine = Arc::new(Mutex::new(Machine::default()));
        let (ready_tx, ready_rx) = mpsc::channel();
        let context = Arc::new(Context {
            machine: Arc::clone(&machine),
            capture,
            on_event,
        });
        *context
            .capture
            .window_input
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(&context);
        let thread = std::thread::Builder::new()
            .name("sulsul-hotkey".into())
            .spawn(move || run(context, ready_tx))
            .map_err(HotkeyError::Install)?;
        let ready = ready_rx.recv().unwrap_or_else(|_| {
            Err(std::io::Error::other(
                "the keyboard hook thread ended unexpectedly",
            ))
        });
        match ready {
            Ok(thread_id) => {
                tracing::info!("keyboard hook installed");
                Ok(Self {
                    machine,
                    thread_id,
                    thread: Some(thread),
                })
            }
            Err(error) => {
                let _ = thread.join();
                Err(HotkeyError::Install(error))
            }
        }
    }

    pub fn set_binding(&self, binding: Option<Binding>) {
        lock(&self.machine).set_binding(binding);
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        // SAFETY: 스레드 ID는 메시지 큐를 만든 뒤에 받은 값입니다. 스레드가 이미 끝났으면 실패할 뿐입니다.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 후크 함수가 쓰는 상태. 후크 스레드에만 있습니다.
struct Context {
    machine: Arc<Mutex<Machine>>,
    capture: KeyCapture,
    on_event: Box<dyn Fn(HotkeyEvent) + Send + Sync>,
}

thread_local! {
    static CONTEXT: RefCell<Option<Arc<Context>>> = const { RefCell::new(None) };
}

fn run(context: Arc<Context>, ready: mpsc::Sender<std::io::Result<u32>>) {
    let mut msg = MSG::default();
    // 메시지 큐를 먼저 만들어 둡니다. 그래야 끝내라는 메시지(PostThreadMessageW)가 사라지지 않습니다.
    // SAFETY: `msg`는 유효한 MSG이고, 메시지를 꺼내지 않고 들여다보기만 합니다.
    unsafe {
        PeekMessageW(
            &mut msg,
            std::ptr::null_mut(),
            WM_USER,
            WM_USER,
            PM_NOREMOVE,
        )
    };
    CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));

    // SAFETY: 이 실행 파일 자신의 모듈 핸들을 얻습니다.
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    // SAFETY: 후크 함수는 이 모듈에 있는 `extern "system"` 함수이고, 이 스레드가 메시지 루프를 돕니다.
    let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0) };
    if keyboard.is_null() {
        let error = std::io::Error::last_os_error();
        CONTEXT.with(|slot| slot.borrow_mut().take());
        let _ = ready.send(Err(error));
        return;
    }
    // 마우스 후크는 "단축키를 누른 채 클릭"을 가려내는 데만 씁니다. 실패해도 단축키는 동작합니다.
    // SAFETY: 위와 같습니다.
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0) };
    if mouse.is_null() {
        tracing::warn!(
            "could not install the mouse hook: {}",
            std::io::Error::last_os_error()
        );
    }
    // SAFETY: 인자가 없는 함수입니다.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    loop {
        // SAFETY: `msg`는 유효한 MSG입니다. 후크는 이 호출 안에서 불립니다.
        let result = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        // SAFETY: 방금 받은 메시지입니다.
        unsafe { DispatchMessageW(&msg) };
    }

    // SAFETY: 이 스레드에서 설치한 후크입니다.
    unsafe {
        UnhookWindowsHookEx(keyboard);
        if !mouse.is_null() {
            UnhookWindowsHookEx(mouse);
        }
    }
    CONTEXT.with(|slot| slot.borrow_mut().take());
    tracing::info!("keyboard hook removed");
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: `HC_ACTION`이면 `lparam`은 이 호출 동안 유효한 KBDLLHOOKSTRUCT를 가리킵니다.
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let block = CONTEXT.with(|slot| {
            // 다시 들어오는 일은 없어야 하지만, 혹시 그러면 키를 그대로 넘깁니다.
            let Ok(slot) = slot.try_borrow() else {
                return false;
            };
            slot.as_ref()
                .is_some_and(|context| context.on_key(wparam as u32, info))
        });
        if block {
            return 1;
        }
    }
    // SAFETY: 받은 인자를 그대로 다음 후크에 넘깁니다.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let clicked = matches!(
        wparam as u32,
        WM_LBUTTONDOWN
            | WM_RBUTTONDOWN
            | WM_MBUTTONDOWN
            | WM_XBUTTONDOWN
            | WM_MOUSEWHEEL
            | WM_MOUSEHWHEEL
    );
    if code == HC_ACTION as i32 && clicked {
        CONTEXT.with(|slot| {
            if let Ok(slot) = slot.try_borrow()
                && let Some(context) = slot.as_ref()
            {
                context.on_other_input();
            }
        });
    }
    // SAFETY: 받은 인자를 그대로 다음 후크에 넘깁니다.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

impl Context {
    /// 키 하나를 처리합니다. 막아야 하면 `true`.
    fn on_key(&self, message: u32, info: &KBDLLHOOKSTRUCT) -> bool {
        self.on_key_with_probe(message, info, &AsyncKeyState)
    }

    fn on_key_with_probe(
        &self,
        message: u32,
        info: &KBDLLHOOKSTRUCT,
        probe: &dyn KeyProbe,
    ) -> bool {
        if info.dwExtraInfo == SIGNATURE {
            return false;
        }
        let Some((key, remapped)) = key_of(info) else {
            return false;
        };
        let input = KeyInput {
            key,
            down: matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN),
            remapped,
            raw: info.vkCode,
            scan: place_of(info),
        };
        let capturing = self.capture.is_active();
        let verdict = lock(&self.machine).handle(input, capturing, probe);

        match verdict.capture {
            Some(CaptureUpdate::Preview(preview)) => self.capture.set_preview(preview),
            Some(CaptureUpdate::Finish(result)) => self.capture.finish(result),
            None => {}
        }
        if let Some(event) = verdict.event {
            (self.on_event)(event);
        }
        if verdict.paste {
            (self.on_event)(HotkeyEvent::Paste);
        }
        match verdict.mask {
            Mask::None => verdict.swallow,
            Mask::Now => {
                send_mask(None);
                verdict.swallow
            }
            Mask::BeforeRelease => {
                send_mask(Some(info));
                true
            }
        }
    }

    fn on_other_input(&self) {
        if let Some(event) = lock(&self.machine).other_input() {
            (self.on_event)(event);
        }
    }
}

/// 지금 OS가 아는 키 상태
struct AsyncKeyState;

/// 창 입력은 메시지의 누름·뗌을 기준으로 합니다. 원격 입력은 OS의 비동기 키 상태에 없을 수 있습니다.
struct WindowKeyState;

impl KeyProbe for WindowKeyState {
    fn is_down(&self, _raw: u32) -> bool {
        true
    }
}

impl KeyProbe for AsyncKeyState {
    fn is_down(&self, raw: u32) -> bool {
        let Ok(vk) = i32::try_from(raw) else {
            return false;
        };
        // SAFETY: 키 코드만 받는 함수입니다. 맨 위 비트가 서 있으면 눌려 있습니다.
        unsafe { GetAsyncKeyState(vk) < 0 }
    }
}

/// 가짜 키를 눌렀다 뗍니다. `release`가 있으면 그 뒤에 그 키의 뗌을 다시 보냅니다.
fn send_mask(release: Option<&KBDLLHOOKSTRUCT>) {
    let mut inputs = [
        keyboard_input(MASK_VK, 0, 0),
        keyboard_input(MASK_VK, 0, KEYEVENTF_KEYUP),
        INPUT::default(),
    ];
    let mut count = 2;
    if let Some(info) = release {
        let mut flags = KEYEVENTF_KEYUP;
        if info.flags & LLKHF_EXTENDED != 0 {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        inputs[2] = keyboard_input(info.vkCode as u16, info.scanCode as u16, flags);
        count = 3;
    }
    // SAFETY: `inputs`의 앞 `count`개는 채워진 INPUT이고 크기를 함께 넘깁니다.
    let sent = unsafe {
        SendInput(
            count as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent as usize != count {
        tracing::debug!("SendInput sent {sent} of {count} mask inputs");
    }
}

fn keyboard_input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: SIGNATURE,
            },
        },
    }
}

/// 키보드에서 키의 자리 ([`KeyInput::scan`]). 모르면 0.
/// 글자를 바로 넣는 가짜 키(`VK_PACKET`)는 스캔 코드 자리에 글자가 들어 있으므로 쓰지 않습니다.
fn place_of(info: &KBDLLHOOKSTRUCT) -> u32 {
    if info.vkCode == u32::from(VK_PACKET) || info.scanCode == 0 {
        return 0;
    }
    if info.flags & LLKHF_EXTENDED != 0 {
        0xE000 | info.scanCode
    } else {
        info.scanCode
    }
}

/// Windows 키 코드를 [`Key`]로 바꿉니다. 두 번째 값은 한자·한/영으로 들어온 오른쪽 Ctrl·Alt인지입니다.
/// 무시할 입력(AltGr가 만드는 가짜 Ctrl)이면 `None`.
fn key_of(info: &KBDLLHOOKSTRUCT) -> Option<(Key, bool)> {
    let extended = info.flags & LLKHF_EXTENDED != 0;
    let scan = info.scanCode;
    let Ok(vk) = u16::try_from(info.vkCode) else {
        return Some((Key::Other(info.vkCode), false));
    };
    if vk == VK_LCONTROL && scan == SCAN_ALTGR_CTRL {
        return None;
    }
    let key = match vk {
        VK_LCONTROL => Key::LeftCtrl,
        VK_RCONTROL => Key::RightCtrl,
        VK_CONTROL if extended => Key::RightCtrl,
        VK_CONTROL => Key::LeftCtrl,
        VK_LMENU => Key::LeftAlt,
        VK_RMENU => Key::RightAlt,
        VK_MENU if extended => Key::RightAlt,
        VK_MENU => Key::LeftAlt,
        VK_LSHIFT => Key::LeftShift,
        VK_RSHIFT => Key::RightShift,
        VK_SHIFT if scan == SCAN_RIGHT_SHIFT => Key::RightShift,
        VK_SHIFT => Key::LeftShift,
        VK_LWIN => Key::LeftWin,
        VK_RWIN => Key::RightWin,
        // 한국어 키보드 배열에 따라 오른쪽 Ctrl은 한자, 오른쪽 Alt는 한/영 키로 들어옵니다.
        VK_HANJA if scan == SCAN_CTRL => return Some((Key::RightCtrl, true)),
        VK_HANGUL if scan == SCAN_ALT => return Some((Key::RightAlt, true)),
        VK_HANJA => Key::Hanja,
        VK_HANGUL => Key::HangulMode,
        0x41..=0x5A => Key::Letter(vk as u8),
        0x30..=0x39 => Key::Digit((vk - 0x30) as u8),
        VK_F1..=VK_F24 => Key::Function((vk - VK_F1 + 1) as u8),
        VK_NUMPAD0..=VK_NUMPAD9 => Key::Numpad((vk - VK_NUMPAD0) as u8),
        VK_SPACE => Key::Space,
        VK_RETURN => Key::Enter,
        VK_TAB => Key::Tab,
        VK_BACK => Key::Backspace,
        VK_ESCAPE => Key::Escape,
        VK_INSERT => Key::Insert,
        VK_DELETE => Key::Delete,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_CAPITAL => Key::CapsLock,
        VK_SCROLL => Key::ScrollLock,
        VK_NUMLOCK => Key::NumLock,
        VK_PAUSE => Key::Pause,
        VK_SNAPSHOT => Key::PrintScreen,
        VK_APPS => Key::ContextMenu,
        VK_ADD => Key::NumpadAdd,
        VK_SUBTRACT => Key::NumpadSubtract,
        VK_MULTIPLY => Key::NumpadMultiply,
        VK_DIVIDE => Key::NumpadDivide,
        VK_DECIMAL => Key::NumpadDecimal,
        VK_OEM_MINUS => Key::Minus,
        VK_OEM_PLUS => Key::Equal,
        VK_OEM_4 => Key::BracketLeft,
        VK_OEM_6 => Key::BracketRight,
        VK_OEM_5 => Key::Backslash,
        VK_OEM_1 => Key::Semicolon,
        VK_OEM_7 => Key::Quote,
        VK_OEM_COMMA => Key::Comma,
        VK_OEM_PERIOD => Key::Period,
        VK_OEM_2 => Key::Slash,
        VK_OEM_3 => Key::Backquote,
        _ => Key::Other(info.vkCode),
    };
    Some((key, false))
}

fn lock(machine: &Mutex<Machine>) -> MutexGuard<'_, Machine> {
    // 판단 상태는 단순한 값이라 잠금을 쥔 채 패닉이 났어도 그대로 씁니다.
    machine.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_context(
        capture: &KeyCapture,
        on_event: impl Fn(HotkeyEvent) + Send + Sync + 'static,
    ) -> (WindowCapture, Arc<Context>) {
        let context = Arc::new(Context {
            machine: Arc::new(Mutex::new(Machine::default())),
            capture: capture.clone(),
            on_event: Box::new(on_event),
        });
        *capture
            .window_input
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(&context);
        (
            WindowCapture {
                input: capture.window_input.clone(),
            },
            context,
        )
    }

    fn window_message(vk: u16, scan: u32, extended: bool, down: bool) -> MSG {
        MSG {
            message: if down { WM_KEYDOWN } else { WM_KEYUP },
            wParam: usize::from(vk),
            lParam: ((scan << 16) | (u32::from(extended) << 24)) as isize,
            ..MSG::default()
        }
    }

    #[test]
    fn window_messages_capture_combination_without_the_global_hook() {
        let capture = KeyCapture::default();
        let (mut window, _context) = window_context(&capture, |_| {});
        capture.start();
        for (vk, scan) in [(VK_CONTROL, SCAN_CTRL), (VK_SHIFT, 0x2A), (VK_SPACE, 0x39)] {
            assert!(window.on_message(&window_message(vk, scan, false, true)));
            assert!(
                capture.is_active(),
                "누르는 동안에는 입력을 확정하지 않아야 합니다"
            );
        }
        assert_eq!(capture.preview().as_deref(), Some("Ctrl+Shift+Space"));
        for (vk, scan) in [(VK_CONTROL, SCAN_CTRL), (VK_SHIFT, 0x2A)] {
            assert!(window.on_message(&window_message(vk, scan, false, false)));
            assert!(capture.is_active(), "모든 키를 뗄 때까지 기다려야 합니다");
        }
        assert!(window.on_message(&window_message(VK_SPACE, 0x39, false, false)));
        assert_eq!(
            capture.take_result(),
            Some(super::super::Captured::Key("Ctrl+Shift+Space".into()))
        );
        assert!(!capture.is_active());

        // 확정 뒤의 보통 입력은 화면으로 전달합니다.
        assert!(!window.on_message(&window_message(VK_SPACE, 0x39, false, true)));
        assert!(!window.on_message(&window_message(VK_SPACE, 0x39, false, false)));

        capture.start();
        assert!(window.on_message(&window_message(VK_CONTROL, SCAN_CTRL, true, true)));
        assert_eq!(capture.preview().as_deref(), Some("RightCtrl"));
        assert!(window.on_message(&window_message(VK_CONTROL, SCAN_CTRL, true, false)));
        assert_eq!(
            capture.take_result(),
            Some(super::super::Captured::Key("RightCtrl".into()))
        );
    }

    #[test]
    fn window_messages_cancel_on_escape_without_changing_the_hotkey() {
        let capture = KeyCapture::default();
        let (mut window, _context) = window_context(&capture, |_| {});
        capture.start();
        assert!(window.on_message(&window_message(VK_ESCAPE, 1, false, true)));
        assert!(window.on_message(&window_message(VK_ESCAPE, 1, false, false)));
        assert_eq!(
            capture.take_result(),
            Some(super::super::Captured::Cancelled)
        );
    }

    #[test]
    fn window_hotkey_reaches_the_dictation_sink_in_both_modes() {
        use sulsul_core::config::HotkeyMode;

        for mode in [HotkeyMode::Toggle, HotkeyMode::PushToTalk] {
            let capture = KeyCapture::default();
            let (tx, rx) = mpsc::channel();
            let (mut window, context) = window_context(&capture, move |event| {
                tx.send(event).expect("녹음 알림을 받아야 합니다");
            });
            lock(&context.machine).set_binding(Some(Binding {
                spec: super::super::HotkeySpec::parse("Ctrl+Shift+Space").expect("올바른 조합"),
                mode,
            }));
            for down in [true, false] {
                for (vk, scan) in [(VK_CONTROL, SCAN_CTRL), (VK_SHIFT, 0x2A), (VK_SPACE, 0x39)] {
                    window.on_message(&window_message(vk, scan, false, down));
                }
            }
            let expected = match mode {
                HotkeyMode::Toggle => vec![HotkeyEvent::Toggle],
                HotkeyMode::PushToTalk => vec![HotkeyEvent::Press, HotkeyEvent::Release],
            };
            assert_eq!(rx.try_iter().collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn hook_and_window_do_not_deliver_a_modifier_hotkey_twice() {
        let capture = KeyCapture::default();
        let (tx, rx) = mpsc::channel();
        let (mut window, context) = window_context(&capture, move |event| {
            tx.send(event).expect("녹음 알림을 받아야 합니다");
        });
        lock(&context.machine).set_binding(Some(Binding {
            spec: super::super::HotkeySpec::parse("RightCtrl").expect("올바른 키"),
            mode: sulsul_core::config::HotkeyMode::Toggle,
        }));
        for (message, down) in [(WM_KEYDOWN, true), (WM_KEYUP, false)] {
            context.on_key_with_probe(
                message,
                &info(VK_RCONTROL, SCAN_CTRL, true),
                &WindowKeyState,
            );
            window.on_message(&window_message(VK_CONTROL, SCAN_CTRL, true, down));
        }
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![HotkeyEvent::Toggle]);
    }

    /// 한국어 키보드 종류 3(`kbd101c.dll`)에서 Shift를 먼저 떼며 Shift+Space로 한/영을 바꾼 뒤의 OS 상태:
    /// 한/영 키 뗌이 오지 않아 `GetAsyncKeyState(VK_HANGUL)`이 계속 눌림을 답합니다.
    struct HangulStuck;

    impl KeyProbe for HangulStuck {
        fn is_down(&self, raw: u32) -> bool {
            raw == u32::from(VK_HANGUL)
        }
    }

    /// 저수준 후크가 실제로 받은 순서 그대로입니다 (Shift 누름 → 한/영 누름 → Shift 뗌 → Space 뗌).
    #[test]
    fn right_ctrl_still_works_after_switching_language_with_shift_space() {
        let capture = KeyCapture::default();
        let (tx, rx) = mpsc::channel();
        let (_window, context) = window_context(&capture, move |event| {
            tx.send(event).expect("녹음 알림을 받아야 합니다");
        });
        lock(&context.machine).set_binding(Some(Binding {
            spec: super::super::HotkeySpec::parse("RightCtrl").expect("올바른 키"),
            mode: sulsul_core::config::HotkeyMode::Toggle,
        }));
        for (message, vk, scan) in [
            (WM_KEYDOWN, VK_LSHIFT, 0x2A),
            (WM_KEYDOWN, VK_HANGUL, 0x39),
            (WM_KEYUP, VK_LSHIFT, 0x2A),
            (WM_KEYUP, VK_SPACE, 0x39),
        ] {
            assert!(
                !context.on_key_with_probe(message, &info(vk, scan, false), &HangulStuck),
                "한/영 전환은 막지 않아야 합니다"
            );
        }
        for message in [WM_KEYDOWN, WM_KEYUP] {
            context.on_key_with_probe(message, &info(VK_RCONTROL, SCAN_CTRL, true), &HangulStuck);
        }
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![HotkeyEvent::Toggle]);
    }

    #[test]
    fn place_keeps_extended_keys_apart_and_ignores_packets() {
        assert_eq!(place_of(&info(VK_LCONTROL, SCAN_CTRL, false)), 0x1D);
        assert_eq!(place_of(&info(VK_RCONTROL, SCAN_CTRL, true)), 0xE01D);
        assert_eq!(
            place_of(&info(VK_PACKET, 0x39, false)),
            0,
            "글자 값은 자리가 아님"
        );
        assert_eq!(place_of(&info(0x41, 0, false)), 0);
    }

    fn info(vk: u16, scan: u32, extended: bool) -> KBDLLHOOKSTRUCT {
        KBDLLHOOKSTRUCT {
            vkCode: u32::from(vk),
            scanCode: scan,
            flags: if extended { LLKHF_EXTENDED } else { 0 },
            time: 0,
            dwExtraInfo: 0,
        }
    }

    #[test]
    fn korean_layout_right_keys_are_recognized() {
        // 오른쪽 Ctrl이 한자 키로, 오른쪽 Alt가 한/영 키로 들어오는 배열
        assert_eq!(
            key_of(&info(VK_HANJA, SCAN_CTRL, true)),
            Some((Key::RightCtrl, true))
        );
        assert_eq!(
            key_of(&info(VK_HANGUL, SCAN_ALT, true)),
            Some((Key::RightAlt, true))
        );
        // 따로 있는 한자·한/영 키는 그대로
        assert_eq!(
            key_of(&info(VK_HANJA, 0xF1, false)),
            Some((Key::Hanja, false))
        );
        assert_eq!(
            key_of(&info(VK_HANGUL, 0xF2, false)),
            Some((Key::HangulMode, false))
        );
    }

    #[test]
    fn modifiers_keep_their_side() {
        assert_eq!(
            key_of(&info(VK_RCONTROL, SCAN_CTRL, true)),
            Some((Key::RightCtrl, false))
        );
        assert_eq!(
            key_of(&info(VK_CONTROL, SCAN_CTRL, true)),
            Some((Key::RightCtrl, false))
        );
        assert_eq!(
            key_of(&info(VK_SHIFT, SCAN_RIGHT_SHIFT, false)),
            Some((Key::RightShift, false))
        );
        assert_eq!(key_of(&info(VK_LCONTROL, SCAN_ALTGR_CTRL, false)), None);
    }

    #[test]
    fn other_keys_map_to_names() {
        assert_eq!(
            key_of(&info(0x41, 0x1E, false)),
            Some((Key::Letter(b'A'), false))
        );
        assert_eq!(
            key_of(&info(0x37, 0x08, false)),
            Some((Key::Digit(7), false))
        );
        assert_eq!(
            key_of(&info(VK_F1 + 12, 0x64, false)),
            Some((Key::Function(13), false))
        );
        assert_eq!(
            key_of(&info(0xFF, 0, false)),
            Some((Key::Other(0xFF), false))
        );
    }
}
