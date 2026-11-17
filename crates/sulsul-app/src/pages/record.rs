//! 녹음 화면: 마이크 입력이 제대로 들어오는지 확인합니다.
//!
//! 녹음에 쓸 입력 장치는 설정 화면에서 고르고, 여기서는 지금 고른 장치를 알려 주기만 합니다.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Align2, Color32, Response, RichText, Role, Sense, Stroke, Ui, WidgetInfo, vec2};
use rust_i18n::t;
use sulsul_core::audio::{self, AudioClip, AudioError, Recording, RecordingOptions};
use sulsul_core::config::{Config, HotkeyMode, MAX_RECORD_SECONDS};
use sulsul_core::stt::WHISPER_SAMPLE_RATE;

use super::{Page, SECTION_GAP, header};
use crate::error_text;
use crate::format::{
    SILENCE_DBFS, clock_text, clock_text_short, dbfs, dbfs_text, hertz_text, percent_text,
    seconds_text,
};
use crate::i18n::microphone_settings_hint;
use crate::icons::{self, Icon};
use crate::session::{DictationState, DictationStatus, PageContext, Recorded, Shared};
use crate::system;
use crate::theme::{self, Palette, size};
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card::Section;
use crate::widgets::device_combo::device_problem;
use crate::widgets::hotkey_field::keycaps;
use crate::widgets::level_meter::{MeterState, level_meter};
use crate::widgets::stat::{Stat, stats};
use crate::widgets::{controls, row, text};

/// 결과 카드로 화면을 옮길 때 카드 둘레에 남기는 여백
const REVEAL_MARGIN: f32 = 16.0;

/// 장치가 데이터를 보내지 않아도 녹음이 끝나도록 두는 실제 시간 여유
const STALL_GRACE: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct RecordPage {
    active: Option<Active>,
    problem: Option<Problem>,
    /// 방금 끝난 녹음의 결과가 보이도록 화면을 내려야 하는지
    reveal_result: bool,
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
}

impl RecordPage {
    pub fn is_recording(&self) -> bool {
        self.active.is_some()
    }

    /// 매 프레임 부릅니다(화면이 보이지 않아도). 최대 길이에 닿으면 녹음을 끝냅니다.
    pub fn tick(&mut self, shared: &mut Shared, ctx: &egui::Context) {
        let Some(active) = &self.active else {
            return;
        };
        if active.recording.limit_reached() {
            self.stop(shared, true);
        } else if let Some(message) = active.recording.error() {
            self.stop(shared, false);
            // 일부라도 녹음됐으면 결과는 남기되, 중간에 끊긴 이유를 함께 알립니다.
            self.problem = Some(Problem::Audio(AudioError::Stream(message)));
        } else if active.started.elapsed() > active.max + STALL_GRACE {
            self.stop(shared, true);
        } else {
            // 미터와 시계가 계속 움직이도록
            ctx.request_repaint();
        }
    }

    fn start(&mut self, config: &Config) {
        self.problem = None;
        let (min, max) = MAX_RECORD_SECONDS;
        let seconds = i64::from(config.audio.max_record_seconds).clamp(min, max);
        let max = Duration::from_secs(seconds.unsigned_abs());
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

    fn stop(&mut self, shared: &mut Shared, auto_stopped: bool) {
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
                    "recording finished"
                );
                shared.last_recording = Some(Arc::new(Recorded {
                    clip,
                    device_name,
                    auto_stopped,
                }));
                self.reveal_result = true;
            }
            Err(error) => {
                tracing::warn!("recording failed: {error}");
                self.problem = Some(Problem::Audio(error));
            }
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        header(ui, &t!("record.title"), &t!("record.subtitle"));

        self.recorder_card(ui, cx);

        if let Some(problem) = &self.problem {
            ui.add_space(SECTION_GAP);
            match problem {
                Problem::Audio(error) => Banner::new(Tone::Error)
                    .title(&t!("record.error_title"))
                    .line(error_text::audio(error))
                    .show(ui),
                Problem::Empty => Banner::new(Tone::Warning)
                    .title(&t!("record.empty_title"))
                    .line(t!("record.empty_body"))
                    .show(ui),
            }
        }

