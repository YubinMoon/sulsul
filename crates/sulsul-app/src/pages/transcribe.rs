//! 받아쓰기 테스트 화면: 녹음한 오디오나 WAV 파일을 엔진에 넣어 결과를 확인합니다.

mod job;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{Frame, Label, Margin, RichText, Stroke, Ui};
use rust_i18n::t;
use sulsul_core::CancelToken;
use sulsul_core::audio::{self, AudioClip};
use sulsul_core::config::Config;
use sulsul_core::stt::{ComputeDevice, EngineContext, EngineKind, Transcriber};

use self::job::{Failure, JobAudio, JobInput, JobOutput, Success};
use super::{Page, SECTION_GAP, header, language_items};
use crate::accel::{self, AccelEnv, EngineAccel};
use crate::error_text::{self, engine_name};
use crate::format::{elapsed_text, hertz_text, seconds_text};
use crate::i18n::tr;
use crate::icons::Icon;
use crate::session::{PageContext, Shared};
use crate::system;
use crate::theme::{Palette, radius};
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card::Section;
use crate::widgets::controls::{self, ComboItem};
use crate::widgets::row::{self, Row};
use crate::widgets::stat::{Stat, stats};
use crate::widgets::text;

/// 결과 카드로 화면을 옮길 때 카드 둘레에 남기는 여백
const REVEAL_MARGIN: f32 = 16.0;

/// 기록 목록에 보여 줄 결과 앞부분의 최대 글자 수 (칸이 좁으면 더 줄여서 보여 줍니다)
const PREVIEW_CHARS: usize = 80;

pub struct TranscribePage {
    source: Source,
    file: Option<LoadedFile>,
    file_error: Option<String>,
    /// 이 화면에서만 임시로 바꾼 엔진. `None`이면 설정 값을 따릅니다.
    engine_override: Option<String>,
    /// 이 화면에서만 임시로 바꾼 언어. `None`이면 설정 값을 따릅니다.
    language_override: Option<String>,
    running: Option<Running>,
    /// 엔진을 보관해 모델 로딩을 반복하지 않게 합니다. 작업 중에는 작업 스레드에 가 있고,
    /// 돌려받지 못했으면(`None`) 다음 작업 때 새로 만듭니다.
    transcriber: Option<Transcriber>,
    attempts: Vec<Attempt>,
    /// 결과 카드에 보여 줄 시도 (`attempts`의 위치)
    selected: Option<usize>,
    /// 방금 끝난 받아쓰기의 결과가 보이도록 화면을 옮겨야 하는지
    reveal_result: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Recording,
    File,
}

struct LoadedFile {
    path: PathBuf,
    clip: Arc<AudioClip>,
}

struct Running {
    output: Receiver<JobOutput>,
    /// 작업을 멈추는 신호. 취소 버튼이 붙으면 쓰입니다.
    #[allow(dead_code)]
    cancel: CancelToken,
    started: Instant,
    engine_id: String,
    language: String,
    source: String,
    audio: Duration,
}

/// 받아쓰기 시도 한 번의 기록
struct Attempt {
    number: usize,
    engine_name: String,
    language: String,
    source: String,
    audio: Duration,
    /// 버튼을 누른 뒤 결과가 나올 때까지 걸린 전체 시간
    total: Duration,
    outcome: Result<Success, String>,
}

impl Attempt {
    /// 실시간 배율(RTF) = 인식 시간 / 오디오 길이. 낮을수록 빠릅니다.
    fn rtf(&self) -> Option<f64> {
        let success = self.outcome.as_ref().ok()?;
        let audio = self.audio.as_secs_f64();
        (audio > 0.0).then(|| success.recognize.as_secs_f64() / audio)
    }
}

impl Default for TranscribePage {
    fn default() -> Self {
        Self {
            source: Source::Recording,
            file: None,
            file_error: None,
            engine_override: None,
            language_override: None,
            running: None,
            transcriber: None,
            attempts: Vec::new(),
            selected: None,
            reveal_result: false,
        }
    }
}

