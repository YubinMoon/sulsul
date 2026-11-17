//! 데스크톱 연동: 전역 단축키, 단축키 받아쓰기, 트레이, 녹음 중 오버레이, 로그인할 때 자동 실행, 창 보이기·숨기기.
//!
//! 앱이 시작할 때 [`Desktop::start`]로 만들고 매 프레임 `logic`에서 [`Desktop::tick`]을 부릅니다.
//! 창이 트레이에 숨겨져 있어도 eframe은 다시 그려 달라는 요청이 오면 `logic`을 부르므로 여기 일은 계속 동작합니다.
//! 녹음·인식·클립보드는 받아쓰기 스레드([`Dictation`])가, 키 읽기는 후크 스레드([`GlobalHotkey`])가 맡고,
//! 여기서는 설정 값을 그쪽에 넘기고 상태·알림을 화면·트레이·오버레이에 옮깁니다.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use egui::ViewportCommand;
use rust_i18n::t;
use sulsul_core::config::{Config, HotkeyMode};

use crate::autostart::{self, Entry, SystemLoginItems};
use crate::dictation::{Dictation, DictationSettings, Notice};
use crate::error_text;
use crate::hotkey::{Binding, GlobalHotkey, HotkeySpec};
use crate::overlay::Overlay;
use crate::pages::Page;
use crate::session::{DictationState, DictationStatus, Shared};
use crate::tray::{Tray, TrayCommand};

/// 다른 스레드(다른 실행의 알림 등)가 창을 보여 달라고 하는 창구
#[derive(Default)]
pub struct WindowRequests {
    show: AtomicBool,
    ctx: OnceLock<egui::Context>,
}

impl WindowRequests {
    pub fn request_show(&self) {
        self.show.store(true, Ordering::SeqCst);
        if let Some(ctx) = self.ctx.get() {
            ctx.request_repaint();
        }
    }

    fn attach(&self, ctx: &egui::Context) {
        let _ = self.ctx.set(ctx.clone());
    }

    fn take_show(&self) -> bool {
        self.show.swap(false, Ordering::SeqCst)
    }
}

/// 실행할 때 정해진 것
pub struct Launch {
    /// 창 없이 트레이로 시작했는지 (로그인할 때 자동 실행)
    pub background: bool,
    /// 명령줄(`--home`)이나 `SULSUL_HOME`으로 정한 설정·데이터 폴더
    pub home: Option<PathBuf>,
    pub requests: Arc<WindowRequests>,
}

pub struct Desktop {
    dictation: Option<Dictation>,
    hotkey: Option<GlobalHotkey>,
    /// 전역 단축키를 시작하지 못한 이유 (화면 문구)
    hotkey_unavailable: Option<String>,
    tray: Option<Tray>,
    /// 녹음하는 동안 화면에 띄우는 물방울
    overlay: Option<Overlay>,
    /// 받아쓰기 스레드에 넘긴 설정
    applied_settings: Option<DictationSettings>,
    /// 후크에 넘긴 단축키 값과 방식
    applied_binding: Option<(String, HotkeyMode)>,
    /// 단축키 값을 읽지 못했으면 그 값
    invalid_key: Option<String>,
    /// 자동 실행 등록에 쓸 항목과 마지막으로 맞춘 값
    autostart_entry: Option<Entry>,
    applied_autostart: Option<bool>,
    requests: Arc<WindowRequests>,
    hidden: bool,
    /// 트레이로 시작할 때 첫 프레임이 지날 때까지 창을 가려 둔 것
    start_hidden: Option<StartHidden>,
    quitting: bool,
}

