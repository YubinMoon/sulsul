//! 2단계 받아쓰기: 1단계에서 정리한 녹음(또는 WAV 파일)을 설정의 엔진과 언어로 받아씁니다.
//! WAV 파일은 녹음이 아니므로 정리(음량 조절, 무음 자르기)하지 않고 그대로 받아씁니다.
//!
//! 엔진은 단축키 받아쓰기와 함께 쓰는 [`SharedTranscriber`](crate::transcriber::SharedTranscriber)에
//! 보관되므로, 여기서 한 번 올린 모델을 단축키 받아쓰기가 그대로 씁니다(그 반대도 마찬가지).

mod job;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{Label, Ui};
use rust_i18n::t;
use sulsul_core::CancelToken;
use sulsul_core::audio::{self, AudioClip};
use sulsul_core::config::Config;
use sulsul_core::stt::{ComputeDevice, EngineContext, EngineKind, SttError};

use self::job::{Failure, JobInput, JobOutput, Success};
use super::record::Recorded;
use super::{Gate, copy_button, output_box, progress_row};
use crate::accel::{self, AccelEnv, EngineAccel};
use crate::error_text::{self, engine_name};
use crate::format::{elapsed_text, hertz_text, seconds_text};
use crate::icons::Icon;
use crate::pages::Page;
use crate::session::{PageContext, Shared};
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card::Section;
use crate::widgets::language::language_name;
use crate::widgets::row::{self, Row};
use crate::widgets::stat::{Stat, stats};
use crate::widgets::{controls, text};

#[derive(Default)]
pub struct SttCard {
    /// 1단계에서 마지막으로 정리한 녹음
    recording: Option<Arc<Recorded>>,
    file: Option<LoadedFile>,
    /// 녹음 대신 WAV 파일을 받아쓰는지
    use_file: bool,
    file_error: Option<String>,
    running: Option<Running>,
    result: Option<Attempt>,
    /// 다음 카드에 아직 넘기지 않은 새 받아쓰기 결과
    fresh: bool,
}

struct LoadedFile {
    path: PathBuf,
    clip: Arc<AudioClip>,
}

struct Running {
    output: Receiver<JobOutput>,
    cancel: CancelToken,
    started: Instant,
    engine_id: String,
    audio: Duration,
}

impl Drop for Running {
    /// 결과를 받지 않고 버리면(앱을 끝낼 때 등) 작업도 멈춥니다.
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// 끝난 받아쓰기 한 번
struct Attempt {
    engine_name: String,
    audio: Duration,
    outcome: Outcome,
}

enum Outcome {
    Done(Success),
    Cancelled,
    Failed(String),
}

impl SttCard {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// 1단계에서 새로 정리한 녹음을 받아쓸 오디오로 고릅니다. 이전 결과는 다른 오디오의 것이라 지웁니다.
    pub fn use_recording(&mut self, recorded: Arc<Recorded>) {
        self.recording = Some(recorded);
        self.use_file = false;
        self.result = None;
    }

    /// 지난번에 부른 뒤로 받아쓰기에 성공했으면 그 글을 돌려줍니다.
    pub fn take_new_text(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.fresh) {
            return None;
        }
        match &self.result {
            Some(Attempt {
                outcome: Outcome::Done(success),
                ..
            }) => Some(success.text.trim().to_owned()),
            _ => None,
        }
    }

    /// 매 프레임 부릅니다(다른 화면을 보고 있어도). 작업 스레드가 끝났으면 결과를 기록합니다.
    pub fn tick(&mut self, shared: &Shared) {
        let Some(running) = &self.running else {
            return;
        };
        let result = match running.output.try_recv() {
            Ok(output) => output.result,
            Err(TryRecvError::Empty) => return,
            // 작업 스레드가 결과 없이 끝났습니다(엔진 내부 패닉 등).
            Err(TryRecvError::Disconnected) => Err(Failure::Crashed),
        };
        let Some(running) = self.running.take() else {
            return;
        };
        let outcome = match result {
            Ok(success) => {
                self.fresh = true;
                Outcome::Done(success)
            }
            Err(Failure::Stt(SttError::Cancelled)) => Outcome::Cancelled,
            Err(Failure::Stt(error)) => {
                tracing::warn!("test transcription failed: {error}");
                Outcome::Failed(error_text::stt(&error, &shared.registry))
            }
            Err(Failure::Audio(error)) => {
                tracing::warn!("could not prepare audio: {error}");
                Outcome::Failed(error_text::audio(&error))
            }
            Err(Failure::Crashed) => {
                tracing::error!("transcription thread ended without a result");
                Outcome::Failed(t!("transcribe.error.crashed").into_owned())
            }
        };
        self.result = Some(Attempt {
            engine_name: engine_name(&shared.registry, &running.engine_id),
            audio: running.audio,
            outcome,
        });
    }

    /// 지금 받아쓸 오디오와 그 이름
    fn audio(&self) -> Option<(Arc<AudioClip>, String)> {
        if self.use_file {
            self.file
                .as_ref()
                .map(|file| (Arc::clone(&file.clip), file_name(&file.path)))
        } else {
            self.recording.as_ref().map(|recorded| {
                (
                    Arc::clone(&recorded.clip),
                    t!("test.stt.recording").into_owned(),
                )
            })
        }
    }

