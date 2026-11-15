//! 모델 관리 화면: 로컬 STT 엔진·모델 크기 관리 및 클라우드 API 프로필 관리.

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Align, Color32, Layout, ProgressBar, RichText, Spinner, TextEdit, Ui, vec2};
use rust_i18n::t;
use sulsul_core::cancel::CancelToken;
use sulsul_core::config::{ApiProfile, Config, SttConfig};
use sulsul_core::download::{DownloadPhase, DownloadRequest, download, part_path};
use sulsul_core::settings::SettingField;
use sulsul_core::stt::{
    self, ComputeDevice, EngineContext, EngineDescriptor, EngineKind, EngineTestResult,
    LanguageSupport, ModelVariant, SttRegistry, TranscribeError, delete_variant, format_bytes,
};
use sulsul_core::toml::Value;
use sulsul_stt_cloud::{
    ENGINE_ID as CLOUD_ENGINE_ID, PRESETS, find_preset, normalize_transcriptions_url,
    profile_from_preset,
};

use super::{Page, SECTION_GAP, header};
use crate::accel::{self, AccelEnv, AccelRequest, EngineAccel};
use crate::error_text;
use crate::format;
use crate::i18n::{tr, tr_opt};
use crate::icons::Icon;
use crate::session::{PageContext, Shared};
use crate::system;
use crate::theme::{Palette, radius, size};
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card::Section;
use crate::widgets::confirm::{Answer, Confirm};
use crate::widgets::controls::{self, ComboItem};
use crate::widgets::row::{self, Row};
use crate::widgets::setting_field::setting_fields;
use crate::widgets::text;
use crate::widgets::tile;

