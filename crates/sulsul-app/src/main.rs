//! 술술 데스크톱 앱.
//!
//! 지금은 녹음(M0)과 음성 인식(M1)을 확인하는 화면, 그리고 TOML 설정 파일과
//! 연동되는 설정 화면이 있습니다. 전역 단축키·트레이·커서 입력은 아직 없습니다.

// 릴리스 빌드에서는 콘솔 창을 띄우지 않습니다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config_sync;
mod engines;
mod error_text;
mod fonts;
mod format;
mod i18n;
mod icons;
mod pages;
mod session;
mod system;
mod theme;
mod widgets;

use anyhow::Context as _;
use rust_i18n::t;
use sulsul_core::{AppPaths, ConfigStore};

use crate::app::SulsulApp;
use crate::config_sync::ConfigSync;

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
    let paths = AppPaths::resolve().context("could not determine the app directories")?;
    let sync = ConfigSync::open(ConfigStore::new(paths.config_file()));
    i18n::apply_locale(&sync.config.general.ui_language);
    let registry = engines::registry();
    tracing::info!(
        config = %paths.config_file().display(),
        data = %paths.data_dir.display(),
        engines = ?registry,
        "starting sulsul"
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(t!("app.name"))
            .with_app_id("sulsul")
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(WINDOW_MIN_SIZE),
        ..Default::default()
    };
    eframe::run_native(
        "sulsul",
        options,
        Box::new(move |cc| Ok(Box::new(SulsulApp::new(cc, paths, registry, sync)))),
    )
    .map_err(|error| anyhow::anyhow!("could not run the window: {error}"))
}