    fn start(&mut self, cx: &PageContext<'_>, ctx: &egui::Context) {
        let Some((audio, _)) = self.audio() else {
            return;
        };
        let engine_id = cx.config.stt.engine.clone();
        let duration = audio.duration();
        let cancel = CancelToken::new();
        let input = JobInput {
            transcriber: cx.shared.transcriber.clone(),
            stt: cx.config.stt.clone(),
            engine_id: engine_id.clone(),
            audio,
            cancel: cancel.clone(),
        };
        self.result = None;
        self.running = Some(Running {
            output: job::spawn(input, ctx.clone()),
            cancel,
            started: Instant::now(),
            engine_id,
            audio: duration,
        });
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, gate: Gate) {
        Section::new(Icon::Captions, &t!("test.stt.title")).show(ui, |ui| {
            // 받아쓰는 동안에는 결과와 입력이 엇갈리지 않게 입력을 바꾸지 않습니다.
            let can_change = self.running.is_none();
            self.input_row(ui, cx, can_change);
            row::separator(ui);
            engine_row(ui, cx.shared, cx.config);

            ui.add_space(10.0);
            self.action_row(ui, cx, gate);

            if let Some(attempt) = &self.result {
                ui.add_space(10.0);
                result_ui(ui, cx.shared, attempt);
            }
        });
    }

    fn input_row(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, can_change: bool) {
        let summary = match self.audio() {
            Some((clip, name)) => format!("{name} · {}", clip_summary(&clip)),
            None => t!("test.stt.no_audio").into_owned(),
        };
        let mut open = false;
        let mut back = false;
        Row::new(&t!("test.stt.input"))
            .help(&summary)
            .warning_opt(self.file_error.as_deref())
            .show(ui, |ui| {
                ui.add_enabled_ui(can_change, |ui| {
                    open =
                        controls::icon_text_button(ui, Icon::FolderOpen, t!("test.stt.open_wav"))
                            .clicked();
                    if self.use_file && self.recording.is_some() {
                        // 좁은 창에서도 한 줄에 들어가게 아이콘만 둡니다 (이름은 툴팁).
                        back =
                            controls::icon_button(ui, Icon::Undo2, &t!("test.stt.use_recording"))
                                .clicked();
                    }
                });
            });
        if open {
            self.open_wav(cx);
        }
        if back {
            self.use_file = false;
            self.result = None;
        }
    }

    fn open_wav(&mut self, cx: &mut PageContext<'_>) {
        let mut dialog = rfd::FileDialog::new()
            .set_parent(cx.frame)
            .set_title(t!("test.stt.open_dialog_title"))
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
                self.file_error = Some(t!("test.stt.empty_file").into_owned());
            }
            Ok(clip) => {
                tracing::info!("opened {}", path.display());
                self.file = Some(LoadedFile {
                    path,
                    clip: Arc::new(clip),
                });
                self.use_file = true;
                self.file_error = None;
                self.result = None;
            }
            Err(error) => {
                tracing::warn!("could not read WAV: {error}");
                self.file_error = Some(error_text::audio(&error));
            }
        }
    }

    fn action_row(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, gate: Gate) {
        if let Some(running) = &self.running {
            if progress_row(ui, running.started, running.cancel.is_cancelled()) {
                running.cancel.cancel();
            }
            if super::dictation_busy(cx.shared.dictation.state) {
                ui.label(text::caption(ui, t!("test.dictation_waits")));
            }
            return;
        }
        let has_audio = self.audio().is_some();
        let reason = gate
            .reason()
            .or_else(|| (!has_audio).then(|| t!("test.stt.need_audio").into_owned()));
        ui.horizontal(|ui| {
            let clicked = ui
                .add_enabled_ui(reason.is_none(), |ui| {
                    controls::primary_icon_button(ui, Icon::Play, t!("test.stt.run")).clicked()
                })
                .inner;
            if clicked {
                let ctx = ui.ctx().clone();
                self.start(cx, &ctx);
            }
            if let Some(reason) = &reason {
                // 이유가 길면(모델 안내 등) 버튼 옆 남은 너비에서 줄을 바꿉니다.
                ui.add(Label::new(text::caption(ui, reason)).wrap());
            }
        });
    }
}

