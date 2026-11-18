//! 정보 화면: 버전, 파일 위치, 등록된 엔진.

use std::path::Path;

use egui::{RichText, Ui};
use rust_i18n::t;
use sulsul_core::stt::EngineKind;

use super::{SECTION_GAP, header};
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::session::{PageContext, Shared};
use crate::system;
use crate::theme::{self, Palette, size};
use crate::widgets::card::{Section, card};
use crate::widgets::row::{self, Row};
use crate::widgets::{controls, text};

pub fn ui(ui: &mut Ui, cx: &mut PageContext<'_>) {
    header(ui, &t!("about.title"), None);

    app_card(ui);
    ui.add_space(SECTION_GAP);
    paths_card(ui, cx.shared);
    ui.add_space(SECTION_GAP);
    engines_card(ui, cx.shared);
}

fn app_card(ui: &mut Ui) {
    card(ui, |ui| {
        let palette = Palette::of(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.label(
                icons::text(Icon::AudioLines)
                    .size(34.0)
                    .color(palette.accent_text),
            );
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(RichText::new(t!("app.name")).font(theme::semibold(size::TITLE)));
                    controls::badge(
                        ui,
                        &t!("about.version", version = env!("CARGO_PKG_VERSION")),
                    );
                    let build = if cfg!(debug_assertions) {
                        t!("about.build_debug")
                    } else {
                        t!("about.build_release")
                    };
                    controls::badge(ui, &build);
                });
                ui.label(text::muted(ui, t!("app.tagline")));
            });
        });
    });
}

fn paths_card(ui: &mut Ui, shared: &mut Shared) {
    Section::new(Icon::FolderOpen, &t!("about.paths.title")).show(ui, |ui| {
        let paths = shared.paths.clone();
        let rows: [(String, std::path::PathBuf, bool); 4] = [
            (
                t!("about.paths.config").into_owned(),
                paths.config_file(),
                false,
            ),
            (
                t!("about.paths.data").into_owned(),
                paths.data_dir.clone(),
                true,
            ),
            (
                t!("about.paths.models").into_owned(),
                paths.models_dir(),
                true,
            ),
            // 화면 알림에는 오류 원문을 싣지 않으므로, 원문을 볼 수 있는 로그 폴더를 엽니다.
            (t!("about.paths.logs").into_owned(), paths.logs_dir(), true),
        ];
        for (index, (label, path, is_dir)) in rows.iter().enumerate() {
            if index > 0 {
                row::separator(ui);
            }
            path_row(ui, shared, label, path, *is_dir);
        }
    });
}

fn path_row(ui: &mut Ui, shared: &mut Shared, label: &str, path: &Path, is_dir: bool) {
    const LABEL_WIDTH: f32 = 96.0;
    const BUTTON_WIDTH: f32 = 30.0;
    const GAP: f32 = 12.0;

    let top_down = egui::Layout::top_down(egui::Align::Min);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let offset = row::first_line_offset(ui);
        ui.allocate_ui_with_layout(egui::vec2(LABEL_WIDTH, 0.0), top_down, |ui| {
            ui.set_width(LABEL_WIDTH);
            ui.add_space(offset);
            ui.label(label);
        });
        let path_width = (ui.available_width() - BUTTON_WIDTH - GAP).max(80.0);
        ui.allocate_ui_with_layout(egui::vec2(path_width, 0.0), top_down, |ui| {
            ui.set_width(path_width);
            ui.add_space(offset);
            ui.add(text::path_label(ui, path));
        });
        let open_label = t!("about.paths.open", name = label);
        if controls::icon_button(ui, Icon::ExternalLink, &open_label).clicked() {
            let result = if is_dir {
                system::open_dir(path)
            } else {
                system::open_path(path)
            };
            if result.is_err() {
                shared.toasts.error(t!("common.open_failed"));
            }
        }
    });
}

fn engines_card(ui: &mut Ui, shared: &Shared) {
    Section::new(Icon::Cpu, &t!("about.engines.title")).show(ui, |ui| {
        let palette = Palette::of(ui);
        let mut first = true;
        for descriptor in shared.registry.descriptors() {
            if !first {
                row::separator(ui);
            }
            first = false;
            let name = tr(&descriptor.name_key);
            let description = tr(&descriptor.description_key);
            Row::new(&name).help(&description).show(ui, |ui| {
                let kind = match descriptor.kind {
                    EngineKind::Local => t!("about.engines.local"),
                    EngineKind::Cloud => t!("about.engines.cloud"),
                };
                controls::badge(ui, &kind);
                ui.label(text::mono(&descriptor.id).color(palette.text_muted));
            });
        }
        if first {
            ui.label(text::muted(ui, t!("about.engines.none")));
        }
    });
}