/// 가속 장치 카드에서 엔진별 칸을 나란히 놓을 수 있는 최소 너비
const ACCEL_COLUMNS_WIDTH: f32 = 600.0;
/// 가속 장치 칸에서 항목 이름이 차지하는 너비
const ACCEL_FACT_LABEL_WIDTH: f32 = 64.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ModelsTab {
    #[default]
    Local,
    Cloud,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestTarget {
    LocalVariant {
        engine_id: String,
        variant_id: String,
    },
    LocalCustom {
        engine_id: String,
    },
    CloudProfile {
        profile_id: String,
    },
}

struct ActiveTest {
    target: TestTarget,
    cancel: CancelToken,
    rx: Receiver<Result<EngineTestResult, String>>,
    started: Instant,
    /// 테스트한 설정이 요청한 가속. 가속 설정이 없는 엔진(클라우드 등)이면 `None`입니다.
    accel: Option<AccelRequest>,
}

struct TestOutcome {
    target: TestTarget,
    result: Result<EngineTestResult, String>,
    accel: Option<AccelRequest>,
}

struct DownloadSnapshot {
    file_index: usize,
    total_files: usize,
    file_name: String,
    completed_bytes_before: u64,
    total_bytes: u64,
    current_downloaded: u64,
    current_file_size: u64,
    phase: DownloadPhase,
    finished: Option<Result<(), String>>,
}

struct ActiveDownload {
    engine_id: String,
    variant_id: String,
    variant_label: String,
    cancel: CancelToken,
    snapshot: Arc<Mutex<DownloadSnapshot>>,
}

enum PendingConfirm {
    DeleteVariant {
        engine_id: String,
        variant: ModelVariant,
        label: String,
    },
    DeleteProfile {
        id: String,
        name: String,
    },
}

pub struct ModelsPage {
    tab: ModelsTab,
    initialized: bool,
    selected_local_engine: String,
    selected_profile_id: Option<String>,
    reveal_api_key: bool,
    download: Option<ActiveDownload>,
    test: Option<ActiveTest>,
    last_test: Option<TestOutcome>,
    confirm: Option<PendingConfirm>,
}

impl Default for ModelsPage {
    fn default() -> Self {
        Self {
            tab: ModelsTab::Local,
            initialized: false,
            selected_local_engine: sulsul_stt_whisper::ENGINE_ID.to_owned(),
            selected_profile_id: None,
            reveal_api_key: false,
            download: None,
            test: None,
            last_test: None,
            confirm: None,
        }
    }
}

impl ModelsPage {
    /// 백그라운드 다운로드나 테스트가 진행 중이면 `true`입니다.
    pub fn is_busy(&self) -> bool {
        self.download.is_some() || self.test.is_some()
    }

    /// 백그라운드 다운로드 및 테스트 스레드 완료 여부를 확인합니다.
    pub fn tick(&mut self, shared: &mut Shared, ctx: &egui::Context) {
        if let Some(active) = &self.download {
            let finished = active
                .snapshot
                .lock()
                .ok()
                .and_then(|snap| snap.finished.clone());
            if let Some(result) = finished {
                let label = active.variant_label.clone();
                self.download = None;
                match result {
                    Ok(()) => shared
                        .toasts
                        .info(t!("models.local.download.done", model = label)),
                    Err(msg) if msg.is_empty() => {
                        shared.toasts.info(t!("models.local.download.cancelled"));
                    }
                    Err(msg) => shared
                        .toasts
                        .error(t!("models.local.download.failed", message = msg)),
                }
            }
        }

        if let Some(active) = &self.test
            && let Ok(result) = active.rx.try_recv()
        {
            let target = active.target.clone();
            let accel = active.accel;
            self.test = None;
            self.last_test = Some(TestOutcome {
                target,
                result,
                accel,
            });
        }

        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        self.sync_initial_state(cx);

        header(ui, &t!("models.title"), &t!("models.subtitle"));

        if cx.locked {
            Banner::new(Tone::Warning)
                .line(t!("models.locked"))
                .show(ui);
            ui.add_space(10.0);
        }

        self.summary_card(ui, cx);
        ui.add_space(12.0);

        let local_label = t!("models.tab.local").into_owned();
        let cloud_label = t!("models.tab.cloud").into_owned();
        controls::segmented(
            ui,
            &mut self.tab,
            &[
                (ModelsTab::Local, local_label),
                (ModelsTab::Cloud, cloud_label),
            ],
        );
        ui.add_space(14.0);

        match self.tab {
            ModelsTab::Local => self.local_tab(ui, cx),
            ModelsTab::Cloud => self.cloud_tab(ui, cx),
        }

        self.confirm_modal(ui, cx);
    }

    fn sync_initial_state(&mut self, cx: &PageContext<'_>) {
        if !self.initialized {
            self.initialized = true;
            if cx.config.stt.engine == CLOUD_ENGINE_ID {
                self.tab = ModelsTab::Cloud;
            } else if cx
                .shared
                .registry
                .descriptor(&cx.config.stt.engine)
                .is_some_and(|d| d.kind == EngineKind::Local)
            {
                self.selected_local_engine = cx.config.stt.engine.clone();
            }
        }

        let selected_valid = self
            .selected_profile_id
            .as_ref()
            .is_some_and(|id| cx.config.stt.api_profiles.iter().any(|p| &p.id == id));
        if !selected_valid {
            self.selected_profile_id = cx
                .config
                .stt
                .active_api_profile()
                .or_else(|| cx.config.stt.api_profiles.first())
                .map(|p| p.id.clone());
        }
    }

    fn summary_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let palette = Palette::of(ui);
        let engine_ctx = EngineContext {
            paths: cx.shared.paths.clone(),
        };
        let (summary_text, ready) =
            active_setup_summary(&cx.shared.registry, &engine_ctx, cx.config);

        egui::Frame::new()
            .fill(palette.card)
            .stroke(egui::Stroke::new(1.0, palette.card_border))
            .corner_radius(radius::CARD)
            .inner_margin(egui::Margin::symmetric(16, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // 좁은 창에서는 요약 글이 버튼과 겹치지 않게 버튼을 아래 줄로 내립니다.
                let beside = ui.available_width() >= row::ACTIONS_BESIDE_WIDTH;
                row::with_actions(
                    ui,
                    beside,
                    |ui| {
                        if controls::icon_text_button(
                            ui,
                            Icon::AudioLines,
                            t!("models.summary.go_transcribe"),
                        )
                        .clicked()
                        {
                            cx.shared.navigate = Some(Page::Transcribe);
                        }
                    },
                    |ui| {
                        row::title_line(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            if ready {
                                controls::tone_badge(
                                    ui,
                                    &t!("models.summary.ready"),
                                    palette.success_soft,
                                    palette.success_text,
                                );
                            } else {
                                controls::tone_badge(
                                    ui,
                                    &t!("models.summary.not_ready"),
                                    palette.warning_soft,
                                    palette.warning_text,
                                );
                            }
                            ui.label(text::strong(summary_text));
                        });
                    },
                );
            });
    }

    fn local_tab(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        self.local_engines_card(ui, cx);
        ui.add_space(SECTION_GAP);
        self.local_accel_card(ui, cx);
        self.local_catalog_card(ui, cx);
        ui.add_space(SECTION_GAP);
        self.local_settings_card(ui, cx);
    }

    fn local_engines_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let palette = Palette::of(ui);
        let local_engines: Vec<EngineDescriptor> = cx
            .shared
            .registry
            .descriptors()
            .filter(|d| d.kind == EngineKind::Local)
            .cloned()
            .collect();

        if local_engines
            .iter()
            .all(|d| d.id != self.selected_local_engine)
            && let Some(first) = local_engines.first()
        {
            self.selected_local_engine = first.id.clone();
        }

        Section::new(Icon::Cpu, &t!("models.local.engines_title")).show(ui, |ui| {
            ui.label(text::caption(ui, t!("models.local.engines_subtitle")));
            ui.add_space(6.0);

            // 엔진마다 타일 하나. 타일을 누르면 아래 카드들이 그 엔진의 내용으로 바뀝니다.
            for descriptor in &local_engines {
                let is_viewing = self.selected_local_engine == descriptor.id;
                let is_active = cx.config.stt.engine == descriptor.id;
                let title = tr(&descriptor.name_key);
                let desc = tr(&descriptor.description_key);
                let locked = cx.locked;
                let mut activate = false;

                let response = tile::selectable(
                    ui,
                    ("local_engine", &descriptor.id),
                    is_viewing,
                    &title,
                    |ui| {
                        row::with_actions(
                            ui,
                            true,
                            |ui| {
                                // 쓰고 있는 엔진은 버튼 대신 상태만 보여 줍니다.
                                if is_active {
                                    controls::tone_badge(
                                        ui,
                                        &t!("models.local.badge.active"),
                                        palette.success_soft,
                                        palette.success_text,
                                    );
                                } else {
                                    ui.add_enabled_ui(!locked, |ui| {
                                        activate = controls::primary_button(
                                            ui,
                                            t!("models.local.use_engine"),
                                        )
                                        .clicked();
                                    });
                                }
                            },
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                let title_color = if is_viewing {
                                    palette.accent_text
                                } else {
                                    palette.text
                                };
                                // 설명은 남은 너비 안에서 줄을 바꾸므로 버튼에 가려지지 않습니다.
                                ui.label(text::strong(&title).color(title_color));
                                ui.label(text::caption(ui, &desc));
                                row::title_line(ui, |ui| {
                                    controls::badge(
                                        ui,
                                        &language_support_label(&descriptor.capabilities.languages),
                                    );
                                    if descriptor.capabilities.supports_prompt {
                                        controls::badge(ui, &t!("models.cap.prompt"));
                                    }
                                });
                            },
                        );
                    },
                )
                .response;

                if activate {
                    cx.config.stt.engine = descriptor.id.clone();
                    cx.shared
                        .registry
                        .ensure_engine_defaults(&mut cx.config.stt, &descriptor.id);
                }
                if activate || response.clicked() {
                    self.selected_local_engine = descriptor.id.clone();
                }
            }
        });
    }

    /// 가속 장치 카드: 엔진마다 쓸 수 있는 GPU·NPU와, 지금 설정으로 쓸 것으로 예상하는 장치.
    fn local_accel_card(&self, ui: &mut Ui, cx: &PageContext<'_>) {
        let registry = &cx.shared.registry;
        let env = AccelEnv::current();
        let reports: Vec<AccelReport> = registry
            .descriptors()
            .filter(|descriptor| descriptor.kind == EngineKind::Local)
            .filter_map(|descriptor| {
                let accel = EngineAccel::of(registry, &cx.config.stt, &descriptor.id, env)?;
                Some(AccelReport {
                    engine_name: tr(&descriptor.name_key),
                    is_active: cx.config.stt.engine == descriptor.id,
                    note: accel.note(registry),
                    accel,
                })
            })
            .collect();
        if reports.is_empty() {
            return;
        }

        Section::new(Icon::Zap, &t!("models.accel.title")).show(ui, |ui| {
            ui.label(text::caption(ui, t!("models.accel.subtitle")));
            ui.add_space(6.0);

            if ui.available_width() >= ACCEL_COLUMNS_WIDTH {
                // 나란히 놓인 칸은 앞 프레임에서 잰 가장 큰 내용 높이에 맞춰 키를 같게 합니다.
                let height_id = ui.scope_id().with("accel_panel_height");
                let min_height = ui
                    .data(|data| data.get_temp::<f32>(height_id))
                    .unwrap_or(0.0);
                let mut tallest = 0.0_f32;
                for pair in reports.chunks(2) {
                    ui.columns(2, |columns| {
                        for (column, report) in columns.iter_mut().zip(pair) {
                            // `ui.columns`의 칸은 양쪽 맞춤 배치라, 그대로 두면 줄을 바꾼 글의
                            // 낱말 사이가 벌어집니다. 왼쪽 맞춤 배치로 바꿔 그립니다.
                            let height = column
                                .with_layout(Layout::top_down(Align::Min), |ui| {
                                    accel_panel(ui, report, min_height)
                                })
                                .inner;
                            tallest = tallest.max(height);
                        }
                    });
                }
                ui.data_mut(|data| data.insert_temp(height_id, tallest));
            } else {
                for report in &reports {
                    accel_panel(ui, report, 0.0);
                }
            }

            ui.add_space(2.0);
            ui.label(text::caption(ui, t!("models.accel.footnote")));
        });
        ui.add_space(SECTION_GAP);
    }

    fn local_catalog_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let engine_id = self.selected_local_engine.clone();
        let Some(descriptor) = cx.shared.registry.descriptor(&engine_id).cloned() else {
            return;
        };
        let engine_name = tr(&descriptor.name_key);
        let engine_ctx = EngineContext {
            paths: cx.shared.paths.clone(),
        };
        let variants = cx
            .shared
            .registry
            .model_variants(&engine_ctx, &engine_id)
            .unwrap_or_default();
        let model_key = cx
            .shared
            .registry
            .model_setting_key(&engine_id)
            .ok()
            .flatten();
        let resolved = cx
            .shared
            .registry
            .resolve_settings(&cx.config.stt, &engine_id)
            .ok();
        let selected_model = model_key
            .and_then(|key| resolved.as_ref().map(|r| r.str(key).to_owned()))
            .unwrap_or_default();
        let custom_path = resolved
            .as_ref()
            .map(|r| r.str("model_path").trim().to_owned())
            .unwrap_or_default();

        let models_folder: PathBuf = variants
            .first()
            .and_then(|v| v.primary_path())
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| cx.shared.paths.models_dir());

        Section::new(
            Icon::Boxes,
            &t!("models.local.catalog_title", engine = engine_name),
        )
        .show(ui, |ui| {
            let beside = ui.available_width() >= row::ACTIONS_BESIDE_WIDTH;
            row::with_actions(
                ui,
                beside,
                |ui| {
                    if controls::icon_text_button(ui, Icon::FolderOpen, t!("models.local.open_dir"))
                        .clicked()
                        && let Err(message) = system::open_dir(&models_folder)
                    {
                        cx.shared
                            .toasts
                            .error(t!("common.open_failed", message = message));
                    }
                },
                |ui| {
                    ui.label(text::caption(ui, t!("models.local.catalog_help")));
                },
            );

            if !custom_path.is_empty() {
                ui.add_space(6.0);
                let custom_target = TestTarget::LocalCustom {
                    engine_id: engine_id.clone(),
                };
                Banner::new(Tone::Info)
                    .line(t!("models.local.custom_path_active", path = custom_path))
                    .show_with(ui, |ui| {
                        if self.is_testing(&custom_target) {
                            if ui.button(t!("common.cancel")).clicked() {
                                self.cancel_test();
                            }
                        } else {
                            let can_test = self.test.is_none();
                            ui.add_enabled_ui(can_test, |ui| {
                                if controls::icon_text_button(
                                    ui,
                                    Icon::PlugZap,
                                    t!("models.local.custom_path_test"),
                                )
                                .clicked()
                                {
                                    self.start_local_custom_test(
                                        cx.shared,
                                        ui.ctx(),
                                        &cx.config.stt,
                                        &engine_id,
                                    );
                                }
                            });
                        }
                        ui.add_enabled_ui(!cx.locked, |ui| {
                            if ui.button(t!("models.local.custom_path_clear")).clicked()
                                && let Some(table) = cx.config.stt.engines.get_mut(&engine_id)
                            {
                                table.remove("model_path");
                            }
                        });
                    });
                self.render_test_progress(ui, &custom_target);
                self.render_test_outcome(ui, &custom_target);
            }

            if variants.is_empty() {
                ui.add_space(6.0);
                ui.label(text::muted(ui, t!("models.local.catalog_empty")));
                return;
            }

            for variant in &variants {
                row::separator(ui);
                self.variant_row(
                    ui,
                    cx,
                    &engine_id,
                    model_key,
                    &selected_model,
                    custom_path.is_empty(),
                    variant,
                );
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn variant_row(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        engine_id: &str,
        model_key: Option<&'static str>,
        selected_model: &str,
        using_catalog: bool,
        variant: &ModelVariant,
    ) {
        let palette = Palette::of(ui);
        let name = variant_label(variant);
        let description = tr_opt(&variant.description_key);
        let installed = variant.is_installed();
        let active =
            cx.config.stt.engine == engine_id && using_catalog && selected_model == variant.id;
        let partial = if installed { 0 } else { partial_bytes(variant) };
        let is_downloading_this = self
            .download
            .as_ref()
            .is_some_and(|d| d.engine_id == engine_id && d.variant_id == variant.id);
        let target = TestTarget::LocalVariant {
            engine_id: engine_id.to_owned(),
            variant_id: variant.id.clone(),
        };
        let is_testing_this = self.is_testing(&target);

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            // 좁은 창에서는 버튼 묶음을 아래 줄로 내려 이름·설명과 겹치지 않게 합니다.
            let beside = ui.available_width() >= row::ACTIONS_BESIDE_WIDTH;
            row::with_actions(
                ui,
                beside,
                // 오른쪽 끝부터: 삭제 / 테스트(취소) / 사용, 또는 설치(이어받기) / 취소
                |ui| {
                    if is_downloading_this {
                        if ui.button(t!("common.cancel")).clicked()
                            && let Some(active_dl) = &self.download
                        {
                            active_dl.cancel.cancel();
                        }
                    } else if installed {
                        ui.add_enabled_ui(!cx.locked && !is_testing_this, |ui| {
                            if controls::icon_button(
                                ui,
                                Icon::Trash2,
                                &t!("models.local.action.delete"),
                            )
                            .clicked()
                            {
                                self.confirm = Some(PendingConfirm::DeleteVariant {
                                    engine_id: engine_id.to_owned(),
                                    variant: variant.clone(),
                                    label: name.clone(),
                                });
                            }
                        });

                        if is_testing_this {
                            if ui.button(t!("common.cancel")).clicked() {
                                self.cancel_test();
                            }
                        } else {
                            let can_test = self.test.is_none();
                            ui.add_enabled_ui(can_test, |ui| {
                                if controls::icon_text_button(
                                    ui,
                                    Icon::PlugZap,
                                    t!("models.local.action.test"),
                                )
                                .clicked()
                                {
                                    self.start_local_variant_test(
                                        cx.shared,
                                        ui.ctx(),
                                        &cx.config.stt,
                                        engine_id,
                                        model_key,
                                        &variant.id,
                                    );
                                }
                            });
                        }

                        if !active {
                            ui.add_enabled_ui(!cx.locked, |ui| {
                                if controls::primary_icon_button(
                                    ui,
                                    Icon::Check,
                                    t!("models.local.action.activate"),
                                )
                                .clicked()
                                {
                                    activate_local_variant(cx, engine_id, model_key, &variant.id);
                                }
                            });
                        }
                    } else {
                        let label = if partial > 0 {
                            t!("models.local.action.resume")
                        } else {
                            t!("models.local.action.install")
                        };
                        let can_install = self.download.is_none();
                        ui.add_enabled_ui(can_install, |ui| {
                            if controls::primary_icon_button(ui, Icon::Download, label).clicked() {
                                self.start_download(ui.ctx(), engine_id, variant);
                            }
                        });
                    }
                },
                // 이름 · 크기 · 배지 · 설명
                |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    row::title_line(ui, |ui| {
                        ui.label(text::strong(&name));
                        controls::badge(ui, &format_bytes(variant.total_bytes()));
                        if active {
                            controls::tone_badge(
                                ui,
                                &t!("models.local.badge.active"),
                                palette.success_soft,
                                palette.success_text,
                            );
                        } else if installed {
                            controls::badge(ui, &t!("models.local.badge.installed"));
                        }
                        if variant.recommended {
                            controls::tone_badge(
                                ui,
                                &t!("models.local.badge.recommended"),
                                palette.accent_soft,
                                palette.accent_text,
                            );
                        }
                        if partial > 0 {
                            controls::tone_badge(
                                ui,
                                &t!("models.local.badge.resumable"),
                                palette.warning_soft,
                                palette.warning_text,
                            );
                        }
                    });
                    if let Some(desc) = &description {
                        ui.label(text::caption(ui, desc));
                    }
                },
            );

            if is_downloading_this && let Some(active_dl) = &self.download {
                render_download_progress(ui, active_dl);
            }
            self.render_test_progress(ui, &target);
            self.render_test_outcome(ui, &target);
        });
    }

    fn local_settings_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let engine_id = self.selected_local_engine.clone();
        let Some(descriptor) = cx.shared.registry.descriptor(&engine_id).cloned() else {
            return;
        };
        let model_key = cx
            .shared
            .registry
            .model_setting_key(&engine_id)
            .ok()
            .flatten();
        let extra_fields: Vec<SettingField> = descriptor
            .settings
            .into_iter()
            .filter(|f| Some(f.key.as_str()) != model_key)
            .collect();
        if extra_fields.is_empty() {
            return;
        }

        let engine_name = tr(&descriptor.name_key);
        Section::new(
            Icon::Settings,
            &t!("models.local.settings_title", engine = engine_name),
        )
        .show(ui, |ui| {
            ui.label(text::caption(
                ui,
                t!("models.local.settings_help", id = engine_id),
            ));
            ui.add_space(6.0);
            ui.add_enabled_ui(!cx.locked, |ui| {
                let table = cx.config.stt.engines.entry(engine_id.clone()).or_default();
                setting_fields(ui, &engine_id, &extra_fields, table, cx.frame);
            });
        });
    }

    fn cloud_tab(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        self.cloud_profiles_card(ui, cx);
        ui.add_space(SECTION_GAP);
        self.cloud_editor_card(ui, cx);
        ui.add_space(SECTION_GAP);
        self.cloud_settings_card(ui, cx);
    }

    fn cloud_profiles_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let palette = Palette::of(ui);
        let engine_active = cx.config.stt.engine == CLOUD_ENGINE_ID;

        Section::new(Icon::Cloud, &t!("models.cloud.profiles_title")).show(ui, |ui| {
            ui.label(text::caption(ui, t!("models.cloud.profiles_subtitle")));
            ui.add_space(8.0);

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                if engine_active {
                    controls::tone_badge(
                        ui,
                        &t!("models.cloud.active_engine"),
                        palette.success_soft,
                        palette.success_text,
                    );
                } else {
                    ui.add_enabled_ui(!cx.locked, |ui| {
                        if controls::primary_button(ui, t!("models.cloud.activate_engine"))
                            .clicked()
                        {
                            cx.config.stt.engine = CLOUD_ENGINE_ID.to_owned();
                            cx.shared
                                .registry
                                .ensure_engine_defaults(&mut cx.config.stt, CLOUD_ENGINE_ID);
                        }
                    });
                }

                ui.add_enabled_ui(!cx.locked, |ui| {
                    for preset in PRESETS {
                        let provider_label = tr(preset.name_key);
                        if controls::icon_text_button(
                            ui,
                            Icon::Plus,
                            t!("models.cloud.add_preset", provider = provider_label),
                        )
                        .clicked()
                        {
                            let profile =
                                create_profile_from_preset(&cx.config.stt.api_profiles, preset.id);
                            let id = profile.id.clone();
                            cx.config.stt.api_profiles.push(profile);
                            if cx.config.stt.api_profile.trim().is_empty() {
                                cx.config.stt.api_profile = id.clone();
                            }
                            self.selected_profile_id = Some(id);
                        }
                    }
                });
            });

            if cx.config.stt.api_profiles.is_empty() {
                ui.add_space(8.0);
                ui.label(text::muted(ui, t!("models.cloud.profiles_empty")));
                return;
            }

            let active_id = cx.config.stt.active_api_profile().map(|p| p.id.clone());
            let mut select_id: Option<String> = None;
            let mut activate_id: Option<String> = None;
            let mut duplicate_id: Option<String> = None;
            let mut delete_target: Option<(String, String)> = None;

            ui.add_space(4.0);
            let beside = ui.available_width() >= row::ACTIONS_BESIDE_WIDTH;
            let locked = cx.locked;
            // 프로필마다 타일 하나. 타일을 누르면 아래 편집 카드가 그 프로필로 바뀝니다.
            for profile in &cx.config.stt.api_profiles {
                let is_selected = self.selected_profile_id.as_deref() == Some(profile.id.as_str());
                let is_default_profile = active_id.as_deref() == Some(profile.id.as_str());
                let provider_label = find_preset(&profile.provider)
                    .map(|p| tr(p.name_key))
                    .unwrap_or_else(|| profile.provider.clone());
                let display_name = if profile.name.trim().is_empty() {
                    profile.id.clone()
                } else {
                    profile.name.clone()
                };

                let response = tile::selectable(
                    ui,
                    ("api_profile", &profile.id),
                    is_selected,
                    &display_name,
                    |ui| {
                        row::with_actions(
                            ui,
                            beside,
                            // 오른쪽 끝부터: 삭제 / 복제 / 활성화
                            |ui| {
                                ui.add_enabled_ui(!locked, |ui| {
                                    if controls::icon_button(
                                        ui,
                                        Icon::Trash2,
                                        &t!("models.cloud.action.delete"),
                                    )
                                    .clicked()
                                    {
                                        delete_target =
                                            Some((profile.id.clone(), display_name.clone()));
                                    }
                                    if controls::icon_button(
                                        ui,
                                        Icon::Copy,
                                        &t!("models.cloud.action.duplicate"),
                                    )
                                    .clicked()
                                    {
                                        duplicate_id = Some(profile.id.clone());
                                    }
                                    if !is_default_profile
                                        && ui.button(t!("models.cloud.action.use")).clicked()
                                    {
                                        activate_id = Some(profile.id.clone());
                                    }
                                });
                            },
                            // 이름 · 공급자 · 상태, 그 아래 모델 이름
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 3.0;
                                let name_color = if is_selected {
                                    palette.accent_text
                                } else {
                                    palette.text
                                };
                                row::title_line(ui, |ui| {
                                    ui.label(text::strong(&display_name).color(name_color));
                                    controls::badge(ui, &provider_label);
                                    if is_default_profile {
                                        controls::tone_badge(
                                            ui,
                                            &t!("models.cloud.badge.default_profile"),
                                            palette.success_soft,
                                            palette.success_text,
                                        );
                                    }
                                });
                                if !profile.model.trim().is_empty() {
                                    ui.label(text::caption(ui, &profile.model));
                                }
                            },
                        );
                    },
                )
                .response;
                if response.clicked() {
                    select_id = Some(profile.id.clone());
                }
            }

            if let Some(id) = select_id {
                self.selected_profile_id = Some(id);
            }
            if let Some(id) = activate_id {
                cx.config.stt.api_profile = id.clone();
                self.selected_profile_id = Some(id);
            }
            if let Some(id) = duplicate_id
                && let Some(source) = cx
                    .config
                    .stt
                    .api_profiles
                    .iter()
                    .find(|p| p.id == id)
                    .cloned()
            {
                let mut cloned = source.clone();
                cloned.id = next_profile_id(&cx.config.stt.api_profiles, &source.provider);
                cloned.name = format!("{}{}", source.name.trim(), t!("models.cloud.copy_suffix"));
                let new_id = cloned.id.clone();
                cx.config.stt.api_profiles.push(cloned);
                self.selected_profile_id = Some(new_id);
            }
            if let Some((id, name)) = delete_target {
                self.confirm = Some(PendingConfirm::DeleteProfile { id, name });
            }
        });
    }

    fn cloud_editor_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let Some(selected_id) = self.selected_profile_id.clone() else {
            Section::new(Icon::Settings, &t!("models.cloud.editor_title")).show(ui, |ui| {
                ui.label(text::muted(ui, t!("models.cloud.editor_empty")));
            });
            return;
        };

        let Some(index) = cx
            .config
            .stt
            .api_profiles
            .iter()
            .position(|p| p.id == selected_id)
        else {
            Section::new(Icon::Settings, &t!("models.cloud.editor_title")).show(ui, |ui| {
                ui.label(text::muted(ui, t!("models.cloud.editor_empty")));
            });
            return;
        };

        Section::new(Icon::Settings, &t!("models.cloud.editor_title")).show(ui, |ui| {
            let palette = Palette::of(ui);
            let test_snapshot: ApiProfile;

            {
                let profile = &mut cx.config.stt.api_profiles[index];
                let field_enabled = !cx.locked;

                ui.add_enabled_ui(field_enabled, |ui| {
                    // 프로필 이름
                    Row::new(&t!("models.cloud.field.name")).show(ui, |ui| {
                        // 좁은 창에서는 조작 영역이 기본 너비보다 좁아집니다.
                        let width = row::CONTROL_WIDTH.min(ui.available_width());
                        ui.add_sized(
                            [width, ui.spacing().interact_size.y],
                            TextEdit::singleline(&mut profile.name)
                                .hint_text(t!("models.cloud.field.name_hint")),
                        );
                    });
                    row::separator(ui);

                    // 공급자 프리셋
                    let old_provider = profile.provider.clone();
                    let provider_items: Vec<ComboItem<String>> = PRESETS
                        .iter()
                        .map(|p| ComboItem::new(p.id.to_owned(), tr(p.name_key)))
                        .collect();
                    Row::new(&t!("models.cloud.field.provider")).show(ui, |ui| {
                        let fallback = profile.provider.clone();
                        let width = row::CONTROL_WIDTH.min(ui.available_width());
                        controls::combo(
                            ui,
                            ("cloud_provider", &profile.id),
                            width,
                            &mut profile.provider,
                            &provider_items,
                            &fallback,
                        );
                    });
                    if profile.provider != old_provider {
                        apply_provider_preset_switch(
                            profile,
                            &old_provider,
                            &profile.provider.clone(),
                        );
                    }
                    row::separator(ui);

                    // 엔드포인트 URL
                    let endpoint_preview = normalize_transcriptions_url(&profile.endpoint);
                    Row::new(&t!("models.cloud.field.endpoint")).show_below(ui, |ui| {
                        ui.add(
                            TextEdit::singleline(&mut profile.endpoint)
                                .desired_width(f32::INFINITY)
                                .hint_text(t!("models.cloud.field.endpoint_hint")),
                        );
                        match &endpoint_preview {
                            Ok(url) => {
                                ui.label(text::caption(
                                    ui,
                                    t!("models.cloud.field.endpoint_resolved", url = url),
                                ));
                            }
                            Err(err) => {
                                ui.label(
                                    text::caption(
                                        ui,
                                        t!(
                                            "models.cloud.field.endpoint_invalid",
                                            message = err.to_string()
                                        ),
                                    )
                                    .color(palette.warning_text),
                                );
                            }
                        }
                    });
                    row::separator(ui);

                    // API 키 (비밀번호 마스킹 + 보기 토글)
                    Row::new(&t!("models.cloud.field.api_key")).show_below(ui, |ui| {
                        let eye_icon = if self.reveal_api_key {
                            Icon::EyeOff
                        } else {
                            Icon::Eye
                        };
                        let toggle_label = if self.reveal_api_key {
                            t!("models.cloud.field.api_key_hide")
                        } else {
                            t!("models.cloud.field.api_key_show")
                        };
                        // 버튼을 오른쪽 끝에 먼저 놓고, 남은 너비를 입력 칸이 씁니다.
                        // 너비를 미리 어림하면 줄이 카드 밖으로 조금 넘칩니다.
                        let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
                        ui.allocate_ui_with_layout(
                            size,
                            Layout::right_to_left(Align::Center),
                            |ui| {
                                if controls::icon_button(ui, eye_icon, &toggle_label).clicked() {
                                    self.reveal_api_key = !self.reveal_api_key;
                                }
                                ui.add(
                                    TextEdit::singleline(&mut profile.api_key)
                                        .password(!self.reveal_api_key)
                                        .desired_width(ui.available_width())
                                        .hint_text(t!("models.cloud.field.api_key_hint")),
                                );
                            },
                        );
                    });
                    row::separator(ui);

                    // 모델 이름 + 추천 모델 칩
                    Row::new(&t!("models.cloud.field.model")).show_below(ui, |ui| {
                        ui.add(
                            TextEdit::singleline(&mut profile.model)
                                .desired_width(f32::INFINITY)
                                .hint_text(t!("models.cloud.field.model_hint")),
                        );
                        if let Some(preset) = find_preset(&profile.provider)
                            && !preset.model_suggestions.is_empty()
                        {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                                ui.label(text::caption(ui, t!("models.cloud.field.suggestions")));
                                for suggestion in preset.model_suggestions {
                                    let selected = profile.model.trim() == *suggestion;
                                    if ui.selectable_label(selected, *suggestion).clicked() {
                                        profile.model = (*suggestion).to_owned();
                                    }
                                }
                            });
                        }
                    });
                });

                test_snapshot = profile.clone();
            }

            ui.add_space(10.0);
            let test_target = TestTarget::CloudProfile {
                profile_id: selected_id,
            };
            let is_testing_this = self.is_testing(&test_target);

            ui.horizontal(|ui| {
                if is_testing_this {
                    if ui.button(t!("common.cancel")).clicked() {
                        self.cancel_test();
                    }
                } else {
                    let can_test = self.test.is_none();
                    ui.add_enabled_ui(can_test, |ui| {
                        if controls::primary_icon_button(
                            ui,
                            Icon::PlugZap,
                            t!("models.cloud.test_button"),
                        )
                        .clicked()
                        {
                            self.start_cloud_profile_test(
                                cx.shared,
                                ui.ctx(),
                                &cx.config.stt,
                                test_snapshot,
                            );
                        }
                    });
                }
            });

            self.render_test_progress(ui, &test_target);
            self.render_test_outcome(ui, &test_target);
        });
    }

    fn cloud_settings_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let Some(descriptor) = cx.shared.registry.descriptor(CLOUD_ENGINE_ID).cloned() else {
            return;
        };
        if descriptor.settings.is_empty() {
            return;
        }
        Section::new(Icon::Settings, &t!("models.cloud.settings_title")).show(ui, |ui| {
            ui.label(text::caption(
                ui,
                t!("models.cloud.settings_help", id = CLOUD_ENGINE_ID),
            ));
            ui.add_space(6.0);
            ui.add_enabled_ui(!cx.locked, |ui| {
                let table = cx
                    .config
                    .stt
                    .engines
                    .entry(CLOUD_ENGINE_ID.to_owned())
                    .or_default();
                setting_fields(ui, CLOUD_ENGINE_ID, &descriptor.settings, table, cx.frame);
            });
        });
    }

    fn is_testing(&self, target: &TestTarget) -> bool {
        self.test.as_ref().is_some_and(|t| &t.target == target)
    }

    fn cancel_test(&mut self) {
        if let Some(active) = &self.test {
            active.cancel.cancel();
        }
    }

    fn start_download(
        &mut self,
        egui_ctx: &egui::Context,
        engine_id: &str,
        variant: &ModelVariant,
    ) {
        if self.download.is_some() {
            return;
        }
        let files_to_download: Vec<_> = variant.missing_files().into_iter().cloned().collect();
        if files_to_download.is_empty() {
            return;
        }
        let total_bytes: u64 = files_to_download.iter().map(|f| f.asset.size_bytes).sum();
        let first = &files_to_download[0];
        let cancel = CancelToken::new();
        let snapshot = Arc::new(Mutex::new(DownloadSnapshot {
            file_index: 1,
            total_files: files_to_download.len(),
            file_name: first.asset.file_name.clone(),
            completed_bytes_before: 0,
            total_bytes,
            current_downloaded: 0,
            current_file_size: first.asset.size_bytes,
            phase: DownloadPhase::Downloading,
            finished: None,
        }));

        let bg_cancel = cancel.clone();
        let bg_snapshot = snapshot.clone();
        let repaint_ctx = egui_ctx.clone();

        std::thread::Builder::new()
            .name(format!("model-download-{}", variant.id))
            .spawn(move || {
                let mut completed_before = 0u64;
                let total_files = files_to_download.len();
                let mut final_result: Result<(), String> = Ok(());

                for (idx, file) in files_to_download.iter().enumerate() {
                    if let Ok(mut snap) = bg_snapshot.lock() {
                        snap.file_index = idx + 1;
                        snap.total_files = total_files;
                        snap.file_name = file.asset.file_name.clone();
                        snap.completed_bytes_before = completed_before;
                        snap.current_downloaded = 0;
                        snap.current_file_size = file.asset.size_bytes;
                        snap.phase = DownloadPhase::Downloading;
                    }
                    repaint_ctx.request_repaint();

                    let req = DownloadRequest::for_file(file);
                    let mut cb = |progress: sulsul_core::download::DownloadProgress| {
                        if let Ok(mut snap) = bg_snapshot.lock() {
                            snap.current_downloaded = progress.downloaded;
                            if let Some(total) = progress.total {
                                snap.current_file_size = total;
                            }
                            snap.phase = progress.phase;
                        }
                        repaint_ctx.request_repaint();
                    };

                    if let Err(err) = download(&req, &bg_cancel, &mut cb) {
                        final_result = match err {
                            sulsul_core::download::DownloadError::Cancelled => Err(String::new()),
                            other => Err(error_text::download(&other)),
                        };
                        break;
                    }
                    completed_before += file.asset.size_bytes;
                }

                if let Ok(mut snap) = bg_snapshot.lock() {
                    snap.finished = Some(final_result);
                }
                repaint_ctx.request_repaint();
            })
            .ok();

        self.download = Some(ActiveDownload {
            engine_id: engine_id.to_owned(),
            variant_id: variant.id.clone(),
            variant_label: variant_label(variant),
            cancel,
            snapshot,
        });
    }

    fn start_local_variant_test(
        &mut self,
        shared: &Shared,
        egui_ctx: &egui::Context,
        base_stt: &SttConfig,
        engine_id: &str,
        model_key: Option<&'static str>,
        variant_id: &str,
    ) {
        let mut stt = base_stt.clone();
        stt.engine = engine_id.to_owned();
        let table = stt.engines.entry(engine_id.to_owned()).or_default();
        if let Some(key) = model_key {
            table.insert(key.to_owned(), Value::String(variant_id.to_owned()));
        }
        table.remove("model_path");
        self.spawn_test(
            shared,
            egui_ctx,
            TestTarget::LocalVariant {
                engine_id: engine_id.to_owned(),
                variant_id: variant_id.to_owned(),
            },
            stt,
            engine_id.to_owned(),
        );
    }

    fn start_local_custom_test(
        &mut self,
        shared: &Shared,
        egui_ctx: &egui::Context,
        base_stt: &SttConfig,
        engine_id: &str,
    ) {
        let mut stt = base_stt.clone();
        stt.engine = engine_id.to_owned();
        self.spawn_test(
            shared,
            egui_ctx,
            TestTarget::LocalCustom {
                engine_id: engine_id.to_owned(),
            },
            stt,
            engine_id.to_owned(),
        );
    }

    fn start_cloud_profile_test(
        &mut self,
        shared: &Shared,
        egui_ctx: &egui::Context,
        base_stt: &SttConfig,
        profile: ApiProfile,
    ) {
        let mut stt = base_stt.clone();
        stt.engine = CLOUD_ENGINE_ID.to_owned();
        stt.api_profile = profile.id.clone();
        if let Some(existing) = stt.api_profiles.iter_mut().find(|p| p.id == profile.id) {
            *existing = profile.clone();
        } else {
            stt.api_profiles.push(profile.clone());
        }
        // CLI 재정의 테이블이 테스트 결과를 덮지 않도록 비웁니다.
        if let Some(table) = stt.engines.get_mut(CLOUD_ENGINE_ID) {
            for key in ["provider", "endpoint", "api_key", "model"] {
                table.remove(key);
            }
        }
        self.spawn_test(
            shared,
            egui_ctx,
            TestTarget::CloudProfile {
                profile_id: profile.id,
            },
            stt,
            CLOUD_ENGINE_ID.to_owned(),
        );
    }

    fn spawn_test(
        &mut self,
        shared: &Shared,
        egui_ctx: &egui::Context,
        target: TestTarget,
        stt: SttConfig,
        engine_id: String,
    ) {
        if self.test.is_some() {
            return;
        }
        let cancel = CancelToken::new();
        let (tx, rx) = mpsc::channel();
        let registry = shared.registry.clone();
        let ctx = EngineContext {
            paths: shared.paths.clone(),
        };
        // 테스트에 쓰는 설정 그대로에서 읽어, 결과와 견줄 때 화면의 값이 바뀌어도 어긋나지 않게 합니다.
        let accel = AccelRequest::from_settings(&registry, &stt, &engine_id);
        let bg_cancel = cancel.clone();
        let repaint_ctx = egui_ctx.clone();

        std::thread::Builder::new()
            .name(format!("stt-test-{engine_id}"))
            .spawn(move || {
                let outcome = stt::test_engine(&registry, &ctx, &stt, &engine_id, bg_cancel)
                    .map_err(|err| match err {
                        TranscribeError::Stt(e) => error_text::stt(&e, &registry),
                        TranscribeError::Audio(e) => error_text::audio(&e),
                    });
                let _ = tx.send(outcome);
                repaint_ctx.request_repaint();
            })
            .ok();

        self.test = Some(ActiveTest {
            target,
            cancel,
            rx,
            started: Instant::now(),
            accel,
        });
    }

    /// 테스트가 진행 중이면 경과 시간과, 가속 장치를 처음 쓸 때 오래 걸릴 수 있다는 안내를 보여 줍니다.
    fn render_test_progress(&self, ui: &mut Ui, target: &TestTarget) {
        let Some(active) = &self.test else {
            return;
        };
        if &active.target != target {
            return;
        }
        let elapsed = format::seconds_text(active.started.elapsed());
        let status = match target {
            TestTarget::CloudProfile { .. } => {
                format!("{} {elapsed}", t!("models.cloud.testing"))
            }
            _ => t!("models.test.running", elapsed = elapsed).into_owned(),
        };
        let hint = active
            .accel
            .and_then(|request| accel::first_run_hint(request, AccelEnv::current()));
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add(Spinner::new().size(16.0));
            ui.label(text::muted(ui, status));
        });
        if let Some(hint) = hint {
            ui.label(text::caption(ui, hint));
        }
    }

    fn render_test_outcome(&self, ui: &mut Ui, target: &TestTarget) {
        let Some(outcome) = &self.last_test else {
            return;
        };
        if &outcome.target != target {
            return;
        }
        ui.add_space(4.0);
        match &outcome.result {
            Ok(res) => {
                // 여기 나오는 장치는 엔진이 모델을 불러온 뒤 보고한 실제 값입니다.
                let device = t!(
                    "models.test.used_device",
                    device = accel::device_label(&res.device)
                );
                let timing = t!(
                    "models.test.timing",
                    prepare = format::elapsed_text(res.prepare),
                    recognize = format::elapsed_text(res.recognize)
                );
                let detail = if res.transcript.text.trim().is_empty() {
                    t!("models.test.empty_transcript").into_owned()
                } else {
                    t!("models.test.transcript", text = res.transcript.text.trim()).into_owned()
                };
                Banner::new(Tone::Success)
                    .title(&t!("models.test.success_title"))
                    .line(device)
                    .line(timing)
                    .line(detail)
                    .show(ui);
                // 가속을 요청했는데 그 장치로 실행하지 못했으면 이유를 따로 알립니다.
                let note = outcome.accel.and_then(|request| {
                    accel::fallback_note(request, &res.device, AccelEnv::current())
                });
                if let Some(note) = note {
                    ui.add_space(4.0);
                    Banner::new(Tone::Warning).line(note).show(ui);
                }
            }
            Err(message) => {
                Banner::new(Tone::Error)
                    .line(t!("models.test.failed", message = message))
                    .show(ui);
            }
        }
    }

    fn confirm_modal(&mut self, ui: &Ui, cx: &mut PageContext<'_>) {
        let Some(pending) = self.confirm.take() else {
            return;
        };
        match pending {
            PendingConfirm::DeleteVariant {
                engine_id,
                variant,
                label,
            } => {
                let body = t!("models.local.delete.body", model = label);
                let answer = Confirm {
                    id: "confirm_delete_model_variant",
                    title: &t!("models.local.delete.title"),
                    body: &body,
                    confirm_label: &t!("models.local.delete.confirm"),
                    cancel_label: &t!("common.cancel"),
                    danger: true,
                }
                .show(ui);
                match answer {
                    Some(Answer::Confirm) => {
                        let engine_ctx = EngineContext {
                            paths: cx.shared.paths.clone(),
                        };
                        let catalog = cx
                            .shared
                            .registry
                            .model_variants(&engine_ctx, &engine_id)
                            .unwrap_or_default();
                        match delete_variant(&variant, &catalog) {
                            Ok(()) => cx
                                .shared
                                .toasts
                                .info(t!("models.local.delete.done", model = label)),
                            Err(err) => cx
                                .shared
                                .toasts
                                .error(t!("models.local.delete.failed", message = err.to_string())),
                        }
                    }
                    Some(Answer::Cancel) => {}
                    None => {
                        self.confirm = Some(PendingConfirm::DeleteVariant {
                            engine_id,
                            variant,
                            label,
                        });
                    }
                }
            }
            PendingConfirm::DeleteProfile { id, name } => {
                let body = t!("models.cloud.delete.body", name = name);
                let answer = Confirm {
                    id: "confirm_delete_api_profile",
                    title: &t!("models.cloud.delete.title"),
                    body: &body,
                    confirm_label: &t!("models.cloud.delete.confirm"),
                    cancel_label: &t!("common.cancel"),
                    danger: true,
                }
                .show(ui);
                match answer {
                    Some(Answer::Confirm) => {
                        cx.config.stt.api_profiles.retain(|p| p.id != id);
                        if cx.config.stt.api_profile == id {
                            cx.config.stt.api_profile = cx
                                .config
                                .stt
                                .api_profiles
                                .first()
                                .map(|p| p.id.clone())
                                .unwrap_or_default();
                        }
                        if self.selected_profile_id.as_deref() == Some(id.as_str()) {
                            self.selected_profile_id =
                                cx.config.stt.api_profiles.first().map(|p| p.id.clone());
                        }
                    }
                    Some(Answer::Cancel) => {}
                    None => {
                        self.confirm = Some(PendingConfirm::DeleteProfile { id, name });
                    }
                }
            }
        }
    }
}