        if let Some(recorded) = cx.shared.last_recording.clone() {
            ui.add_space(SECTION_GAP);
            let card = ui.scope(|ui| result_card(ui, cx, &recorded)).response.rect;
            if std::mem::take(&mut self.reveal_result) {
                // 결과(특히 소리가 없다는 경고)를 놓치지 않도록 카드 끝까지 보여 줍니다.
                ui.scroll_to_rect(card.expand(REVEAL_MARGIN), Some(egui::Align::Max));
            }
        }
    }

    fn recorder_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        Section::new(Icon::AudioWaveform, &t!("record.recorder.title")).show(ui, |ui| {
            let recording = self.is_recording();
            let dt = ui.input(|input| input.stable_dt);
            if let Some(active) = &mut self.active {
                active.meter.update(active.recording.level(), dt);
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                let label = if recording {
                    t!("record.stop")
                } else {
                    t!("record.start")
                };
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    let button = record_button(ui, recording, &label);
                    ui.allocate_ui_with_layout(
                        vec2(button.rect.width(), 18.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.label(text::caption(ui, label.as_ref()));
                        },
                    );
                    if button.clicked() {
                        if recording {
                            self.stop(cx.shared, false);
                        } else {
                            self.start(cx.config);
                        }
                    }
                });

                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    self.status_ui(ui, cx.config);
                    ui.add_space(8.0);
                    let default_meter = MeterState::default();
                    let meter = self.active.as_ref().map_or(&default_meter, |a| &a.meter);
                    level_meter(ui, meter, self.active.is_some());
                    let readout = if self.active.is_some() {
                        t!(
                            "record.recorder.level_readout",
                            level = dbfs_text(meter.level_dbfs()),
                            peak = dbfs_text(meter.peak_dbfs())
                        )
                    } else {
                        t!("record.recorder.level_idle")
                    };
                    ui.label(text::caption(ui, readout));
                });
            });

            row::separator(ui);
            hotkey_hint(ui, cx.config, &cx.shared.dictation);
            device_hint(ui, cx.shared, cx.config);
        });
    }

    fn status_ui(&self, ui: &mut Ui, config: &Config) {
        let palette = Palette::of(ui);
        match &self.active {
            Some(active) => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    let pulse = (ui.input(|input| input.time) * 3.0).sin() as f32 * 0.5 + 0.5;
                    ui.painter().circle_filled(
                        dot.center(),
                        4.0,
                        theme::mix(palette.danger_soft, palette.danger, 0.4 + 0.6 * pulse),
                    );
                    ui.label(
                        text::strong(t!("record.recorder.recording")).color(palette.danger_text),
                    );
                });
                ui.label(
                    RichText::new(clock_text(active.recording.elapsed()))
                        .font(theme::semibold(size::DISPLAY)),
                );
                ui.label(text::caption(
                    ui,
                    t!(
                        "record.recorder.limit_with_device",
                        max = clock_text_short(active.max),
                        device = active.recording.device_name()
                    ),
                ));
            }
            None => {
                ui.label(text::strong(t!("record.recorder.idle")).color(palette.text_muted));
                ui.label(
                    RichText::new(clock_text(Duration::ZERO))
                        .font(theme::semibold(size::DISPLAY))
                        .color(palette.text_faint),
                );
                let (min, max) = MAX_RECORD_SECONDS;
                let seconds = i64::from(config.audio.max_record_seconds).clamp(min, max);
                ui.label(text::caption(
                    ui,
                    t!(
                        "record.recorder.limit",
                        max = clock_text_short(Duration::from_secs(seconds.unsigned_abs()))
                    ),
                ));
            }
        }
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
        let (fill, hover, halo) = if recording {
            (palette.danger, palette.danger_hover, palette.danger)
        } else {
            (palette.accent, palette.accent_hover, palette.accent)
        };
        let painter = ui.painter();

        if recording {
            // 녹음 중임을 알리는 퍼지는 고리
            let phase = (ui.input(|input| input.time) * 0.9).fract() as f32;
            let alpha = ((1.0 - phase) * 90.0) as u8;
            painter.circle_stroke(
                center,
                radius + margin * phase,
                Stroke::new(
                    2.0,
                    Color32::from_rgba_unmultiplied(halo.r(), halo.g(), halo.b(), alpha),
                ),
            );
        } else if response.hovered() || response.has_focus() {
            painter.circle_filled(
                center,
                radius + 5.0,
                Color32::from_rgba_unmultiplied(halo.r(), halo.g(), halo.b(), 50),
            );
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
            palette.on_accent,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 단축키 안내: 지금 단축키와 동작 방식, 단축키로 받아쓰는 중이면 그 상태.
/// 전역 단축키를 쓸 수 없으면 그 이유를 아래에 경고로 보여 줍니다.
fn hotkey_hint(ui: &mut Ui, config: &Config, dictation: &DictationStatus) {
    let palette = Palette::of(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
        ui.label(
            icons::text(Icon::Keyboard)
                .size(size::BODY)
                .color(palette.text_muted),
        );
        ui.label(text::caption(
            ui,
            t!(
                "record.hotkey.label",
                mode = hotkey_mode_name(config.hotkey.mode)
            ),
        ));
        keycaps(ui, &config.hotkey.key);
        match dictation.state {
            DictationState::Idle => {}
            DictationState::Recording => {
                controls::tone_badge(
                    ui,
                    &t!("record.hotkey.recording"),
                    palette.danger_soft,
                    palette.danger_text,
                );
            }
            DictationState::Transcribing => {
                controls::tone_badge(
                    ui,
                    &t!("record.hotkey.transcribing"),
                    palette.accent_soft,
                    palette.accent_text,
                );
            }
            DictationState::WaitingPaste => {
                controls::tone_badge(
                    ui,
                    &t!("record.hotkey.waiting_paste"),
                    palette.success_soft,
                    palette.success_text,
                );
            }
        }
    });
    if let Some(error) = &dictation.hotkey_error {
        ui.label(text::caption(ui, error).color(palette.warning_text));
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
        ui.label(text::caption(ui, t!("record.device.label", name = name)));
        let change = RichText::new(t!("record.device.change")).size(size::CAPTION);
        open_settings = ui.link(change).clicked();
    });
    if let Some(problem) = device_problem(&shared.devices, selector) {
        ui.label(text::caption(ui, problem).color(palette.warning_text));
    }
    if open_settings {
        shared.navigate = Some(Page::Settings);
    }
}

