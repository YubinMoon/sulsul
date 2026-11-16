//! Windows 오버레이 창: 맨 위에 뜨고, 클릭이 통과하고, 초점을 가져가지 않는 투명 계층 창(layered window).
//!
//! 창은 보이는 동안만 있습니다(녹음을 시작할 때 만들고, 사라지는 동작이 끝나면 없앱니다). 창이 있는 동안
//! 이 스레드는 쉬지 않고 메시지를 꺼내므로, 다른 프로그램이 모든 창에 보내는 메시지를 붙잡아 두지 않습니다.
//! 그림은 화면 갱신(DWM)에 맞춰 그리고, `UpdateLayeredWindow`로 픽셀마다 투명도를 넣어 올립니다.
//! 창은 초점이 있는 창(받아쓴 글이 들어갈 곳)의 모니터에 띄웁니다.

use std::ffi::c_void;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetMonitorInfoW,
    HBITMAP, HDC, HGDIOBJ, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromWindow, SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    HTTRANSPARENT, MA_NOACTIVATE, MSG, PM_REMOVE, PeekMessageW, RegisterClassExW,
    SPI_GETCLIENTAREAANIMATION, SW_SHOWNOACTIVATE, ShowWindow, SystemParametersInfoW, ULW_ALPHA,
    UpdateLayeredWindow, WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::render::{DropletRenderer, Frame, RenderError};
use super::{Command, Placement, Timeline, WorkArea, place, to_premultiplied_bgra};

/// 화면 갱신이 빠른 모니터에서도 1초에 60~80번 정도만 그리도록 두는 최소 간격
const MIN_FRAME_GAP: Duration = Duration::from_millis(12);
/// 화면 갱신을 기다릴 수 없을 때나 멈춘 그림을 보일 때 쉬는 시간
const IDLE_WAIT: Duration = Duration::from_millis(16);
const CLASS_NAME: *const u16 = windows_sys::w!("SulsulOverlay");