fn render_download_progress(ui: &mut Ui, active: &ActiveDownload) {
    let Ok(snap) = active.snapshot.lock() else {
        return;
    };
    let (fraction, label) = match snap.phase {
        DownloadPhase::Downloading => {
            let overall =
                snap.completed_bytes_before + snap.current_downloaded.min(snap.current_file_size);
            let frac = (overall as f32 / snap.total_bytes.max(1) as f32).clamp(0.0, 1.0);
            let progress_text = format!(
                "{} / {} ({})",
                format_bytes(overall),
                format_bytes(snap.total_bytes),
                format::percent_text(frac)
            );
            let text = t!(
                "models.local.download.downloading",
                file = snap.file_name,
                index = snap.file_index,
                total_files = snap.total_files,
                progress = progress_text
            )
            .into_owned();
            (frac, text)
        }
        DownloadPhase::Verifying => {
            let frac = (snap.current_downloaded as f32 / snap.current_file_size.max(1) as f32)
                .clamp(0.0, 1.0);
            let text = t!(
                "models.local.download.verifying",
                file = snap.file_name,
                index = snap.file_index,
                total_files = snap.total_files
            )
            .into_owned();
            (frac, text)
        }
    };
    ui.add(
        ProgressBar::new(fraction)
            .text(label)
            .desired_width(f32::INFINITY),
    );
}

