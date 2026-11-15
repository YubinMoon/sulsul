//! 효과음: 녹음 시작, 받아쓰기 완료, 실패.
//!
//! 소리 파일 없이 짧은 사인파 음을 코드로 만들어 메모리의 WAV로 재생합니다.
//! Windows는 `PlaySoundW`로 비동기 재생하고, 다른 OS는 아직 소리를 내지 않습니다.

use std::f32::consts::TAU;
use std::sync::OnceLock;

use sulsul_core::audio::{AudioClip, encode_wav_bytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// 녹음을 시작함: 올라가는 두 음
    Start,
    /// 받아쓴 글을 클립보드에 넣음: 맑은 한 음
    Done,
    /// 녹음·인식·복사에 실패했거나 인식된 말이 없음: 내려가는 낮은 두 음
    Fail,
}

const SAMPLE_RATE: u32 = 44_100;
/// 최대 음량 (0.0 ~ 1.0). 녹음 시작음이 마이크에 크게 들어가지 않게 작게 둡니다.
const VOLUME: f32 = 0.2;
/// 음 사이 쉼
const GAP_SECONDS: f32 = 0.012;
/// 음이 커지는 시간 (딸깍 소리 방지)
const ATTACK_SECONDS: f32 = 0.004;
/// 음 끝을 줄이는 시간 (딸깍 소리 방지)
const RELEASE_SECONDS: f32 = 0.012;

/// 음 하나: 주파수(Hz), 길이(초), 줄어드는 빠르기(1/초)
struct Note {
    frequency: f32,
    seconds: f32,
    decay: f32,
}

impl Cue {
    const ALL: [Self; 3] = [Self::Start, Self::Done, Self::Fail];

    fn notes(self) -> &'static [Note] {
        match self {
            // 레(D5) → 라(A5)
            Self::Start => &[
                Note {
                    frequency: 587.3,
                    seconds: 0.07,
                    decay: 12.0,
                },
                Note {
                    frequency: 880.0,
                    seconds: 0.11,
                    decay: 14.0,
                },
            ],
            // 미(E6) 한 음을 길게 울림
            Self::Done => &[Note {
                frequency: 1318.5,
                seconds: 0.2,
                decay: 16.0,
            }],
            // 라(A4) → 레(D4)
            Self::Fail => &[
                Note {
                    frequency: 440.0,
                    seconds: 0.1,
                    decay: 8.0,
                },
                Note {
                    frequency: 293.7,
                    seconds: 0.17,
                    decay: 8.0,
                },
            ],
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Start => 0,
            Self::Done => 1,
            Self::Fail => 2,
        }
    }
}

/// 효과음 샘플 (-1.0 ~ 1.0)
fn render(cue: Cue) -> Vec<f32> {
    let rate = SAMPLE_RATE as f32;
    let mut samples = Vec::new();
    for (i, note) in cue.notes().iter().enumerate() {
        if i > 0 {
            samples.extend(std::iter::repeat_n(0.0, (GAP_SECONDS * rate) as usize));
        }
        let length = (note.seconds * rate) as usize;
        for n in 0..length {
            let t = n as f32 / rate;
            let remaining = note.seconds - t;
            let envelope = (t / ATTACK_SECONDS).min(1.0)
                * (-note.decay * t).exp()
                * (remaining / RELEASE_SECONDS).min(1.0);
            // 배음을 조금 섞어 부드럽게 만듭니다.
            let phase = TAU * note.frequency * t;
            let tone = phase.sin() * 0.85 + (phase * 2.0).sin() * 0.15;
            samples.push(tone * envelope * VOLUME);
        }
    }
    samples
}

/// 효과음 WAV (처음 쓸 때 한 번 만듭니다)
fn wav(cue: Cue) -> &'static [u8] {
    static WAVS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    let wavs = WAVS.get_or_init(|| {
        Cue::ALL
            .iter()
            .map(|cue| {
                encode_wav_bytes(&AudioClip::new(render(*cue), SAMPLE_RATE)).unwrap_or_else(
                    |error| {
                        tracing::warn!("could not build the {cue:?} sound: {error}");
                        Vec::new()
                    },
                )
            })
            .collect()
    });
    &wavs[cue.index()]
}

/// 효과음을 냅니다. 기다리지 않고 바로 돌아오며, 앞의 효과음이 아직 나고 있으면 끊고 새로 냅니다.
pub fn play(cue: Cue) {
    let bytes = wav(cue);
    if bytes.is_empty() {
        return;
    }
    platform_play(bytes);
}

#[cfg(windows)]
fn platform_play(bytes: &'static [u8]) {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    // SAFETY: `SND_MEMORY`이면 첫 인자는 WAV 이미지의 주소입니다. 비동기로 재생하는 동안에도
    // 읽을 수 있도록 프로그램이 끝날 때까지 살아 있는 데이터를 넘깁니다.
    let played = unsafe {
        PlaySoundW(
            bytes.as_ptr().cast::<u16>(),
            std::ptr::null_mut(),
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        )
    };
    if played == 0 {
        tracing::debug!("PlaySoundW failed");
    }
}

#[cfg(not(windows))]
fn platform_play(_bytes: &'static [u8]) {
    tracing::debug!("feedback sounds are not supported on this platform");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_are_short_quiet_and_click_free() {
        for cue in Cue::ALL {
            let samples = render(cue);
            let seconds = samples.len() as f32 / SAMPLE_RATE as f32;
            assert!((0.1..0.4).contains(&seconds), "{cue:?}: 길이 {seconds}초");
            let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
            assert!(peak > 0.05 && peak <= VOLUME, "{cue:?}: 최대 음량 {peak}");
            assert!(samples[0].abs() < 0.01, "{cue:?}: 시작이 0에 가까워야 함");
            assert!(
                samples.last().unwrap().abs() < 0.01,
                "{cue:?}: 끝이 0에 가까워야 함"
            );
        }
    }

    #[test]
    fn cues_are_distinct_wav_files() {
        let start = wav(Cue::Start);
        let done = wav(Cue::Done);
        let fail = wav(Cue::Fail);
        for bytes in [start, done, fail] {
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(&bytes[8..12], b"WAVE");
        }
        assert_ne!(start, done);
        assert_ne!(done, fail);
    }
}
