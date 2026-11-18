//! 1단계 녹음: 마이크 입력을 확인하고, 단축키 받아쓰기와 같은 오디오 처리를 거친 녹음을 만듭니다.
//!
//! 입력 장치는 설정 화면에서 고르고, 여기서는 지금 고른 장치를 알려 주기만 합니다.
//!
//! 녹음을 끝내면 작업 스레드에서 단축키 받아쓰기(`crate::dictation`)와 같은 순서로 늘 정리합니다.
//! 키우기 전의 녹음에서 말소리를 찾고, 녹음 전체로 음량을 맞춘 뒤, 말소리 바깥을 잘라 냅니다.
//! 단축키 받아쓰기는 말소리가 없으면 받아쓰지 않지만, 여기서는 무엇이 녹음됐는지 볼 수 있게
//! 녹음을 남기고 경고만 보여 줍니다.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{Align, Align2, Layout, Response, RichText, Role, Sense, Stroke, Ui, WidgetInfo, vec2};
use rust_i18n::t;
use sulsul_core::audio::{self, AudioClip, AudioError, AutoGain, Recording, RecordingOptions};
use sulsul_core::config::{Config, MAX_RECORD_SECONDS};
use sulsul_core::stt::WHISPER_SAMPLE_RATE;

use super::Gate;
use crate::error_text;
use crate::format::{SILENCE_DBFS, clock_text, clock_text_short, dbfs, dbfs_text, seconds_text};
use crate::i18n::microphone_settings_hint;
use crate::icons::{self, Icon};
use crate::pages::Page;
use crate::session::{PageContext, Shared};
use crate::system;
use crate::theme::{self, Palette, size};
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card::Section;
use crate::widgets::device_combo::device_problem;
use crate::widgets::level_meter::{MeterState, level_meter};
use crate::widgets::stat::{Stat, stats};
use crate::widgets::{controls, row, text};

/// 장치가 데이터를 보내지 않아도 녹음이 끝나도록 두는 실제 시간 여유
const STALL_GRACE: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct RecordCard {
    active: Option<Active>,
    /// 녹음을 정리하는 작업 스레드의 결과
    cleaning: Option<Receiver<Recorded>>,
    problem: Option<Problem>,
    /// 마지막으로 정리를 마친 녹음
    recorded: Option<Arc<Recorded>>,
    /// 새 녹음이 생겼는데 아직 [`Self::take_new`]로 넘기지 않았는지
    fresh: bool,
}

struct Active {
    recording: Recording,
    meter: MeterState,
    max: Duration,
    started: Instant,
}

enum Problem {
    Audio(AudioError),
    /// 녹음은 끝났지만 받은 소리가 하나도 없음
    Empty,
    /// 녹음을 정리하는 작업이 결과 없이 끝남
    Crashed,
}

/// 정리를 마친 녹음
pub struct Recorded {
    /// 받아쓰기에 넘기는 소리 (음량을 맞추고 앞뒤를 자른 것)
    pub clip: Arc<AudioClip>,
    pub device_name: String,
    /// 최대 녹음 길이에 닿아 자동으로 끝났는지
    pub auto_stopped: bool,
    /// 정리하기 전 녹음 길이
    pub raw_duration: Duration,
    /// 자동 음량 조절 결과
    pub gain: AutoGain,
    /// 앞뒤 조용한 부분을 자른 결과
    pub trim: SilenceTrim,
}

/// 녹음 앞뒤의 조용한 부분을 자른 결과
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SilenceTrim {
    /// 이만큼 잘라 냈습니다 (0일 수도 있음)
    Trimmed(Duration),
    /// 말소리를 찾지 못해 자르지 않았습니다. 단축키 받아쓰기라면 받아쓰지 않았을 녹음입니다.
    NoSpeech,
    /// 말소리를 판정하지 못해 자르지 않았습니다 (단축키 받아쓰기도 자르지 않고 받아씀).
    Unchecked,
}

impl RecordCard {
    pub fn is_recording(&self) -> bool {
        self.active.is_some()
    }