impl Desktop {
    pub fn start(
        cc: &eframe::CreationContext<'_>,
        shared: &mut Shared,
        config: &Config,
        launch: Launch,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        launch.requests.attach(ctx);

        let wake_ctx = ctx.clone();
        let dictation = match Dictation::start(
            DictationSettings::from_config(config),
            shared.transcriber.clone(),
            shared.refiner.clone(),
            move || wake_ctx.request_repaint(),
        ) {
            Ok(dictation) => Some(dictation),
            Err(error) => {
                tracing::error!("could not start the dictation thread: {error}");
                None
            }
        };

        let (hotkey, hotkey_unavailable) = match &dictation {
            Some(dictation) => {
                match GlobalHotkey::start(shared.key_capture.clone(), dictation.hotkey_sink()) {
                    Ok(hotkey) => (Some(hotkey), None),
                    Err(error) => {
                        tracing::warn!("global hotkey unavailable: {error}");
                        (None, Some(error_text::hotkey(&error)))
                    }
                }
            }
            None => (None, Some(t!("transcribe.error.crashed").into_owned())),
        };

        let tray_ctx = ctx.clone();
        let tray = match Tray::new(move || tray_ctx.request_repaint()) {
            Ok(tray) => Some(tray),
            Err(error) => {
                tracing::warn!("tray icon unavailable: {error}");
                None
            }
        };

        let overlay = match Overlay::start(cc.wgpu_render_state.as_ref()) {
            Ok(overlay) => Some(overlay),
            Err(error) => {
                tracing::warn!("recording overlay unavailable: {error}");
                None
            }
        };

        let autostart_entry = match Entry::current(launch.home.as_deref()) {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::warn!("could not find the executable for launch at login: {error}");
                None
            }
        };