/// 오버레이 스레드 본체. 명령 채널이 닫히거나 `Shutdown`을 받으면 끝납니다.
pub fn run(device: wgpu::Device, queue: wgpu::Queue, commands: Receiver<Command>) {
    let started = Instant::now();
    let mut renderer = match DropletRenderer::new(device, queue) {
        Ok(renderer) => renderer,
        Err(error) => {
            tracing::warn!("recording overlay unavailable: {error}");
            return;
        }
    };
    if let Err(error) = register_class() {
        tracing::warn!("recording overlay unavailable: {error}");
        return;
    }
    tracing::info!(
        seconds = started.elapsed().as_secs_f32(),
        "recording overlay ready"
    );

    let mut shown: Option<Shown> = None;
    loop {
        // 보이는 동안에는 기다리지 않고 명령만 확인합니다.
        let command = if shown.is_some() {
            match commands.try_recv() {
                Ok(command) => Some(command),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => break,
            }
        } else {
            match commands.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        match command {
            Some(Command::Start) => {
                // 앞의 창이 아직 사라지는 중이면 닫고, 지금 초점이 있는 모니터에 새로 엽니다.
                shown = None;
                match Shown::open() {
                    Ok(window) => shown = Some(window),
                    Err(error) => tracing::warn!("could not open the recording overlay: {error}"),
                }
            }
            Some(Command::Pop) => {
                if let Some(window) = &mut shown {
                    window.timeline.pop(Instant::now());
                    window.drawn = None;
                }
            }
            Some(Command::Finish) => {
                if let Some(window) = &mut shown {
                    window.timeline.finish(Instant::now());
                    window.drawn = None;
                }
            }
            Some(Command::Shutdown) => break,
            None => {}
        }
        let Some(window) = &mut shown else {
            continue;
        };
        pump_messages();
        match window.frame(&mut renderer) {
            Ok(true) => window.wait(),
            Ok(false) => shown = None,
            Err(error) => {
                tracing::warn!("could not draw the recording overlay: {error}");
                shown = None;
            }
        }
    }
}

/// 보이는 동안의 창과 재생 상태
struct Shown {
    window: LayeredWindow,
    timeline: Timeline,
    /// 운영체제에서 애니메이션을 껐으면 단계마다 멈춘 모습만 보입니다.
    still: bool,
    /// 마지막으로 그린 시각. 멈춘 모습은 단계가 바뀔 때(`None`으로 지울 때)만 다시 그립니다.
    drawn: Option<Instant>,
}

impl Shown {
    fn open() -> Result<Self, String> {
        let placement = current_placement();
        let window = LayeredWindow::open(placement)?;
        Ok(Self {
            window,
            timeline: Timeline::new(Instant::now()),
            still: !animations_enabled(),
            drawn: None,
        })
    }

    /// 한 장을 그립니다. 사라지는 동작까지 다 끝나 창을 닫을 때가 되면 `false`를 돌려줍니다.
    fn frame(&mut self, renderer: &mut DropletRenderer) -> Result<bool, FrameError> {
        let now = Instant::now();
        let Some(moment) = self.timeline.moment(now, self.still) else {
            return Ok(false);
        };
        match self.drawn {
            Some(_) if self.still => return Ok(true),
            Some(at) if now.duration_since(at) < MIN_FRAME_GAP => return Ok(true),
            _ => {}
        }
        let placement = self.window.placement;
        let frame = Frame {
            width: placement.width,
            height: placement.height,
            stage_height: placement.stage_height,
            time: moment.time,
            pop: moment.pop,
            done: moment.done,
        };
        let stride = placement.width as usize * 4;
        let pixels = self.window.pixels();
        renderer.render(&frame, |y, row| {
            to_premultiplied_bgra(row, &mut pixels[y * stride..(y + 1) * stride]);
        })?;
        self.window.present().map_err(FrameError::Window)?;
        self.drawn = Some(now);
        Ok(true)
    }

    /// 다음 화면 갱신까지 기다립니다.
    fn wait(&self) {
        // SAFETY: 인자가 없는 함수입니다.
        if self.still || unsafe { DwmFlush() } < 0 {
            std::thread::sleep(IDLE_WAIT);
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum FrameError {
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error("{0}")]
    Window(String),
}

/// 픽셀마다 투명도를 넣어 그리는 창과 그 그림을 담는 메모리
struct LayeredWindow {
    hwnd: HWND,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    placement: Placement,
    visible: bool,
}

impl LayeredWindow {
    fn open(placement: Placement) -> Result<Self, String> {
        let width = placement.width.max(1) as i32;
        let height = placement.height.max(1) as i32;
        // SAFETY: 이 실행 파일 자신의 모듈 핸들을 얻습니다.
        let module = unsafe { GetModuleHandleW(std::ptr::null()) };
        // SAFETY: 클래스는 이 스레드에서 등록했고, 이름 문자열은 정적 수명입니다.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
                CLASS_NAME,
                std::ptr::null(),
                WS_POPUP,
                placement.x,
                placement.y,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                module,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(format!("could not create the window: {}", last_error()));
        }
        // SAFETY: 화면과 같은 형식의 메모리 DC를 만듭니다.
        let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        if dc.is_null() {
            // SAFETY: 방금 이 스레드에서 만든 창입니다.
            unsafe { DestroyWindow(hwnd) };
            return Err("could not create a memory DC".into());
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // 음수면 위에서부터 줄이 놓입니다.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: `info`는 32비트 BGRA 비트맵을 설명하고, `bits`는 픽셀 메모리 주소를 받습니다.
        let bitmap = unsafe {
            CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        if bitmap.is_null() || bits.is_null() {
            // SAFETY: 위에서 만든 DC와 창입니다.
            unsafe {
                DeleteDC(dc);
                DestroyWindow(hwnd);
            }
            return Err("could not create the overlay bitmap".into());
        }
        // SAFETY: 위에서 만든 DC와 비트맵입니다.
        let previous = unsafe { SelectObject(dc, bitmap) };
        Ok(Self {
            hwnd,
            dc,
            bitmap,
            previous,
            bits: bits.cast(),
            placement: Placement {
                width: width as u32,
                height: height as u32,
                ..placement
            },
            visible: false,
        })
    }

    /// 위에서부터 줄이 놓인 BGRA(알파를 곱한 값) 픽셀
    fn pixels(&mut self) -> &mut [u8] {
        let len = self.placement.width as usize * self.placement.height as usize * 4;
        // SAFETY: `bits`는 `CreateDIBSection`이 만든 가로×세로×4바이트 메모리이고, 비트맵을 지울 때까지 유효합니다.
        // `&mut self`로만 빌려 주므로 겹쳐 쓰지 않습니다.
        unsafe { std::slice::from_raw_parts_mut(self.bits, len) }
    }

    /// 픽셀을 창에 올립니다. 처음 올릴 때 창을 보입니다(초점은 가져가지 않음).
    fn present(&mut self) -> Result<(), String> {
        let position = POINT {
            x: self.placement.x,
            y: self.placement.y,
        };
        let size = SIZE {
            cx: self.placement.width as i32,
            cy: self.placement.height as i32,
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: 이 스레드의 창과, 그 창 크기의 비트맵을 고른 메모리 DC입니다.
        let updated = unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                std::ptr::null_mut(),
                &position,
                &size,
                self.dc,
                &source,
                0,
                &blend,
                ULW_ALPHA,
            )
        };
        if updated == 0 {
            return Err(format!("could not update the window: {}", last_error()));
        }
        if !self.visible {
            // SAFETY: 이 스레드의 창입니다.
            unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
            self.visible = true;
        }
        Ok(())
    }
}

impl Drop for LayeredWindow {
    fn drop(&mut self) {
        // SAFETY: 모두 이 스레드에서 만든 것이고 한 번만 지웁니다.
        unsafe {
            DestroyWindow(self.hwnd);
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

fn register_class() -> Result<(), String> {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        // SAFETY: 이 실행 파일 자신의 모듈 핸들을 얻습니다.
        hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `class`는 모든 필드가 채워진 구조체이고, 창 함수는 이 모듈의 `extern "system"` 함수입니다.
    if unsafe { RegisterClassExW(&class) } == 0 {
        // SAFETY: 인자가 없는 함수입니다.
        let code = unsafe { GetLastError() };
        if code != ERROR_CLASS_ALREADY_EXISTS {
            return Err(format!(
                "could not register the window class: {}",
                std::io::Error::from_raw_os_error(code as i32)
            ));
        }
    }
    Ok(())
}

/// 마우스가 창을 지나 아래 창으로 가게 하고, 눌러도 창이 활성화되지 않게 합니다.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        // SAFETY: 받은 인자를 그대로 넘깁니다.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// 이 스레드에 온 메시지를 모두 처리합니다.
fn pump_messages() {
    let mut message = MSG::default();
    // SAFETY: `message`는 유효한 MSG이고, 꺼낸 메시지를 바로 창 함수로 보냅니다.
    unsafe {
        while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            DispatchMessageW(&message);
        }
    }
}

/// 초점이 있는 창의 모니터 작업 영역 아래 가운데
fn current_placement() -> Placement {
    // SAFETY: 인자가 없는 함수입니다. 초점이 있는 창이 없으면 주 모니터를 씁니다.
    let monitor = unsafe { MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info`의 크기를 적어 넘깁니다.
    let work = if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        WorkArea {
            left: info.rcWork.left,
            top: info.rcWork.top,
            right: info.rcWork.right,
            bottom: info.rcWork.bottom,
        }
    } else {
        tracing::debug!("could not read the monitor work area: {}", last_error());
        WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    };
    let (mut dpi_x, mut dpi_y) = (96, 96);
    // SAFETY: 위에서 얻은 모니터 핸들이고, 두 값을 받을 자리를 넘깁니다.
    if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } < 0 {
        dpi_y = 96;
    }
    place(work, dpi_y as f32 / 96.0)
}

/// 운영체제의 "애니메이션 효과" 설정. 읽지 못하면 켜진 것으로 봅니다.
fn animations_enabled() -> bool {
    let mut enabled: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION은 BOOL 하나를 채웁니다.
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut enabled).cast(), 0)
    };
    ok == 0 || enabled != 0
}

fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}