pub fn hotkey_mode_name(mode: HotkeyMode) -> String {
    match mode {
        HotkeyMode::Toggle => t!("hotkey.mode.toggle"),
        HotkeyMode::PushToTalk => t!("hotkey.mode.push_to_talk"),
    }
    .into_owned()
}

fn result_card(ui: &mut Ui, cx: &mut PageContext<'_>, recorded: &Recorded) {
    Section::new(Icon::FileAudio, &t!("record.result.title")).show(ui, |ui| {
        let clip = &recorded.clip;
        let rms = dbfs(clip.rms());
        let silent = rms < SILENCE_DBFS;

        if silent {
            Banner::new(Tone::Warning)
                .title(&t!("record.result.silent_title"))
                .line(t!(
                    "record.result.silent_body",
                    hint = microphone_settings_hint()
                ))
                .show(ui);
            ui.add_space(8.0);
        }

        ui.add_space(2.0);
        stats(
            ui,
            &[
                Stat::new(t!("record.result.duration"), seconds_text(clip.duration())),
                Stat::new(
                    t!("record.result.sample_rate"),
                    hertz_text(clip.sample_rate),
                ),
                Stat::new(t!("record.result.peak"), dbfs_text(dbfs(clip.peak())))
                    .note(percent_text(clip.peak())),
                Stat::new(t!("record.result.rms"), dbfs_text(rms)).warn(silent),
            ],
        );
        ui.add_space(4.0);
        ui.label(text::caption(
            ui,
            t!("record.result.device", device = recorded.device_name),
        ));

        if recorded.auto_stopped {
            ui.add_space(6.0);
            Banner::new(Tone::Info)
                .line(t!("record.result.auto_stopped"))
                .show(ui);
        }

        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if controls::icon_text_button(ui, Icon::Save, t!("record.result.save_wav")).clicked() {
                save_wav(cx, recorded);
            }
            if controls::icon_text_button(ui, Icon::FolderOpen, t!("record.result.open_folder"))
                .clicked()
                && let Err(message) = system::open_dir(&cx.shared.paths.recordings_dir())
            {
                cx.shared
                    .toasts
                    .error(t!("common.open_failed", message = message));
            }
            if controls::icon_text_button(ui, Icon::ArrowRight, t!("record.result.to_transcribe"))
                .clicked()
            {
                cx.shared.navigate = Some(Page::Transcribe);
            }
        });
    });
}

/// 저장 대화상자를 띄우고 16kHz 모노 WAV로 저장합니다.
fn save_wav(cx: &mut PageContext<'_>, recorded: &Recorded) {
    let dir = cx.shared.paths.recordings_dir();
    if let Err(error) = std::fs::create_dir_all(&dir) {
        tracing::warn!("could not create {}: {error}", dir.display());
    }
    let picked = rfd::FileDialog::new()
        .set_parent(cx.frame)
        .set_title(t!("record.result.save_dialog_title"))
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

    match write_recording(&path, &recorded.clip) {
        Ok(()) => {
            tracing::info!("saved recording to {}", path.display());
            cx.shared
                .toasts
                .info(t!("record.result.saved", path = path.display()));
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
}