    /// 녹음 중이거나 녹음을 정리하는 중인지
    pub fn is_busy(&self) -> bool {
        self.active.is_some() || self.cleaning.is_some()
    }

    /// 지난번에 부른 뒤로 새 녹음이 생겼으면 그 녹음을 돌려줍니다.
    pub fn take_new(&mut self) -> Option<Arc<Recorded>> {
        if std::mem::take(&mut self.fresh) {
            self.recorded.clone()
        } else {
            None
        }
    }

    /// 매 프레임 부릅니다(화면이 보이지 않아도). 최대 길이에 닿으면 녹음을 끝내고, 정리 결과를 챙깁니다.
    pub fn tick(&mut self, ctx: &egui::Context) {
        self.collect_cleaned();
        let Some(active) = &self.active else {
            return;
        };
        if active.recording.limit_reached() {
            self.stop(ctx, true);
        } else if let Some(message) = active.recording.error() {
            self.stop(ctx, false);
            // 일부라도 녹음됐으면 결과는 남기되, 중간에 끊긴 이유를 함께 알립니다.
            self.problem = Some(Problem::Audio(AudioError::Stream(message)));
        } else if active.started.elapsed() > active.max + STALL_GRACE {
            self.stop(ctx, true);
        } else {
            // 미터와 시계가 계속 움직이도록
            ctx.request_repaint();
        }
    }