impl TranscribePage {
    /// 매 프레임 부릅니다. 작업 스레드가 끝났으면 결과를 기록합니다.
    pub fn tick(&mut self, shared: &Shared) {
        let Some(running) = &self.running else {
            return;
        };
        let (transcriber, result) = match running.output.try_recv() {
            Ok(output) => (Some(output.transcriber), output.result),
            Err(TryRecvError::Empty) => return,
            // 작업 스레드가 결과 없이 끝났습니다(엔진 내부 패닉 등).
            Err(TryRecvError::Disconnected) => (None, Err(Failure::Crashed)),
        };
        let Some(running) = self.running.take() else {
            return;
        };
        self.transcriber = transcriber;
        let outcome = result.map_err(|failure| match failure {
            Failure::Stt(error) => {
                tracing::warn!("transcription failed: {error}");
                error_text::stt(&error, &shared.registry)
            }
            Failure::Audio(error) => {
                tracing::warn!("could not prepare audio: {error}");
                error_text::audio(&error)
            }
            Failure::Crashed => {
                tracing::error!("transcription thread ended without a result");
                t!("transcribe.error.crashed").into_owned()
            }
        });
        self.attempts.push(Attempt {
            number: self.attempts.len() + 1,
            engine_name: engine_name(&shared.registry, &running.engine_id),
            language: running.language,
            source: running.source,
            audio: running.audio,
            total: running.started.elapsed(),
            outcome,
        });
        self.selected = Some(self.attempts.len() - 1);
        self.reveal_result = true;
    }

    fn engine_id(&self, config: &Config) -> String {
        self.engine_override
            .clone()
            .unwrap_or_else(|| config.stt.engine.clone())
    }

    fn language(&self, config: &Config) -> String {
        self.language_override
            .clone()
            .unwrap_or_else(|| config.stt.language.clone())
    }

    /// 선택된 소스의 오디오와 화면에 보일 이름
    fn audio(&self, shared: &Shared) -> Option<(JobAudio, String)> {
        match self.source {
            Source::Recording => shared.last_recording.as_ref().map(|recorded| {
                (
                    JobAudio::Recorded(Arc::clone(recorded)),
                    t!("transcribe.source.recording").into_owned(),
                )
            }),
            Source::File => self.file.as_ref().map(|file| {
                (
                    JobAudio::File(Arc::clone(&file.clip)),
                    file_name(&file.path),
                )
            }),
        }
    }

    fn start(&mut self, cx: &PageContext<'_>, ctx: &egui::Context) {
        let Some((audio, source)) = self.audio(cx.shared) else {
            return;
        };
        let engine_id = self.engine_id(cx.config);
        let language = self.language(cx.config);
        let duration = audio.clip().duration();
        let transcriber = self.transcriber.take().unwrap_or_else(|| {
            Transcriber::new(
                cx.shared.registry.clone(),
                EngineContext {
                    paths: cx.shared.paths.clone(),
                },
            )
        });
        let cancel = CancelToken::new();
        let input = JobInput {
            transcriber,
            stt: cx.config.stt.clone(),
            engine_id: engine_id.clone(),
            language: language.clone(),
            audio,
            cancel: cancel.clone(),
        };
        let output = job::spawn(input, ctx.clone());
        self.running = Some(Running {
            output,
            cancel,
            started: Instant::now(),
            engine_id,
            language,
            source,
            audio: duration,
        });
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        header(ui, &t!("transcribe.title"), &t!("transcribe.subtitle"));

        self.source_card(ui, cx);
        ui.add_space(SECTION_GAP);
        self.engine_card(ui, cx);
        if let Some(attempt) = self.selected.and_then(|index| self.attempts.get(index)) {
            ui.add_space(SECTION_GAP);
            let card = ui
                .scope(|ui| result_card(ui, cx.shared, attempt))
                .response
                .rect;
            if std::mem::take(&mut self.reveal_result) {
                ui.scroll_to_rect(card.expand(REVEAL_MARGIN), Some(egui::Align::Min));
            }
        }
        if !self.attempts.is_empty() {
            ui.add_space(SECTION_GAP);
            self.history_card(ui);
        }
    }