fn activate_local_variant(
    cx: &mut PageContext<'_>,
    engine_id: &str,
    model_key: Option<&'static str>,
    variant_id: &str,
) {
    cx.config.stt.engine = engine_id.to_owned();
    cx.shared
        .registry
        .ensure_engine_defaults(&mut cx.config.stt, engine_id);
    let table = cx
        .config
        .stt
        .engines
        .entry(engine_id.to_owned())
        .or_default();
    if let Some(key) = model_key {
        table.insert(key.to_owned(), Value::String(variant_id.to_owned()));
    }
    table.remove("model_path");
}

fn active_setup_summary(
    registry: &SttRegistry,
    engine_ctx: &EngineContext,
    config: &Config,
) -> (String, bool) {
    let engine_id = config.stt.engine.as_str();
    let engine_name = error_text::engine_name(registry, engine_id);

    if engine_id == CLOUD_ENGINE_ID {
        return match config.stt.active_api_profile() {
            Some(profile) => {
                let name = if profile.name.trim().is_empty() {
                    profile.id.clone()
                } else {
                    profile.name.clone()
                };
                let model = if profile.model.trim().is_empty() {
                    "-".to_owned()
                } else {
                    profile.model.clone()
                };
                let ready = cloud_profile_ready(profile);
                (
                    t!(
                        "models.summary.active_cloud",
                        engine = engine_name,
                        profile = name,
                        model = model
                    )
                    .into_owned(),
                    ready,
                )
            }
            None => (
                t!(
                    "models.summary.active_cloud_unconfigured",
                    engine = engine_name
                )
                .into_owned(),
                false,
            ),
        };
    }

    let req = registry
        .model(engine_ctx, &config.stt, engine_id)
        .ok()
        .flatten();
    let ready = req.as_ref().is_none_or(|r| r.is_installed());

    if let Some(descriptor) = registry.descriptor(engine_id)
        && descriptor.kind == EngineKind::Local
    {
        let resolved = registry.resolve_settings(&config.stt, engine_id).ok();
        let custom = resolved
            .as_ref()
            .is_some_and(|r| !r.str("model_path").trim().is_empty());
        if custom {
            return (
                t!("models.summary.active_local_custom", engine = engine_name).into_owned(),
                ready,
            );
        }
        let model_label = registry
            .model_setting_key(engine_id)
            .ok()
            .flatten()
            .and_then(|key| resolved.as_ref().map(|r| r.str(key).to_owned()))
            .map(|id| {
                registry
                    .model_variants(engine_ctx, engine_id)
                    .ok()
                    .and_then(|vs| vs.into_iter().find(|v| v.id == id))
                    .map(|v| variant_label(&v))
                    .unwrap_or(id)
            })
            .unwrap_or_default();
        return (
            t!(
                "models.summary.active_local",
                engine = engine_name,
                model = model_label
            )
            .into_owned(),
            ready,
        );
    }

    (
        t!("models.summary.active_generic", engine = engine_name).into_owned(),
        ready,
    )
}