    fn collect_cleaned(&mut self) {
        let Some(cleaning) = &self.cleaning else {
            return;
        };
        match cleaning.try_recv() {
            Ok(recorded) => {
                self.recorded = Some(Arc::new(recorded));
                self.fresh = true;
            }
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                tracing::error!("the recording cleanup thread ended without a result");
                self.problem = Some(Problem::Crashed);
            }
        }
        self.cleaning = None;
    }

    fn start(&mut self, config: &Config) {
        self.problem = None;
        let max = max_duration(config);
        let options = RecordingOptions {
            device: config.audio.input_device.clone(),
            max_duration: Some(max),
        };
        match Recording::start(&options) {
            Ok(recording) => {
                self.active = Some(Active {
                    recording,
                    meter: MeterState::default(),
                    max,
                    started: Instant::now(),
                });
            }
            Err(error) => {
                tracing::warn!("could not start recording: {error}");
                self.problem = Some(Problem::Audio(error));
            }
        }
    }

    fn stop(&mut self, ctx: &egui::Context, auto_stopped: bool) {
        let Some(active) = self.active.take() else {
            return;
        };
        let device_name = active.recording.device_name().to_owned();
        match active.recording.stop() {
            Ok(clip) if clip.is_empty() => self.problem = Some(Problem::Empty),
            Ok(clip) => {
                tracing::info!(
                    seconds = clip.duration().as_secs_f64(),
                    sample_rate = clip.sample_rate,
                    "test recording finished"
                );
                self.cleaning = Some(spawn_cleanup(ctx.clone(), move || {
                    let mut recorded = clean(clip);
                    recorded.device_name = device_name;
                    recorded.auto_stopped = auto_stopped;
                    recorded
                }));
            }
            Err(error) => {
                tracing::warn!("recording failed: {error}");
                self.problem = Some(Problem::Audio(error));
            }
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, gate: Gate) {
        Section::new(Icon::Mic, &t!("test.record.title")).show(ui, |ui| {
            self.recorder_ui(ui, cx, gate);

            if let Some(problem) = &self.problem {
                ui.add_space(10.0);
                match problem {
                    Problem::Audio(error) => Banner::new(Tone::Error)
                        .title(&t!("test.record.error_title"))
                        .line(error_text::audio(error))
                        .show(ui),
                    Problem::Empty => Banner::new(Tone::Warning)
                        .line(t!("test.record.empty"))
                        .show(ui),
                    Problem::Crashed => Banner::new(Tone::Error)
                        .line(t!("test.record.crashed"))
                        .show(ui),
                }
            }

            if let Some(recorded) = self.recorded.clone() {
                row::separator(ui);
                result_ui(ui, cx, &recorded);
            }

            row::separator(ui);
            device_hint(ui, cx.shared, cx.config);
        });
    }

    fn recorder_ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, gate: Gate) {
        let recording = self.is_recording();
        let dt = ui.input(|input| input.stable_dt);
        if let Some(active) = &mut self.active {
            active.meter.update(active.recording.level(), dt);
        }
        // 녹음 중이면 언제든 멈출 수 있고, 새로 시작하는 것은 다른 단계가 쉬고 있을 때만 합니다.
        let can_press = recording || (self.cleaning.is_none() && gate.is_open());

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            let label = if recording {
                t!("test.record.stop")
            } else {
                t!("test.record.start")
            };
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let button = ui
                    .add_enabled_ui(can_press, |ui| record_button(ui, recording, &label))
                    .inner;
                ui.allocate_ui_with_layout(
                    vec2(button.rect.width(), 18.0),
                    Layout::top_down(Align::Center),
                    |ui| {
                        ui.label(text::caption(ui, label.as_ref()));
                    },
                );
                if button.clicked() {
                    if recording {
                        let ctx = ui.ctx().clone();
                        self.stop(&ctx, false);
                    } else {
                        self.start(cx.config);
                    }
                }
            });

            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                self.status_ui(ui, cx.config);
                ui.add_space(8.0);
                let idle_meter = MeterState::default();
                let meter = self.active.as_ref().map_or(&idle_meter, |a| &a.meter);
                level_meter(ui, meter, self.active.is_some());
                let readout = if self.active.is_some() {
                    t!(
                        "test.record.level",
                        level = dbfs_text(meter.level_dbfs()),
                        peak = dbfs_text(meter.peak_dbfs())
                    )
                } else if let Some(reason) = gate.reason().filter(|_| !can_press) {
                    reason.into()
                } else {
                    t!("test.record.level_idle")
                };
                ui.label(text::caption(ui, readout));
            });
        });
    }

    fn status_ui(&self, ui: &mut Ui, config: &Config) {
        let palette = Palette::of(ui);
        if let Some(active) = &self.active {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                let pulse = (ui.input(|input| input.time) * 3.0).sin() as f32 * 0.5 + 0.5;
                ui.painter().circle_filled(
                    dot.center(),
                    4.0,
                    theme::mix(palette.danger_soft, palette.danger, 0.4 + 0.6 * pulse),
                );
                ui.label(text::strong(t!("test.record.recording")).color(palette.danger_text));
            });
            ui.label(
                RichText::new(clock_text(active.recording.elapsed()))
                    .font(theme::semibold(size::DISPLAY)),
            );
            ui.label(text::caption(
                ui,
                t!(
                    "test.record.limit",
                    max = clock_text_short(active.max),
                    device = active.recording.device_name()
                ),
            ));
            return;
        }

        if self.cleaning.is_some() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.spinner();
                ui.label(text::strong(t!("test.record.cleaning")).color(palette.text_muted));
            });
        } else {
            ui.label(text::strong(t!("test.record.idle")).color(palette.text_muted));
        }
        let shown = self
            .recorded
            .as_ref()
            .map_or(Duration::ZERO, |recorded| recorded.raw_duration);
        ui.label(
            RichText::new(clock_text(shown))
                .font(theme::semibold(size::DISPLAY))
                .color(palette.text_faint),
        );
        ui.label(text::caption(
            ui,
            t!(
                "test.record.limit_idle",
                max = clock_text_short(max_duration(config))
            ),
        ));
    }
}

/// 설정의 최대 녹음 길이 (허용 범위로 맞춤)
fn max_duration(config: &Config) -> Duration {
    let (min, max) = MAX_RECORD_SECONDS;
    let seconds = i64::from(config.audio.max_record_seconds).clamp(min, max);
    Duration::from_secs(seconds.unsigned_abs())
}