    fn source_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        Section::new(Icon::FileAudio, &t!("transcribe.source.title")).show(ui, |ui| {
            controls::segmented(
                ui,
                &mut self.source,
                &[
                    (
                        Source::Recording,
                        t!("transcribe.source.recording").into_owned(),
                    ),
                    (Source::File, t!("transcribe.source.file").into_owned()),
                ],
            );
            ui.add_space(4.0);

            match self.source {
                Source::Recording => match &cx.shared.last_recording {
                    Some(recorded) => {
                        ui.label(clip_summary(&recorded.clip));
                        ui.label(text::caption(ui, &recorded.device_name));
                    }
                    None => {
                        ui.label(text::muted(ui, t!("transcribe.source.no_recording")));
                        if controls::icon_text_button(
                            ui,
                            Icon::Mic,
                            t!("transcribe.source.go_record"),
                        )
                        .clicked()
                        {
                            cx.shared.navigate = Some(Page::Record);
                        }
                    }
                },
                Source::File => {
                    ui.horizontal(|ui| {
                        if controls::icon_text_button(
                            ui,
                            Icon::FolderOpen,
                            t!("transcribe.source.open_wav"),
                        )
                        .clicked()
                        {
                            self.open_wav(cx);
                        }
                        if let Some(file) = &self.file {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(text::strong(file_name(&file.path)));
                                ui.label(text::caption(ui, clip_summary(&file.clip)));
                            });
                        } else {
                            ui.label(text::muted(ui, t!("transcribe.source.no_file")));
                        }
                    });
                    if let Some(file) = &self.file {
                        ui.add(
                            Label::new(text::caption(ui, file.path.display().to_string()))
                                .truncate(),
                        );
                    }
                    if let Some(error) = &self.file_error {
                        ui.add_space(4.0);
                        Banner::new(Tone::Error).line(error).show(ui);
                    }
                }
            }
        });
    }

    fn open_wav(&mut self, cx: &mut PageContext<'_>) {
        let mut dialog = rfd::FileDialog::new()
            .set_parent(cx.frame)
            .set_title(t!("transcribe.source.open_dialog_title"))
            .add_filter(t!("common.wav_files"), &["wav"]);
        let recordings = cx.shared.paths.recordings_dir();
        if recordings.is_dir() {
            dialog = dialog.set_directory(&recordings);
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        match audio::read_wav(&path) {
            Ok(clip) if clip.is_empty() => {
                self.file_error = Some(t!("transcribe.source.empty_file").into_owned());
            }
            Ok(clip) => {
                tracing::info!("opened {}", path.display());
                self.file = Some(LoadedFile {
                    path,
                    clip: Arc::new(clip),
                });
                self.file_error = None;
            }
            Err(error) => {
                tracing::warn!("could not read WAV: {error}");
                self.file_error = Some(error_text::audio(&error));
            }
        }
    }

    fn engine_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        Section::new(Icon::Cpu, &t!("transcribe.engine.title")).show(ui, |ui| {
            let registry = &cx.shared.registry;
            let busy = self.running.is_some();
            let engine_id = self.engine_id(cx.config);
            let descriptor = registry.descriptor(&engine_id);

            ui.add_enabled_ui(!busy, |ui| {
                // 엔진
                let mut items = vec![ComboItem::new(
                    None,
                    t!(
                        "transcribe.follow_settings",
                        value = engine_name(registry, &cx.config.stt.engine)
                    ),
                )];
                items.extend(registry.descriptors().map(|descriptor| {
                    ComboItem::new(Some(descriptor.id.clone()), tr(&descriptor.name_key))
                }));
                let description = descriptor.map(|descriptor| tr(&descriptor.description_key));
                let unknown = descriptor
                    .is_none()
                    .then(|| t!("stt.engine.unknown", id = engine_id.as_str()).into_owned());
                Row::new(&t!("stt.engine.label"))
                    .help_opt(description.as_deref())
                    .warning_opt(unknown.as_deref())
                    .show(ui, |ui| {
                        controls::combo(
                            ui,
                            "transcribe_engine",
                            row::CONTROL_WIDTH,
                            &mut self.engine_override,
                            &items,
                            "",
                        );
                    });
                row::separator(ui);

                // 언어
                let mut items = vec![ComboItem::new(
                    None,
                    t!(
                        "transcribe.follow_settings",
                        value = language_label(&cx.config.stt.language)
                    ),
                )];
                items.extend(
                    language_items(&cx.config.stt.language)
                        .into_iter()
                        .map(|item| ComboItem::new(Some(item.value), item.label)),
                );
                Row::new(&t!("stt.language.label"))
                    .help(&t!("transcribe.temporary_help"))
                    .show(ui, |ui| {
                        controls::combo(
                            ui,
                            "transcribe_language",
                            row::CONTROL_WIDTH,
                            &mut self.language_override,
                            &items,
                            "",
                        );
                    });
            });

            if engine_id == sulsul_stt_cloud::ENGINE_ID
                || cx
                    .shared
                    .registry
                    .descriptor(&engine_id)
                    .is_some_and(|d| d.kind == EngineKind::Local)
            {
                row::separator(ui);
                active_setup_row(ui, cx.shared, cx.config, &engine_id);
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let ready = !busy && self.audio(cx.shared).is_some();
                let clicked = ui
                    .add_enabled_ui(ready, |ui| {
                        controls::primary_icon_button(ui, Icon::Play, t!("transcribe.run"))
                            .clicked()
                    })
                    .inner;
                if clicked {
                    let ctx = ui.ctx().clone();
                    self.start(cx, &ctx);
                }
                if let Some(running) = &self.running {
                    ui.spinner();
                    ui.label(text::muted(
                        ui,
                        t!(
                            "transcribe.running",
                            elapsed = seconds_text(running.started.elapsed())
                        ),
                    ));
                } else if self.audio(cx.shared).is_none() {
                    ui.label(text::caption(ui, t!("transcribe.need_audio")));
                }
            });
        });
    }

    fn history_card(&mut self, ui: &mut Ui) {
        Section::new(Icon::History, &t!("transcribe.history.title")).show(ui, |ui| {
            ui.label(text::caption(ui, t!("transcribe.history.help")));
            ui.add_space(4.0);
            let palette = Palette::of(ui);

            let header_height = ui.text_style_height(&egui::TextStyle::Small);
            history_row(ui, header_height, |ui, column| {
                let key = match column {
                    0 => "transcribe.history.number",
                    1 => "transcribe.history.engine",
                    2 => "transcribe.history.language",
                    3 => "transcribe.history.time",
                    4 => "transcribe.history.rtf",
                    _ => "transcribe.history.result",
                };
                ui.add(Label::new(text::caption(ui, tr(key))).truncate());
            });

            // 최근 시도를 위에
            let row_height = ui.spacing().interact_size.y;
            let mut clicked = None;
            for index in (0..self.attempts.len()).rev() {
                let attempt = &self.attempts[index];
                let selected = self.selected == Some(index);
                history_row(ui, row_height, |ui, column| match column {
                    0 => {
                        if ui
                            .selectable_label(selected, format!("#{}", attempt.number))
                            .clicked()
                        {
                            clicked = Some(index);
                        }
                    }
                    1 => {
                        ui.add(Label::new(&attempt.engine_name).truncate());
                    }
                    2 => {
                        ui.add(Label::new(language_label(&attempt.language)).truncate());
                    }
                    3 => {
                        ui.label(elapsed_text(attempt.total));
                    }
                    4 => {
                        ui.label(attempt.rtf().map_or_else(|| "–".to_owned(), rtf_text));
                    }
                    _ => {
                        let preview = match &attempt.outcome {
                            Ok(success) => RichText::new(preview(&success.text)),
                            Err(message) => {
                                RichText::new(preview(message)).color(palette.danger_text)
                            }
                        };
                        ui.add(Label::new(preview).truncate());
                    }
                });
            }
            if clicked.is_some() {
                self.selected = clicked;
            }
        });
    }
}

