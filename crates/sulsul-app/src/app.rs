//! 앱 상태와 프레임 루프.

use std::time::Duration;

use egui::{
    Align, Align2, Area, CentralPanel, CornerRadius, Frame, Key, Layout, Margin, Modifiers, Order,
    Panel, Response, RichText, Role, ScrollArea, Sense, Stroke, Ui, ViewportCommand, WidgetInfo,
    vec2,
};
use rust_i18n::t;
use sulsul_core::AppPaths;
use sulsul_core::config::{Config, Theme};
use sulsul_core::stt::SttRegistry;

use crate::config_sync::{ConfigSync, ProblemKind};
use crate::desktop::{Desktop, Launch};
use crate::error_text;
use crate::i18n;
use crate::icons::{self, Icon};
use crate::pages::models::ModelsPage;
use crate::pages::prompts::PromptsPage;
use crate::pages::record::RecordPage;
use crate::pages::settings::SettingsPage;
use crate::pages::transcribe::TranscribePage;
use crate::pages::{self, Page};
use crate::session::{PageContext, Shared};
use crate::system;
use crate::theme::{self, Palette, radius, size};
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::controls;

const SIDEBAR_WIDTH: f32 = 208.0;
/// 본문이 너무 넓어지지 않게 하는 최대 너비
const CONTENT_MAX_WIDTH: f32 = 780.0;

pub struct SulsulApp {
    shared: Shared,
    sync: ConfigSync,
    page: Page,
    record: RecordPage,
    transcribe: TranscribePage,
    models: ModelsPage,
    prompts: PromptsPage,
    settings: SettingsPage,
    /// 전역 단축키·단축키 받아쓰기·트레이·자동 실행
    desktop: Desktop,
    /// 지금 화면에 적용된 언어·테마 (설정이 바뀌었는지 비교용)
    applied_locale: String,
    applied_theme: Theme,
    /// 배너 버튼으로 요청한 동작. 다음 프레임 시작에 처리합니다.
    pending: Option<SyncAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncAction {
    Reload,
    /// 지금 화면 값으로 파일을 씁니다 (읽을 수 없는 파일 덮어쓰기, 저장 재시도).
    Save,
}

impl SulsulApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        paths: AppPaths,
        registry: SttRegistry,
        mut sync: ConfigSync,
        launch: Launch,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        crate::fonts::install(ctx);
        theme::install(ctx);
        theme::apply(ctx, sync.config.general.theme);
        sync.start_watching(ctx.clone());