/// 녹음을 정리하는 작업 스레드를 시작합니다. 끝나면 결과를 보내고 화면을 다시 그리게 합니다.
fn spawn_cleanup(
    ctx: egui::Context,
    work: impl FnOnce() -> Recorded + Send + 'static,
) -> Receiver<Recorded> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("sulsul-test-cleanup".into())
        .spawn(move || {
            // 받는 쪽이 사라졌다면 앱이 끝나는 중이므로 결과를 버립니다.
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    if let Err(error) = spawned {
        // 보내는 쪽이 버려져서 받는 쪽은 "결과 없이 끝남"으로 처리합니다.
        tracing::error!("could not start the recording cleanup thread: {error}");
    }
    rx
}

/// 녹음을 단축키 받아쓰기와 같은 순서로 정리합니다 (장치 이름 등은 부르는 쪽이 채움).
fn clean(mut clip: AudioClip) -> Recorded {
    let raw_duration = clip.duration();
    // 말소리는 키우기 전의 녹음에서 찾고, 배경 소음을 잘 재도록 자르기 전의 녹음 전체로 음량을 맞춥니다.
    let speech = sulsul_vad::find_speech(&clip).inspect_err(|error| {
        tracing::warn!("could not check the test recording for speech: {error}");
    });
    let gain = clip.auto_gain();
    let trim = match speech {
        Ok(Some(speech)) => {
            clip.keep_range(speech.range);
            SilenceTrim::Trimmed(raw_duration.saturating_sub(clip.duration()))
        }
        Ok(None) => SilenceTrim::NoSpeech,
        Err(_) => SilenceTrim::Unchecked,
    };
    Recorded {
        clip: Arc::new(clip),
        device_name: String::new(),
        auto_stopped: false,
        raw_duration,
        gain,
        trim,
    }
}

/// 크고 둥근 녹음 시작/정지 버튼
fn record_button(ui: &mut Ui, recording: bool, label: &str) -> Response {
    let diameter = 76.0;
    let margin = 8.0;
    let (rect, response) = ui.allocate_exact_size(
        vec2(diameter + margin * 2.0, diameter + margin * 2.0),
        Sense::click(),
    );
    let enabled = ui.is_enabled();
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));

    if ui.is_rect_visible(rect) {
        let palette = Palette::of(ui);
        let center = rect.center();
        let radius = diameter / 2.0;
        let (fill, hover, halo) = if !enabled {
            (palette.control, palette.control, palette.control)
        } else if recording {
            (palette.danger, palette.danger_hover, palette.danger)
        } else {
            (palette.accent, palette.accent_hover, palette.accent)
        };
        let painter = ui.painter();

        if recording {
            // 녹음 중임을 알리는 퍼지는 고리
            let phase = (ui.input(|input| input.time) * 0.9).fract() as f32;
            painter.circle_stroke(
                center,
                radius + margin * phase,
                Stroke::new(2.0, halo.gamma_multiply((1.0 - phase) * 0.35)),
            );
        } else if enabled && (response.hovered() || response.has_focus()) {
            painter.circle_filled(center, radius + 5.0, halo.gamma_multiply(0.2));
        }

        let pressed = response.is_pointer_button_down_on();
        let color = if response.hovered() { hover } else { fill };
        painter.circle_filled(center, if pressed { radius - 2.0 } else { radius }, color);
        let icon = if recording { Icon::Square } else { Icon::Mic };
        painter.text(
            center,
            Align2::CENTER_CENTER,
            icon.unicode(),
            icons::font(if recording { 26.0 } else { 30.0 }),
            if enabled {
                palette.on_accent
            } else {
                palette.text_faint
            },
        );
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// 입력 장치 안내: 녹음에 쓸 장치와 그 장치를 바꾸는 설정 화면으로 가는 링크.
/// 장치를 쓸 수 없으면 그 이유를 아래에 경고로 보여 줍니다.
fn device_hint(ui: &mut Ui, shared: &mut Shared, config: &Config) {
    let palette = Palette::of(ui);
    let selector = config.audio.input_device.trim();
    let name = match shared.devices.find(selector) {
        Some(device) => device.name.clone(),
        None if selector.is_empty() => t!("device.system_default").into_owned(),
        None => selector.to_owned(),
    };
    let mut open_settings = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
        ui.label(
            icons::text(Icon::Mic)
                .size(size::BODY)
                .color(palette.text_muted),
        );
        ui.label(text::caption(ui, t!("test.record.device", name = name)));
        let change = RichText::new(t!("test.change_in_settings")).size(size::CAPTION);
        open_settings = ui.link(change).clicked();
    });
    if let Some(problem) = device_problem(&shared.devices, selector) {
        ui.label(text::caption(ui, problem).color(palette.warning_text));
    }
    if open_settings {
        shared.navigate = Some(Page::Settings);
    }
}