/// 기록 표의 칸 너비. 마지막 "결과" 칸은 남은 너비를 모두 씁니다.
const HISTORY_COLUMNS: [f32; 5] = [48.0, 140.0, 64.0, 76.0, 56.0];
const HISTORY_COLUMN_GAP: f32 = 14.0;

/// 기록 표의 한 줄. `add_cell`은 칸 번호(0부터)와 함께 칸마다 한 번씩 불립니다.
fn history_row(ui: &mut Ui, height: f32, mut add_cell: impl FnMut(&mut Ui, usize)) {
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.scope(|ui| {
        // 줄 높이를 정해 둡니다 (제목 줄은 낮게).
        ui.spacing_mut().interact_size.y = height;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = HISTORY_COLUMN_GAP;
            for (column, width) in HISTORY_COLUMNS.iter().enumerate() {
                ui.allocate_ui_with_layout(egui::vec2(*width, height), layout, |ui| {
                    ui.set_width(*width);
                    add_cell(ui, column);
                });
            }
            let rest = ui.available_width().max(40.0);
            ui.allocate_ui_with_layout(egui::vec2(rest, height), layout, |ui| {
                ui.set_width(rest);
                add_cell(ui, HISTORY_COLUMNS.len());
            });
        });
    });
}

fn result_card(ui: &mut Ui, shared: &mut Shared, attempt: &Attempt) {
    let title = t!("transcribe.result.title");
    let number = format!("#{}", attempt.number);
    Section::new(Icon::Text, &title)
        .badge(&number)
        .show(ui, |ui| match &attempt.outcome {
            Ok(success) => {
                let palette = Palette::of(ui);
                Frame::new()
                    .fill(palette.field)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(radius::CONTROL)
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if success.text.trim().is_empty() {
                            ui.label(text::muted(ui, t!("transcribe.result.empty")));
                        } else {
                            ui.add(Label::new(&success.text).selectable(true).wrap());
                        }
                    });
                ui.horizontal(|ui| {
                    if controls::icon_text_button(ui, Icon::Copy, t!("transcribe.result.copy"))
                        .clicked()
                    {
                        ui.ctx().copy_text(success.text.clone());
                        shared.toasts.info(t!("transcribe.result.copied"));
                    }
                    let mut details = vec![attempt.source.clone(), attempt.engine_name.clone()];
                    details.push(language_label(&attempt.language));
                    if let Some(detected) = &success.detected_language {
                        details.push(
                            t!("transcribe.result.detected", language = detected).into_owned(),
                        );
                    }
                    // 엔진이 보고한 실제 장치. 클라우드처럼 장치를 알 수 없으면 생략합니다.
                    if success.device != ComputeDevice::Unknown {
                        details.push(
                            t!(
                                "transcribe.result.device",
                                device = accel::device_label(&success.device)
                            )
                            .into_owned(),
                        );
                    }
                    ui.label(text::caption(ui, details.join(" · ")));
                });

                ui.add_space(8.0);
                let prepare = if success.reused {
                    Stat::new(
                        t!("transcribe.result.prepare"),
                        t!("transcribe.result.reused"),
                    )
                    .note(t!("transcribe.result.reused_note"))
                } else {
                    Stat::new(
                        t!("transcribe.result.prepare"),
                        elapsed_text(success.prepare),
                    )
                };
                let rtf = Stat::new(
                    t!("transcribe.result.rtf"),
                    attempt.rtf().map_or_else(|| "–".to_owned(), rtf_text),
                )
                .note(t!("transcribe.result.rtf_note"));
                stats(
                    ui,
                    &[
                        Stat::new(t!("transcribe.result.audio"), seconds_text(attempt.audio)),
                        prepare,
                        Stat::new(
                            t!("transcribe.result.recognize"),
                            elapsed_text(success.recognize),
                        ),
                        rtf,
                    ],
                );
            }
            Err(message) => {
                Banner::new(Tone::Error)
                    .title(&t!("transcribe.result.failed"))
                    .line(message)
                    .line(format!(
                        "{} · {}",
                        attempt.engine_name,
                        language_label(&attempt.language)
                    ))
                    .show(ui);
            }
        });
}