fn cloud_profile_ready(profile: &ApiProfile) -> bool {
    if normalize_transcriptions_url(&profile.endpoint).is_err() || profile.model.trim().is_empty() {
        return false;
    }
    let key_required = find_preset(&profile.provider).is_some_and(|p| p.requires_api_key);
    !key_required || !profile.api_key.trim().is_empty()
}

fn variant_label(variant: &ModelVariant) -> String {
    tr_opt(&variant.name_key).unwrap_or_else(|| variant.id.clone())
}

fn partial_bytes(variant: &ModelVariant) -> u64 {
    variant
        .missing_files()
        .into_iter()
        .map(|f| {
            fs::metadata(part_path(&f.path))
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .sum()
}

fn create_profile_from_preset(existing: &[ApiProfile], provider: &str) -> ApiProfile {
    let id = next_profile_id(existing, provider);
    let provider_name = find_preset(provider)
        .map(|p| tr(p.name_key))
        .unwrap_or_else(|| provider.to_owned());
    let base_name = t!(
        "models.cloud.default_profile_name",
        provider = provider_name
    )
    .into_owned();
    let name = next_profile_name(existing, &base_name);
    profile_from_preset(provider, id, name)
}

fn next_profile_id(existing: &[ApiProfile], prefix: &str) -> String {
    let clean_prefix: String = prefix
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    let base = if clean_prefix.is_empty() {
        "profile"
    } else {
        clean_prefix.as_str()
    };
    for n in 1usize.. {
        let candidate = if n == 1 {
            base.to_owned()
        } else {
            format!("{base}-{n}")
        };
        if !existing.iter().any(|p| p.id == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn next_profile_name(existing: &[ApiProfile], base_name: &str) -> String {
    if !existing.iter().any(|p| p.name == base_name) {
        return base_name.to_owned();
    }
    for n in 2usize.. {
        let candidate = format!("{base_name} {n}");
        if !existing.iter().any(|p| p.name == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn apply_provider_preset_switch(profile: &mut ApiProfile, old_provider: &str, new_provider: &str) {
    let Some(new_preset) = find_preset(new_provider) else {
        return;
    };
    let old_default_endpoint = find_preset(old_provider).map(|p| p.default_endpoint);
    let old_default_model = find_preset(old_provider).map(|p| p.default_model);

    if profile.endpoint.trim().is_empty()
        || old_default_endpoint.is_some_and(|d| profile.endpoint.trim() == d)
    {
        profile.endpoint = new_preset.default_endpoint.to_owned();
    }
    if profile.model.trim().is_empty()
        || old_default_model.is_some_and(|d| profile.model.trim() == d)
    {
        profile.model = new_preset.default_model.to_owned();
    }
}

/// 가속 장치 카드에 보여 줄 엔진 하나의 내용
struct AccelReport {
    engine_name: String,
    /// 받아쓰기에 쓰고 있는 엔진인지
    is_active: bool,
    accel: EngineAccel,
    /// 예상 장치가 그렇게 정해진 이유
    note: String,
}

/// 가속 장치 카드의 엔진 한 칸을 그리고, 내용의 높이를 돌려줍니다.
///
/// 위에는 지금 설정으로 쓸 것으로 **예상하는** 장치와 그 이유를, 아래에는 이 빌드의 가속
/// 방식과 찾은 장치, 지금 설정 값을 보여 줍니다. 실제로 쓴 장치는 테스트 결과에만 나옵니다.
fn accel_panel(ui: &mut Ui, report: &AccelReport, min_height: f32) -> f32 {
    let palette = Palette::of(ui);
    let plan = &report.accel.plan;
    // 가속 장치를 쓰면 초록, 쓰려던 가속을 못 쓰면 경고색, 일부러 CPU를 쓰면 보통 색입니다.
    let (icon, color) = if plan.device != ComputeDevice::Cpu {
        (Icon::Zap, palette.success_text)
    } else if plan.why.is_blocked() {
        (Icon::TriangleAlert, palette.warning_text)
    } else {
        (Icon::Cpu, palette.text_muted)
    };
    let note_color = if plan.why.is_blocked() {
        palette.warning_text
    } else {
        palette.text_muted
    };

    tile::panel(ui, |ui| {
        ui.set_min_height(min_height);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            row::title_line(ui, |ui| {
                ui.label(text::strong(&report.engine_name));
                if report.is_active {
                    controls::tone_badge(
                        ui,
                        &t!("models.local.badge.active"),
                        palette.success_soft,
                        palette.success_text,
                    );
                }
            });

            ui.add_space(2.0);
            ui.label(text::caption(ui, t!("models.accel.expected")));
            icon_line(
                ui,
                icon,
                color,
                text::strong(accel::device_label(&plan.device)).color(palette.text),
            );
            ui.label(text::caption(ui, &report.note).color(note_color));

            ui.add_space(2.0);
            row::separator(ui);
            let backend = report.accel.backend.map_or_else(
                || t!("models.accel.backend_none").into_owned(),
                str::to_owned,
            );
            accel_fact(ui, &t!("models.accel.backend"), &backend, palette.text);
            for (kind, names) in &report.accel.devices {
                if names.is_empty() {
                    let missing = t!("models.accel.not_found");
                    accel_fact(ui, &kind.label(), &missing, palette.text_faint);
                } else {
                    accel_fact(ui, &kind.label(), &names.join(", "), palette.text);
                }
            }
            accel_fact(
                ui,
                &t!("models.accel.setting"),
                &report.accel.request.label(),
                palette.text,
            );
        })
        .response
        .rect
        .height()
    })
}

/// 아이콘과 글을 나란히 놓습니다. 글이 길면 아이콘 오른쪽에서 줄을 바꿉니다.
fn icon_line(ui: &mut Ui, icon: Icon, icon_color: Color32, text: RichText) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        ui.label(
            crate::icons::text(icon)
                .size(size::BODY + 1.0)
                .color(icon_color),
        );
        ui.vertical(|ui| ui.label(text));
    });
}

/// 가속 장치 칸의 한 줄: 왼쪽에 항목 이름, 오른쪽에 값. 값이 길면 줄을 바꿉니다.
fn accel_fact(ui: &mut Ui, label: &str, value: &str, value_color: Color32) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.allocate_ui_with_layout(
            vec2(ACCEL_FACT_LABEL_WIDTH, 0.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(ACCEL_FACT_LABEL_WIDTH);
                ui.label(text::caption(ui, label));
            },
        );
        ui.vertical(|ui| {
            ui.label(RichText::new(value).size(size::CAPTION).color(value_color));
        });
    });
}