/// 정리를 마친 녹음의 측정값, 경고, 저장 버튼
fn result_ui(ui: &mut Ui, cx: &mut PageContext<'_>, recorded: &Recorded) {
    let clip = &recorded.clip;
    let rms = dbfs(clip.rms());
    let silent = rms < SILENCE_DBFS;

    let length = Stat::new(t!("test.record.duration"), seconds_text(clip.duration()));
    let length = match recorded.trim {
        SilenceTrim::Trimmed(cut) if !cut.is_zero() => {
            length.note(t!("test.record.trimmed", seconds = seconds_text(cut)))
        }
        SilenceTrim::Trimmed(_) | SilenceTrim::NoSpeech | SilenceTrim::Unchecked => {
            length.note(t!("test.record.not_trimmed"))
        }
    };
    let gain = if recorded.gain.applied() {
        Stat::new(
            t!("test.record.gain"),
            t!(
                "test.record.gain_db",
                value = format!("{:.1}", recorded.gain.gain_db)
            ),
        )
    } else {
        Stat::new(t!("test.record.gain"), t!("test.record.gain_none"))
    };
    stats(
        ui,
        &[
            length,
            Stat::new(t!("test.record.peak"), dbfs_text(dbfs(clip.peak()))),
            Stat::new(t!("test.record.rms"), dbfs_text(rms)).warn(silent),
            gain,
        ],
    );

    // 경고는 가장 중요한 것 하나만 보여 줍니다.
    let warning = if silent {
        Some((
            t!("test.record.silent_title"),
            t!("test.record.silent_body", hint = microphone_settings_hint()),
        ))
    } else if recorded.trim == SilenceTrim::NoSpeech {
        Some((
            t!("test.record.no_speech_title"),
            t!("test.record.no_speech_body"),
        ))
    } else {
        None
    };
    if let Some((title, body)) = warning {
        ui.add_space(8.0);
        Banner::new(Tone::Warning).title(&title).line(body).show(ui);
    }
    let mut notes = Vec::new();
    if recorded.trim == SilenceTrim::Unchecked {
        notes.push(t!("test.record.unchecked"));
    }
    if recorded.auto_stopped {
        notes.push(t!("test.record.auto_stopped"));
    }
    if !notes.is_empty() {
        ui.add_space(8.0);
        Banner::new(Tone::Info)
            .lines(notes.into_iter().map(|note| note.into_owned()))
            .show(ui);
    }

    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        if controls::icon_text_button(ui, Icon::Save, t!("test.record.save_wav")).clicked() {
            save_wav(cx, clip);
        }
        if controls::icon_text_button(ui, Icon::FolderOpen, t!("test.record.open_folder")).clicked()
            && let Err(message) = system::open_dir(&cx.shared.paths.recordings_dir())
        {
            cx.shared
                .toasts
                .error(t!("common.open_failed", message = message));
        }
        ui.label(text::caption(ui, &recorded.device_name));
    });
}

/// 저장 대화상자를 띄우고 정리한 녹음을 16kHz 모노 WAV로 저장합니다.
fn save_wav(cx: &mut PageContext<'_>, clip: &AudioClip) {
    let dir = cx.shared.paths.recordings_dir();
    if let Err(error) = std::fs::create_dir_all(&dir) {
        tracing::warn!("could not create {}: {error}", dir.display());
    }
    let picked = rfd::FileDialog::new()
        .set_parent(cx.frame)
        .set_title(t!("test.record.save_dialog_title"))
        .set_directory(&dir)
        .set_file_name(next_file_name(&dir))
        .add_filter(t!("common.wav_files"), &["wav"])
        .save_file();
    let Some(mut path) = picked else {
        return;
    };
    if path.extension().is_none() {
        path.set_extension("wav");
    }

    match write_recording(&path, clip) {
        Ok(()) => {
            tracing::info!("saved recording to {}", path.display());
            cx.shared
                .toasts
                .info(t!("test.record.saved", path = path.display()));
        }
        Err(error) => {
            tracing::warn!("could not save recording: {error}");
            cx.shared.toasts.error(error_text::audio(&error));
        }
    }
}