/// 로컬 모델(Whisper·SenseVoice 등) 또는 클라우드 API의 현재 준비 상태와 모델 관리 이동 버튼
fn active_setup_row(ui: &mut Ui, shared: &mut Shared, config: &Config, engine_id: &str) {
    let palette = Palette::of(ui);
    Row::new(&t!("transcribe.model.label")).show_below(ui, |ui| {
        if engine_id == sulsul_stt_cloud::ENGINE_ID {
            let profile = config.stt.active_api_profile();
            let ready = profile.is_some_and(|p| {
                sulsul_stt_cloud::normalize_transcriptions_url(&p.endpoint).is_ok()
                    && !p.model.trim().is_empty()
                    && (!sulsul_stt_cloud::find_preset(&p.provider)
                        .is_some_and(|preset| preset.requires_api_key)
                        || !p.api_key.trim().is_empty())
            });
            status_badge(ui, &palette, ready);
            match profile {
                Some(p) => {
                    let name = if p.name.trim().is_empty() {
                        p.id.as_str()
                    } else {
                        p.name.as_str()
                    };
                    let model = if p.model.trim().is_empty() {
                        "-"
                    } else {
                        p.model.as_str()
                    };
                    ui.label(text::muted(
                        ui,
                        t!(
                            "transcribe.model.cloud_profile",
                            profile = name,
                            model = model
                        ),
                    ));
                }
                None => {
                    ui.label(
                        text::caption(ui, t!("transcribe.model.cloud_missing"))
                            .color(palette.warning_text),
                    );
                }
            }
            if controls::icon_text_button(ui, Icon::Boxes, t!("transcribe.model.manage")).clicked()
            {
                shared.navigate = Some(Page::Models);
            }
            return;
        }

        let engine_ctx = EngineContext {
            paths: shared.paths.clone(),
        };
        let req = shared
            .registry
            .model(&engine_ctx, &config.stt, engine_id)
            .ok()
            .flatten();
        let exists = req.as_ref().is_some_and(|r| r.is_installed());
        status_badge(ui, &palette, exists);
        match &req {
            Some(r) => ui.add(text::path_label(ui, &r.path)),
            None => ui.label(text::muted(ui, t!("transcribe.model.unknown"))),
        };
        // 지금 설정으로 쓸 것으로 예상하는 장치. 실제로 쓴 장치는 결과 카드에 나옵니다.
        if let Some(report) = EngineAccel::of(
            &shared.registry,
            &config.stt,
            engine_id,
            AccelEnv::current(),
        ) {
            ui.label(text::caption(
                ui,
                t!(
                    "transcribe.model.expected_device",
                    device = accel::device_label(&report.plan.device)
                ),
            ));
        }
        let folder = req
            .as_ref()
            .and_then(|r| r.path.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| shared.paths.models_dir());
        ui.horizontal(|ui| {
            if controls::icon_text_button(ui, Icon::FolderOpen, t!("transcribe.model.open_folder"))
                .clicked()
                && let Err(message) = system::open_dir(&folder)
            {
                shared
                    .toasts
                    .error(t!("common.open_failed", message = message));
            }
            if controls::icon_text_button(ui, Icon::Boxes, t!("transcribe.model.manage")).clicked()
            {
                shared.navigate = Some(Page::Models);
            }
        });
    });
}