        Self {
            dictation,
            hotkey,
            hotkey_unavailable,
            tray,
            overlay,
            applied_settings: Some(DictationSettings::from_config(config)),
            applied_binding: None,
            invalid_key: None,
            autostart_entry,
            applied_autostart: None,
            requests: launch.requests,
            hidden: false,
            start_hidden: None,
            quitting: false,
        }
        .with_initial_visibility(cc, launch.background)
    }

    /// 트레이로 시작하면 창을 숨깁니다. 트레이가 없으면 창을 다시 열 방법이 없으므로 그대로 보입니다.
    fn with_initial_visibility(
        mut self,
        cc: &eframe::CreationContext<'_>,
        background: bool,
    ) -> Self {
        if background && self.tray.is_some() {
            tracing::info!("starting hidden in the tray");
            self.hidden = true;
            self.start_hidden = Some(StartHidden::begin(cc));
        }
        self
    }

    /// 매 프레임 부릅니다(창이 숨겨져 있어도). `locked`는 설정 파일을 읽지 못해 편집이 잠겼는지입니다.
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        shared: &mut Shared,
        config: &Config,
        locked: bool,
        page: Page,
    ) {
        self.apply_settings(config);
        self.apply_binding(config);
        self.collect_status(shared, config);
        self.handle_window(ctx, shared, page);
        if !locked {
            self.apply_autostart(shared, config.general.launch_at_startup);
        }
        shared.transcriber.release_idle(&config.stt, ctx);
        shared.refiner.release_idle(ctx);
    }

    /// 앱을 끝낼 때 부릅니다. 붙여넣지 않은 받아쓰기 글이 있으면 클립보드를 되돌리고, 후크·오버레이·트레이를 뗍니다.
    pub fn shutdown(&mut self) {
        self.hotkey = None;
        self.overlay = None;
        if let Some(mut dictation) = self.dictation.take() {
            dictation.shutdown();
        }
        self.tray = None;
    }

    fn apply_settings(&mut self, config: &Config) {
        let Some(dictation) = &self.dictation else {
            return;
        };
        if self
            .applied_settings
            .as_ref()
            .is_some_and(|applied| applied.matches(config))
        {
            return;
        }
        let settings = DictationSettings::from_config(config);
        dictation.update_settings(settings.clone());
        self.applied_settings = Some(settings);
    }

    fn apply_binding(&mut self, config: &Config) {
        let Some(hotkey) = &self.hotkey else {
            return;
        };
        let wanted = (config.hotkey.key.clone(), config.hotkey.mode);
        if self.applied_binding.as_ref() == Some(&wanted) {
            return;
        }
        match HotkeySpec::parse(&config.hotkey.key) {
            Ok(spec) => {
                tracing::info!(hotkey = %spec, mode = ?config.hotkey.mode, "hotkey set");
                hotkey.set_binding(Some(Binding {
                    spec,
                    mode: config.hotkey.mode,
                }));
                self.invalid_key = None;
            }
            Err(error) => {
                tracing::warn!(key = %config.hotkey.key, "invalid hotkey: {error}");
                hotkey.set_binding(None);
                self.invalid_key = Some(config.hotkey.key.clone());
            }
        }
        self.applied_binding = Some(wanted);
    }

    /// 받아쓰기 상태와 알림을 화면·트레이·오버레이로 옮깁니다.
    fn collect_status(&mut self, shared: &mut Shared, config: &Config) {
        let state = self
            .dictation
            .as_ref()
            .map_or(DictationState::Idle, Dictation::state);
        let hotkey_error = self.hotkey_unavailable.clone().or_else(|| {
            self.invalid_key
                .as_ref()
                .map(|key| t!("dictation.invalid_hotkey", key = key).into_owned())
        });
        let status = DictationStatus {
            state,
            hotkey_error,
        };
        if status.hotkey_error != shared.dictation.hotkey_error
            && let Some(error) = &status.hotkey_error
        {
            // 설정 파일을 직접 고쳤거나 창이 숨겨져 있던 경우에도 알 수 있게 한 번 알립니다.
            shared.toasts.warning(error.clone());
        }
        shared.dictation = status;

        if let Some(dictation) = &self.dictation {
            for notice in dictation.take_notices() {
                show_notice(shared, config, notice);
            }
        }
        if let Some(tray) = &mut self.tray {
            tray.update(shared.dictation.state);
        }
        if let Some(overlay) = &mut self.overlay {
            overlay.update(shared.dictation.state);
        }
    }

    fn handle_window(&mut self, ctx: &egui::Context, shared: &Shared, page: Page) {
        // eframe은 첫 프레임을 그린 뒤 창을 보이게 하고, 그다음에 이 프레임의 명령을 처리합니다.
        // 그래서 첫 프레임이 끝날 때까지 숨기라고 보내고, 그동안 창이 화면에 비치지 않게 가려 둡니다.
        if self.start_hidden.is_some() {
            if ctx.cumulative_frame_nr() == 0 {
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                ctx.request_repaint();
            } else if let Some(start_hidden) = self.start_hidden.take() {
                start_hidden.end();
            }
        }

        let commands = self.tray.as_ref().map(Tray::commands).unwrap_or_default();
        for command in commands {
            match command {
                TrayCommand::Open => self.show(ctx),
                TrayCommand::Quit => {
                    tracing::info!("quit from the tray");
                    self.quitting = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
        if self.requests.take_show() {
            self.show(ctx);
        }

        // 창의 닫기(X)는 앱을 끝내지 않고 트레이로 숨깁니다. 트레이가 없으면 그대로 끝냅니다.
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.quitting && self.tray.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.hide(ctx);
        }

        // 단축키 입력받기는 설정 화면에서 창이 앞에 있을 때만 합니다. 창이 숨겨지거나 다른 화면으로 가면
        // 화면(입력란)이 그려지지 않아 스스로 그만둘 수 없으므로 여기서 그만두게 합니다.
        // 후크가 없으면 결과를 줄 곳이 없으니, 입력받는 상태로 남지 않게 바로 그만둡니다.
        if shared.key_capture.is_active() {
            let focused = ctx.input(|input| input.viewport().focused).unwrap_or(true);
            if self.hotkey.is_none() || self.hidden || !focused || page != Page::Settings {
                shared.key_capture.cancel();
            }
        }
    }

    fn show(&mut self, ctx: &egui::Context) {
        self.hidden = false;
        if let Some(start_hidden) = self.start_hidden.take() {
            start_hidden.end();
        }
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn hide(&mut self, ctx: &egui::Context) {
        tracing::info!("window hidden to the tray");
        self.hidden = true;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }

    fn apply_autostart(&mut self, shared: &mut Shared, enabled: bool) {
        if self.applied_autostart == Some(enabled) {
            return;
        }
        let first = self.applied_autostart.is_none();
        // 실패해도 매 프레임 다시 시도하지 않습니다. 값을 다시 바꾸면 다시 시도합니다.
        self.applied_autostart = Some(enabled);
        let Some(entry) = &self.autostart_entry else {
            return;
        };
        match autostart::apply(&mut SystemLoginItems, entry, enabled) {
            Ok(_) => {}
            // 지원하지 않는 OS에서 꺼 둔 값으로 시작할 때는 알릴 것이 없습니다.
            Err(autostart::AutostartError::Unsupported) if first && !enabled => {}
            Err(error) => {
                tracing::warn!("{error}");
                shared.toasts.error(error_text::autostart(&error));
            }
        }
    }
}

/// 첫 프레임 동안 창 가리기. Windows는 DWM으로 창을 가려 eframe이 잠깐 보이게 해도 화면에 비치지 않습니다.
/// 다른 OS는 가리지 못해 첫 프레임에 창이 잠깐 보일 수 있습니다.
struct StartHidden {
    #[cfg(windows)]
    hwnd: Option<windows_sys::Win32::Foundation::HWND>,
}

impl StartHidden {
    fn begin(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
            let hwnd = match cc.window_handle().map(|handle| handle.as_raw()) {
                Ok(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get() as _),
                _ => None,
            };
            if let Some(hwnd) = hwnd {
                set_cloaked(hwnd, true);
            }
            Self { hwnd }
        }
        #[cfg(not(windows))]
        {
            let _ = cc;
            Self {}
        }
    }

    fn end(self) {
        #[cfg(windows)]
        if let Some(hwnd) = self.hwnd {
            set_cloaked(hwnd, false);
        }
    }
}

#[cfg(windows)]
fn set_cloaked(hwnd: windows_sys::Win32::Foundation::HWND, cloaked: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
    let value = i32::from(cloaked);
    // SAFETY: eframe이 만든 창의 핸들이고, DWMWA_CLOAK은 BOOL 하나를 받습니다.
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK as u32,
            (&raw const value).cast(),
            size_of::<i32>() as u32,
        )
    };
    if result < 0 {
        tracing::warn!("could not cloak the window: HRESULT {result:#x}");
    }
}