/// 받아쓸 엔진: 이름, 언어, 예상 장치, 준비되지 않았으면 그 이유. 엔진은 모델 관리 화면에서 고릅니다.
fn engine_row(ui: &mut Ui, shared: &mut Shared, config: &Config) {
    let registry = &shared.registry;
    let engine_id = config.stt.engine.as_str();
    let mut details = Vec::new();
    if let Ok(language) = registry.resolve_language(&config.stt, engine_id, None) {
        details.push(language_name(language.as_config()));
    }
    // 지금 설정으로 쓸 것으로 예상하는 장치. 실제로 쓴 장치는 결과에 나옵니다.
    if let Some(report) = EngineAccel::of(registry, &config.stt, engine_id, AccelEnv::current()) {
        details.push(
            t!(
                "test.stt.expected_device",
                device = accel::device_label(&report.plan.device)
            )
            .into_owned(),
        );
    }
    details.insert(0, engine_name(registry, engine_id));
    let details = details.join(" · ");
    let warning = setup_warning(shared, config, engine_id);
    let manage = Row::new(&t!("test.stt.engine"))
        .help(&details)
        .warning_opt(warning.as_deref())
        .show(ui, |ui| {
            controls::icon_text_button(ui, Icon::Boxes, t!("test.manage_models")).clicked()
        });
    if manage {
        shared.navigate = Some(Page::Models);
    }
}

/// 엔진을 바로 쓸 수 없어 보이는 이유 (모르는 엔진, 내려받지 않은 모델, 덜 채운 API 프로필).
/// 받아쓰기를 막지는 않고 미리 알리기만 합니다. 실제로 실패하면 결과에 정확한 원인이 나옵니다.
fn setup_warning(shared: &Shared, config: &Config, engine_id: &str) -> Option<String> {
    let registry = &shared.registry;
    let Some(descriptor) = registry.descriptor(engine_id) else {
        return Some(t!("stt.engine.unknown", id = engine_id).into_owned());
    };
    if engine_id == sulsul_stt_cloud::ENGINE_ID {
        let ready = config.stt.active_api_profile().is_some_and(|profile| {
            sulsul_stt_cloud::normalize_transcriptions_url(&profile.endpoint).is_ok()
                && !profile.model.trim().is_empty()
                && (!sulsul_stt_cloud::requires_api_key(&profile.provider, &profile.endpoint)
                    || !profile.api_key.trim().is_empty())
        });
        return (!ready).then(|| t!("test.stt.cloud_not_ready").into_owned());
    }
    if descriptor.kind != EngineKind::Local {
        return None;
    }
    let engine_ctx = EngineContext {
        paths: shared.paths.clone(),
    };
    let installed = registry
        .model(&engine_ctx, &config.stt, engine_id)
        .ok()
        .flatten()
        .is_none_or(|model| model.is_installed());
    (!installed).then(|| t!("test.stt.model_missing").into_owned())
}

fn result_ui(ui: &mut Ui, shared: &mut Shared, attempt: &Attempt) {
    let success = match &attempt.outcome {
        Outcome::Done(success) => success,
        Outcome::Cancelled => {
            Banner::new(Tone::Info).line(t!("test.cancelled")).show(ui);
            return;
        }
        Outcome::Failed(message) => {
            Banner::new(Tone::Error)
                .title(&t!("test.stt.failed"))
                .line(message)
                .show(ui);
            return;
        }
    };

    output_box(ui, &success.text, &t!("test.stt.empty"));
    ui.horizontal_wrapped(|ui| {
        copy_button(ui, shared, &success.text);
        let mut details = vec![
            attempt.engine_name.clone(),
            language_name(&success.language),
        ];
        if let Some(detected) = &success.detected_language {
            details.push(t!("test.stt.detected", language = detected).into_owned());
        }
        // 엔진이 보고한 실제 장치. 클라우드처럼 장치를 알 수 없으면 뺍니다.
        if success.device != ComputeDevice::Unknown {
            details.push(
                t!(
                    "test.stt.device",
                    device = accel::device_label(&success.device)
                )
                .into_owned(),
            );
        }
        ui.label(text::caption(ui, details.join(" · ")));
    });

    ui.add_space(8.0);
    let prepare = if success.reused {
        Stat::new(t!("test.stt.prepare"), t!("test.stt.reused"))
    } else {
        Stat::new(t!("test.stt.prepare"), elapsed_text(success.prepare))
    };
    let mut recognize = Stat::new(t!("test.stt.recognize"), elapsed_text(success.recognize));
    if let Some(rtf) = rtf(success.recognize, attempt.audio) {
        recognize = recognize.note(t!("test.stt.rtf", value = format!("{rtf:.2}")));
    }
    stats(
        ui,
        &[
            Stat::new(t!("test.stt.audio"), seconds_text(attempt.audio)),
            prepare,
            recognize,
        ],
    );
}

/// 실시간 배율(RTF) = 인식 시간 / 오디오 길이. 낮을수록 빠릅니다.
fn rtf(recognize: Duration, audio: Duration) -> Option<f64> {
    let audio = audio.as_secs_f64();
    (audio > 0.0).then(|| recognize.as_secs_f64() / audio)
}

fn clip_summary(clip: &AudioClip) -> String {
    t!(
        "test.stt.summary",
        duration = seconds_text(clip.duration()),
        rate = hertz_text(clip.sample_rate)
    )
    .into_owned()
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtf_is_recognition_time_over_audio_length() {
        let value = rtf(Duration::from_millis(500), Duration::from_secs(2)).unwrap();
        assert!((value - 0.25).abs() < 1e-9, "{value}");
        assert_eq!(rtf(Duration::from_secs(1), Duration::ZERO), None);
    }
}