fn status_badge(ui: &mut Ui, palette: &Palette, ready: bool) {
    if ready {
        controls::tone_badge(
            ui,
            &t!("transcribe.model.found"),
            palette.success_soft,
            palette.success_text,
        );
    } else {
        controls::tone_badge(
            ui,
            &t!("transcribe.model.missing"),
            palette.warning_soft,
            palette.warning_text,
        );
    }
}

fn clip_summary(clip: &AudioClip) -> String {
    t!(
        "transcribe.source.summary",
        duration = seconds_text(clip.duration()),
        rate = hertz_text(clip.sample_rate)
    )
    .into_owned()
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn language_label(code: &str) -> String {
    language_items(code)
        .into_iter()
        .find(|item| item.value == code)
        .map_or_else(|| code.to_owned(), |item| item.label)
}

fn rtf_text(rtf: f64) -> String {
    format!("{rtf:.3}")
}

/// 한 줄로 줄인 결과 앞부분
fn preview(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= PREVIEW_CHARS {
        flat
    } else {
        let head: String = flat.chars().take(PREVIEW_CHARS).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_flattens_and_truncates() {
        assert_eq!(preview("안녕하세요\n  반갑습니다"), "안녕하세요 반갑습니다");
        let long = "가".repeat(PREVIEW_CHARS + 5);
        let short = preview(&long);
        assert_eq!(short.chars().count(), PREVIEW_CHARS + 1);
        assert!(short.ends_with('…'));
    }
}