fn language_support_label(support: &LanguageSupport) -> String {
    match support {
        LanguageSupport::Any => t!("models.cap.languages_any").into_owned(),
        LanguageSupport::Only(codes) => {
            t!("models.cap.languages_count", count = codes.len()).into_owned()
        }
        LanguageSupport::AutoAnd(codes) => {
            t!("models.cap.languages_count_auto", count = codes.len()).into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::stt::{ModelAsset, ModelFile};

    #[test]
    fn next_profile_id_generates_unique_ids() {
        let existing = vec![
            ApiProfile {
                id: "openai".into(),
                ..ApiProfile::default()
            },
            ApiProfile {
                id: "openai-2".into(),
                ..ApiProfile::default()
            },
        ];
        assert_eq!(next_profile_id(&existing, "openai"), "openai-3");
        assert_eq!(next_profile_id(&existing, "groq"), "groq");
    }

    #[test]
    fn provider_switch_updates_untouched_defaults_only() {
        let mut p = profile_from_preset("openai", "o1".into(), "OpenAI".into());
        apply_provider_preset_switch(&mut p, "openai", "groq");
        assert_eq!(p.endpoint, "https://api.groq.com/openai/v1");
        assert_eq!(p.model, "whisper-large-v3-turbo");

        // 사용자가 엔드포인트를 직접 바꾼 뒤에는 프리셋 전환이 덮어쓰지 않습니다.
        p.endpoint = "https://my-proxy.example.com/v1".into();
        apply_provider_preset_switch(&mut p, "groq", "custom");
        assert_eq!(p.endpoint, "https://my-proxy.example.com/v1");
        assert_eq!(p.model, "whisper-1");
    }

    #[test]
    fn partial_bytes_detects_resumable_part_files() {
        let dir = tempfile::tempdir().unwrap();
        let model_path = dir.path().join("ggml-tiny.bin");
        fs::write(part_path(&model_path), b"1234567").unwrap();
        let variant = ModelVariant {
            id: "tiny".into(),
            name_key: "engine.whisper_local.models.tiny.name".into(),
            description_key: "engine.whisper_local.models.tiny.description".into(),
            recommended: false,
            files: vec![ModelFile {
                path: model_path,
                asset: ModelAsset {
                    id: "tiny".into(),
                    file_name: "ggml-tiny.bin".into(),
                    url: "https://example.com".into(),
                    size_bytes: 100,
                    sha256: "a".repeat(64),
                },
            }],
        };
        assert_eq!(partial_bytes(&variant), 7);
    }

    #[test]
    fn cloud_profile_readiness_checks_endpoint_model_and_required_key() {
        let mut openai = profile_from_preset("openai", "o1".into(), "OpenAI".into());
        assert!(!cloud_profile_ready(&openai));
        openai.api_key = "sk-test".into();
        assert!(cloud_profile_ready(&openai));

        let custom = profile_from_preset("custom", "c1".into(), "Custom".into());
        assert!(cloud_profile_ready(&custom));
    }
}