        let mut shared = Shared::new(paths, registry);
        let desktop = Desktop::start(cc, &mut shared, &sync.config, launch);
        let mut app = Self {
            shared,
            desktop,
            applied_locale: sync.config.general.ui_language.clone(),
            applied_theme: sync.config.general.theme,
            sync,
            page: Page::Record,
            record: RecordPage::default(),
            transcribe: TranscribePage::default(),
            models: ModelsPage::default(),
            prompts: PromptsPage::default(),
            settings: SettingsPage::default(),
            pending: None,
        };
        // 설정 파일을 직접 열었을 때 바꿀 수 있는 항목이 보이도록 엔진 기본값을 채워 둡니다.
        if app.ensure_engine_defaults() {
            app.sync.save_now();
        }
        app
    }

    /// 선택된 엔진의 빠진 설정 키를 기본값으로 채웁니다. 무언가 추가했으면 `true`.
    fn ensure_engine_defaults(&mut self) -> bool {
        if self.sync.locked() {
            return false;
        }
        let stt = &mut self.sync.config.stt;
        let engine = stt.engine.clone();
        self.shared.registry.ensure_engine_defaults(stt, &engine)
    }

    /// 설정의 언어·테마가 바뀌었으면 화면에 적용합니다.
    fn apply_general(&mut self, ctx: &egui::Context) {
        let general = &self.sync.config.general;
        if self.applied_locale != general.ui_language {
            i18n::apply_locale(&general.ui_language);
            self.applied_locale.clone_from(&general.ui_language);
            ctx.send_viewport_cmd(ViewportCommand::Title(t!("app.name").into_owned()));
        }
        if self.applied_theme != general.theme {
            theme::apply(ctx, general.theme);
            self.applied_theme = general.theme;
        }
    }

    /// 설정이 파일 내용으로 바뀐 뒤에 부릅니다.
    fn on_config_replaced(&mut self, ctx: &egui::Context) {
        if self.ensure_engine_defaults() {
            self.sync.mark_dirty();
        }
        self.apply_general(ctx);
    }

    /// 화면에서 설정을 바꿨으면 저장을 예약합니다.
    fn commit(&mut self, ctx: &egui::Context, before: Config) {
        if self.sync.config == before {
            return;
        }
        if self.sync.locked() {
            // 파일을 읽지 못하는 동안에는 마지막으로 정상이던 값을 그대로 둡니다.
            self.sync.config = before;
            return;
        }
        if self.sync.config.stt.engine != before.stt.engine {
            self.ensure_engine_defaults();
        }
        self.sync.mark_dirty();
        self.apply_general(ctx);
        ctx.request_repaint();
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        Panel::left("sidebar")
            .exact_size(SIDEBAR_WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(palette.sidebar).inner_margin(Margin {
                left: 12,
                right: 12,
                top: 18,
                bottom: 14,
            }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                brand(ui);
                ui.add_space(18.0);

                for page in Page::ALL {
                    let dot = (page == Page::Record && self.record.is_recording())
                        || (page == Page::Models && self.models.is_busy());
                    let selected = self.page == page;
                    if nav_item(ui, page.icon(), &page.title(), selected, dot).clicked() {
                        self.page = page;
                    }
                }

                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(t!("about.version", version = env!("CARGO_PKG_VERSION")))
                                .size(size::CAPTION)
                                .color(palette.text_faint),
                        );
                    });
                    self.sync_status(ui);
                });
            });
    }

    /// 사이드바 아래쪽의 설정 파일 상태 표시
    fn sync_status(&self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        let (color, label) = if self.sync.locked() {
            (palette.danger, t!("status.config_broken"))
        } else if self.sync.save_error().is_some() {
            (palette.danger, t!("status.save_failed"))
        } else if self.sync.has_pending_save() {
            (palette.warning, t!("status.saving"))
        } else if !self.sync.issues().is_empty() {
            (
                palette.warning,
                t!("status.issues", count = self.sync.issues().len()),
            )
        } else {
            (palette.success, t!("status.synced"))
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            ui.add_space(8.0);
            let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.5, color);
            ui.label(
                RichText::new(label)
                    .size(size::CAPTION)
                    .color(palette.text_muted),
            );
        });
    }

    fn content(&mut self, ui: &mut Ui, frame: &eframe::Frame) {
        let palette = Palette::of(ui);
        CentralPanel::default()
            .frame(Frame::new().fill(palette.background))
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .id_salt(("page", self.page as u8))
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        scroll_with_keys(ui);
                        Frame::new()
                            .inner_margin(Margin {
                                left: 28,
                                right: 28,
                                top: 24,
                                bottom: 28,
                            })
                            .show(ui, |ui| {
                                ui.set_max_width(ui.available_width().min(CONTENT_MAX_WIDTH));
                                ui.spacing_mut().item_spacing.y = 6.0;
                                self.banners(ui);
                                self.page_ui(ui, frame);
                            });
                    });
            });
    }

    fn page_ui(&mut self, ui: &mut Ui, frame: &eframe::Frame) {
        let issues = self.sync.issues().to_vec();
        let mut cx = PageContext {
            shared: &mut self.shared,
            locked: self.sync.locked(),
            config: &mut self.sync.config,
            frame,
        };
        match self.page {
            Page::Record => self.record.ui(ui, &mut cx),
            Page::Transcribe => self.transcribe.ui(ui, &mut cx),
            Page::Models => self.models.ui(ui, &mut cx),
            Page::Prompts => self.prompts.ui(ui, &mut cx),
            Page::Settings => self.settings.ui(ui, &mut cx, &issues),
            Page::About => pages::about::ui(ui, &mut cx),
        }
    }

    /// 설정 파일 문제 안내. 어느 화면에서든 보입니다.
    fn banners(&mut self, ui: &mut Ui) {
        let mut action = None;
        let mut open_file = false;

        if let Some(problem) = self.sync.problem() {
            let note = match problem.kind {
                ProblemKind::Syntax => t!("config_banner.overwrite_note_syntax"),
                ProblemKind::Value => t!("config_banner.overwrite_note_value"),
                ProblemKind::Io => t!("config_banner.overwrite_note_io"),
            };
            Banner::new(Tone::Error)
                .title(&t!("config_banner.title"))
                .line(error_text::config(&problem.error))
                .line(t!("config_banner.locked"))
                .line(note)
                .show_with(ui, |ui| {
                    if controls::icon_text_button(ui, Icon::RefreshCw, t!("config_banner.reload"))
                        .clicked()
                    {
                        action = Some(SyncAction::Reload);
                    }
                    if controls::icon_text_button(ui, Icon::FilePen, t!("settings.file.open"))
                        .clicked()
                    {
                        open_file = true;
                    }
                    if controls::danger_button(ui, t!("config_banner.overwrite")).clicked() {
                        action = Some(SyncAction::Save);
                    }
                });
            ui.add_space(pages::SECTION_GAP);
        }

        if let Some(error) = self.sync.save_error() {
            Banner::new(Tone::Error)
                .title(&t!("config_banner.save_failed_title"))
                .line(error_text::config(error))
                .show_with(ui, |ui| {
                    if controls::icon_text_button(ui, Icon::RefreshCw, t!("common.retry")).clicked()
                    {
                        action = Some(SyncAction::Save);
                    }
                });
            ui.add_space(pages::SECTION_GAP);
        }

        if self.page == Page::Settings
            && let Some(error) = self.sync.watch_error()
        {
            Banner::new(Tone::Warning)
                .title(&t!("config_banner.watch_failed_title"))
                .line(error_text::config(error))
                .line(t!("config_banner.watch_failed_help"))
                .show_with(ui, |ui| {
                    if controls::icon_text_button(ui, Icon::RefreshCw, t!("config_banner.reload"))
                        .clicked()
                    {
                        action = Some(SyncAction::Reload);
                    }
                });
            ui.add_space(pages::SECTION_GAP);
        }

        if open_file && let Err(message) = system::open_path(self.sync.path()) {
            self.shared
                .toasts
                .error(t!("common.open_failed", message = message));
        }
        if action.is_some() {
            self.pending = action;
            ui.ctx().request_repaint();
        }
    }

    fn toasts(&mut self, ui: &Ui) {
        let palette = Palette::of(ui);
        let (toasts, next_expiry) = self.shared.toasts.visible();
        if let Some(after) = next_expiry {
            ui.ctx()
                .request_repaint_after(after + Duration::from_millis(20));
        }
        if toasts.is_empty() {
            return;
        }
        Area::new(egui::Id::unique("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0))
            .order(Order::Foreground)
            .interactable(false)
            .show(ui.ctx(), |ui| {
                ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
                    for toast in toasts.iter().rev() {
                        let (icon, color) = match toast.tone {
                            Tone::Error => (Icon::CircleAlert, palette.danger_text),
                            Tone::Warning => (Icon::TriangleAlert, palette.warning_text),
                            Tone::Info | Tone::Success => (Icon::CircleCheck, palette.success_text),
                        };
                        Frame::new()
                            .fill(palette.card)
                            .stroke(Stroke::new(1.0, palette.border))
                            .corner_radius(10)
                            .shadow(ui.visuals().popup_shadow)
                            .inner_margin(Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.set_max_width(420.0);
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    ui.label(icons::text(icon).color(color));
                                    ui.label(&toast.text);
                                });
                            });
                    }
                });
            });
    }
}

