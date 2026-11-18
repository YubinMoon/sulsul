//! 술술 데스크톱 앱.
//!
//! 녹음(M0)과 음성 인식(M1)을 확인하는 화면, TOML 설정 파일과 연동되는 설정 화면,
//! 그리고 전역 단축키로 받아쓴 글을 클립보드에 넣는 단축키 받아쓰기(M2)와 트레이가 있습니다.
//! 글 다듬기를 켜면 받아쓴 글을 고른 프롬프트와 모델(로컬 모델은 `sulsul-llm-runner` 실행기,
//! 또는 외부 제공자)로 다듬은 뒤 클립보드에 넣습니다.
//! 단축키 받아쓰기가 녹음하고 인식하는 동안에는 화면 아래 가운데에 물방울 오버레이가 뜹니다.
//! 커서 위치에 바로 입력하기는 아직 없습니다.
//!
//! 명령줄 인자: `--background`(창 없이 트레이로 시작, 로그인할 때 자동 실행이 씀),
//! `--home <폴더>`(설정·데이터 폴더, `SULSUL_HOME`과 같음).

// 릴리스 빌드에서는 콘솔 창을 띄우지 않습니다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accel;
mod app;
mod autostart;
mod clipboard;
mod clock;
mod config_sync;
mod desktop;
mod dictation;
mod engines;
mod error_text;
mod fonts;
mod format;
mod history;
mod hotkey;
mod i18n;
mod icons;
mod instance;
mod overlay;
mod pages;
mod refiner;
mod session;
mod sound;
mod system;
mod theme;
mod transcriber;
mod tray;
mod widgets;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use rust_i18n::t;
use sulsul_core::paths::HOME_ENV;
use sulsul_core::{AppPaths, ConfigStore};

use crate::app::SulsulApp;
use crate::autostart::LaunchArgs;
use crate::config_sync::ConfigSync;
use crate::desktop::{Launch, WindowRequests};
use crate::instance::Instance;

// 번역 파일은 저장소 루트의 `locales/`에 있습니다. 번역이 없는 문구는 한국어로 보입니다.
rust_i18n::i18n!("../../locales", fallback = "ko");

/// 창의 처음 크기와 최소 크기 (논리 픽셀)
const WINDOW_SIZE: [f32; 2] = [960.0, 660.0];
const WINDOW_MIN_SIZE: [f32; 2] = [780.0, 540.0];

fn main() {
    init_logging();
    if let Err(error) = run() {
        tracing::error!("{error:#}");
        // 릴리스 빌드에는 콘솔이 없으므로 대화상자로 알립니다.
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(t!("app.name"))
            .set_description(t!("app.fatal", message = format!("{error:#}")))
            .show();
        std::process::exit(1);
    }
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "warn,sulsul=info".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

fn run() -> anyhow::Result<()> {
    let args = LaunchArgs::parse(std::env::args_os().skip(1));
    // 설정·데이터 폴더: `--home`, `SULSUL_HOME`, OS 기본 위치 순서입니다.
    // 자동 실행이 같은 폴더로 시작하도록 절대 경로로 바꿔 둡니다.
    let home = args
        .home
        .clone()
        .or_else(|| {
            std::env::var_os(HOME_ENV)
                .filter(|home| !home.is_empty())
                .map(PathBuf::from)
        })
        .map(|home| std::path::absolute(&home).unwrap_or(home));
    let paths = match &home {
        Some(home) => AppPaths::under(home),
        None => AppPaths::resolve().context("could not determine the app directories")?,
    };

    // 같은 폴더를 쓰는 술술이 이미 떠 있으면 그 창을 보여 달라고 하고 끝냅니다.
    let requests = Arc::new(WindowRequests::default());
    let _instance = match instance::acquire(&paths.config_dir, !args.background) {
        Instance::Secondary => return Ok(()),
        Instance::Primary(primary) => {
            let requests = Arc::clone(&requests);
            primary.listen(move || requests.request_show());
            primary
        }
    };

    let sync = ConfigSync::open(ConfigStore::new(paths.config_file()));
    i18n::apply_locale(&sync.config.general.ui_language);
    let registry = engines::registry();
    tracing::info!(
        config = %paths.config_file().display(),
        data = %paths.data_dir.display(),
        engines = ?registry,
        background = args.background,
        "starting sulsul"
    );

    let key_capture = hotkey::KeyCapture::default();
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(t!("app.name"))
            .with_app_id("sulsul")
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(WINDOW_MIN_SIZE),
        // 로그인할 때 자동 실행(`--background`)이면 창 없이 트레이로 시작합니다. eframe은 첫 프레임을 그린 뒤
        // 창을 늘 보이게 하므로 `with_visible(false)`로는 안 되고, `Desktop`이 그 뒤에 숨깁니다.
        ..Default::default()
    };
    #[cfg(windows)]
    hotkey::install_window_capture(&mut options, key_capture.clone());
    let launch = Launch {
        background: args.background,
        home,
        requests,
    };
    eframe::run_native(
        "sulsul",
        options,
        Box::new(move |cc| {
            Ok(Box::new(SulsulApp::new(
                cc,
                paths,
                registry,
                sync,
                launch,
                key_capture,
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!("could not run the window: {error}"))
}