impl DictationSettings {
    /// 지금 설정과 같은 값인지 (매 프레임 복사하지 않으려고 그대로 비교합니다)
    pub fn matches(&self, config: &Config) -> bool {
        self.input_device == config.audio.input_device
            && self.max_record_seconds == config.audio.max_record_seconds
            && self.feedback_sounds == config.audio.feedback_sounds
            && self.stt == config.stt
            && self.postprocess == config.postprocess
    }
}

fn show_notice(shared: &mut Shared, config: &Config, notice: Notice) {
    match notice {
        Notice::Copied { refined: false } => shared.toasts.info(t!("dictation.copied")),
        Notice::Copied { refined: true } => shared.toasts.info(t!("dictation.refined_copied")),
        Notice::RefineFailed(error) => {
            let message = error_text::refine(&error, &shared.paths, &config.postprocess);
            shared
                .toasts
                .error(t!("dictation.refine_failed", message = message));
        }
        Notice::Empty => shared.toasts.warning(t!("dictation.empty")),
        Notice::Busy => shared.toasts.warning(t!("dictation.busy")),
        Notice::RecordFailed(error) => shared.toasts.error(t!(
            "dictation.record_failed",
            message = error_text::audio(&error)
        )),
        Notice::Failed(error) => {
            let message = error_text::dictation(&error, &shared.registry);
            shared
                .toasts
                .error(t!("dictation.failed", message = message));
        }
        Notice::ClipboardFailed(error) => shared.toasts.error(t!(
            "dictation.clipboard_failed",
            message = error_text::clipboard(&error)
        )),
    }
}