impl eframe::App for SulsulApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let replaced = match self.pending.take() {
            Some(SyncAction::Reload) => self.sync.reload(),
            Some(SyncAction::Save) => self.sync.save_now(),
            None => false,
        };
        if replaced | self.sync.poll() {
            self.on_config_replaced(ctx);
        }
        if let Some(remaining) = self.sync.save_due_in() {
            ctx.request_repaint_after(remaining + Duration::from_millis(10));
        }
        self.record.tick(&mut self.shared, ctx);
        self.transcribe.tick(&self.shared);
        self.models.tick(&mut self.shared, ctx);
        self.desktop.tick(
            ctx,
            &mut self.shared,
            &self.sync.config,
            self.sync.locked(),
            self.page,
        );
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(page) = self.shared.navigate.take() {
            self.page = page;
        }
        // 화면을 그리는 동안 설정이 바뀌었는지 비교해서 저장 여부를 정합니다.
        let before = self.sync.config.clone();

        self.sidebar(ui);
        self.content(ui, frame);
        self.toasts(ui);

        self.commit(&ctx, before);
    }

    fn on_exit(&mut self) {
        // 붙여넣지 않은 받아쓰기 글이 있으면 클립보드를 되돌리고, 키보드 후크와 트레이를 뗍니다.
        self.desktop.shutdown();
        // 저장을 기다리던 변경이 있으면 끝내기 전에 씁니다.
        if self.sync.has_pending_save() && !self.sync.locked() {
            self.sync.save_now();
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        Palette::for_dark_mode(visuals.dark_mode)
            .background
            .to_normalized_gamma_f32()
    }
}