/// 녹음을 음성 인식에 쓰는 형식(16kHz 모노)으로 바꿔 WAV 파일로 씁니다.
fn write_recording(path: &Path, clip: &AudioClip) -> Result<(), AudioError> {
    let clip = clip.resampled(WHISPER_SAMPLE_RATE)?;
    audio::write_wav(path, &clip)
}

/// 폴더에 아직 없는 `sulsul-001.wav` 꼴의 이름
fn next_file_name(dir: &Path) -> String {
    (1..10_000)
        .map(|n| format!("sulsul-{n:03}.wav"))
        .find(|name| !dir.join(name).exists())
        .unwrap_or_else(|| "sulsul.wav".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 앞뒤에 1초씩 무음을 붙인 말소리 (Windows 음성 합성으로 만든 시험용 말)
    fn speech_with_silence() -> AudioClip {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../sulsul-vad/testdata/hello_ko.wav");
        let speech = audio::read_wav(&path).unwrap();
        let quiet = std::iter::repeat_n(0.0, speech.sample_rate as usize);
        let samples = quiet.clone().chain(speech.samples).chain(quiet).collect();
        AudioClip::new(samples, speech.sample_rate)
    }

    #[test]
    fn file_name_skips_existing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(next_file_name(dir.path()), "sulsul-001.wav");
        std::fs::write(dir.path().join("sulsul-001.wav"), b"").unwrap();
        std::fs::write(dir.path().join("sulsul-002.wav"), b"").unwrap();
        assert_eq!(next_file_name(dir.path()), "sulsul-003.wav");
    }

    #[test]
    fn recording_is_saved_as_16khz_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.wav");
        // 48kHz로 녹음한 1초
        let clip = AudioClip::new(vec![0.25; 48_000], 48_000);

        write_recording(&path, &clip).unwrap();

        let saved = audio::read_wav(&path).unwrap();
        assert_eq!(saved.sample_rate, WHISPER_SAMPLE_RATE);
        let seconds = saved.duration().as_secs_f64();
        assert!(
            (seconds - 1.0).abs() < 0.05,
            "길이가 유지되어야 함: {seconds}"
        );
    }

    #[test]
    fn cleanup_trims_silence_like_dictation() {
        let clip = speech_with_silence();
        let raw = clip.duration();
        let recorded = clean(clip);
        assert_eq!(recorded.raw_duration, raw);
        let SilenceTrim::Trimmed(cut) = recorded.trim else {
            panic!("말소리를 찾아 잘라야 합니다: {:?}", recorded.trim);
        };
        // 녹음은 5.4초(앞뒤 1초 무음 + 말 3.4초)이고, 무음은 여유(앞 0.3초, 뒤 0.5초)만 남깁니다
        // (단축키 받아쓰기의 `speech_is_trimmed_before_transcribing`과 같은 기준).
        let kept = recorded.clip.duration().as_secs_f64();
        assert!(
            (3.0..4.0).contains(&kept),
            "앞뒤 무음을 잘라 내야 합니다: 남은 길이 {kept}초, 잘라 낸 길이 {cut:?}"
        );
        assert_eq!(recorded.clip.duration(), raw - cut);
        assert!(
            recorded.gain.speech_dbfs.is_finite(),
            "자동 음량 조절이 말소리를 찾아야 합니다: {:?}",
            recorded.gain
        );
    }

    #[test]
    fn recording_without_speech_is_kept_whole() {
        let silent = AudioClip::new(vec![0.0; 16_000], 16_000);
        let recorded = clean(silent);
        assert_eq!(recorded.trim, SilenceTrim::NoSpeech);
        assert_eq!(
            recorded.clip.duration(),
            Duration::from_secs(1),
            "무엇이 녹음됐는지 볼 수 있게 자르지 않습니다"
        );
        assert!(!recorded.gain.applied());
    }
}
