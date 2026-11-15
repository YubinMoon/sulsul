//! Windows 전역 단축키: 저수준 키보드·마우스 후크.
//!
//! 후크는 전용 스레드에서 메시지 루프를 돌며 받습니다. 후크 함수가 오래 걸리면 Windows가 후크를
//! 말없이 떼어 버리므로, 후크 함수에서는 판단([`Machine`])과 채널 보내기만 합니다.
//! 판단에 따라 키를 막거나(1을 돌려줌) 다음 후크로 넘기고, 메뉴 막대·시작 메뉴가 열리지 않게
//! 쓰지 않는 키 코드(`0xE8`)를 가짜로 눌렀다 떼기도 합니다.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
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
    VK_OEM_PLUS, VK_PAUSE, VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT,
    VK_RWIN, VK_SCROLL, VK_SHIFT, VK_SNAPSHOT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL,
    WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_USER, WM_XBUTTONDOWN,
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

/// 후크 스레드
pub struct Hook {
    machine: Arc<Mutex<Machine>>,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl Hook {
    pub fn start(
        capture: KeyCapture,
        on_event: Box<dyn Fn(HotkeyEvent) + Send>,
    ) -> Result<Self, HotkeyError> {
        let machine = Arc::new(Mutex::new(Machine::default()));
        let (ready_tx, ready_rx) = mpsc::channel();
        let context = Context {
            machine: Arc::clone(&machine),
            capture,
            on_event,
        };
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
    on_event: Box<dyn Fn(HotkeyEvent) + Send>,
}

thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

fn run(context: Context, ready: mpsc::Sender<std::io::Result<u32>>) {
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
        };
        let capturing = self.capture.is_active();
        let verdict = lock(&self.machine).handle(input, capturing, &AsyncKeyState);

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