/// Page Up/Down, Home/End 키로 본문을 넘깁니다. 글을 입력하는 중에는 건드리지 않습니다.
fn scroll_with_keys(ui: &Ui) {
    if ui.text_edit_focused() {
        return;
    }
    let page = ui.clip_rect().height() * 0.85;
    // 끝까지 가기에 충분히 큰 값 (스크롤 범위를 넘는 만큼은 무시됩니다)
    let all = 100_000.0;
    let delta = ui.input_mut(|input| {
        let mut pressed = |key| input.consume_key(Modifiers::NONE, key);
        if pressed(Key::PageDown) {
            -page
        } else if pressed(Key::PageUp) {
            page
        } else if pressed(Key::End) {
            -all
        } else if pressed(Key::Home) {
            all
        } else {
            0.0
        }
    });
    if delta != 0.0 {
        ui.scroll_with_delta(vec2(0.0, delta));
    }
}

/// 사이드바 맨 위의 앱 이름
fn brand(ui: &mut Ui) {
    let palette = Palette::of(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.add_space(6.0);
        let (mark, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
        ui.painter()
            .rect_filled(mark, CornerRadius::same(9), palette.accent);
        ui.painter().text(
            mark.center(),
            Align2::CENTER_CENTER,
            Icon::AudioLines.unicode(),
            icons::font(18.0),
            palette.on_accent,
        );
        ui.label(RichText::new(t!("app.name")).font(theme::semibold(19.0)));
    });
}

/// 사이드바의 화면 이동 항목
fn nav_item(ui: &mut Ui, icon: Icon, label: &str, selected: bool, dot: bool) -> Response {
    let palette = Palette::of(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.has_focus();
        let painter = ui.painter();
        if selected {
            painter.rect_filled(
                rect,
                CornerRadius::same(radius::CONTROL),
                palette.accent_soft,
            );
        } else if hovered {
            painter.rect_filled(rect, CornerRadius::same(radius::CONTROL), palette.control);
        }
        let text_color = if selected || hovered {
            palette.text
        } else {
            palette.text_muted
        };
        let icon_color = if selected {
            palette.accent_text
        } else {
            text_color
        };
        painter.text(
            rect.left_center() + vec2(14.0, 0.0),
            Align2::LEFT_CENTER,
            icon.unicode(),
            icons::font(17.0),
            icon_color,
        );
        let font = if selected {
            theme::semibold(size::BODY)
        } else {
            theme::regular(size::BODY)
        };
        painter.text(
            rect.left_center() + vec2(42.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            font,
            text_color,
        );
        if dot {
            painter.circle_filled(rect.right_center() - vec2(16.0, 0.0), 4.0, palette.danger);
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
