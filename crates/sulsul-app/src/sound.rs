//! 효과음: 녹음 시작, 녹음 끝, 받아쓰기 완료, 실패.
//!
//! 소리는 물방울 오버레이의 움직임에 맞춰 만든 WAV 파일(`prototypes/liquid-droplet/sounds`)을 빌드할 때 넣습니다.
//! 파일은 그 폴더의 `make_sounds.py`로 만들고, 물방울 미리보기도 같은 파일을 재생합니다.
//! Windows는 `PlaySoundW`로 비동기 재생하고, 다른 OS는 아직 소리를 내지 않습니다.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// 녹음을 시작함: 물방울이 떠오르는 "블룹"과 수면에서 떨어지는 "똑"
    Start,
    /// 녹음을 끝내고 인식을 시작함: 물방울이 터지는 "뽁"과 흩어지는 작은 물방울들
    Pop,
    /// 받아쓴 글을 클립보드나 커서 위치에 넣음: 고리가 빨려 들어가 합쳐지는 "뽀"와 사라지는 "뿅"
    Done,
    /// 녹음·인식·복사·입력에 실패했거나, 인식된 말이 없거나, 커서 위치에 넣을 곳이 없음:
    /// 바람 빠지듯 내려가는 두 물소리
    Fail,
}

impl Cue {
    /// 효과음 WAV
    fn wav(self) -> &'static [u8] {
        match self {
            Self::Start => include_bytes!("../../../prototypes/liquid-droplet/sounds/start.wav"),
            Self::Pop => include_bytes!("../../../prototypes/liquid-droplet/sounds/pop.wav"),
            Self::Done => include_bytes!("../../../prototypes/liquid-droplet/sounds/done.wav"),
            Self::Fail => include_bytes!("../../../prototypes/liquid-droplet/sounds/fail.wav"),
        }
    }
}

/// 효과음을 냅니다. 기다리지 않고 바로 돌아오며, 앞의 효과음이 아직 나고 있으면 끊고 새로 냅니다.
pub fn play(cue: Cue) {
    platform_play(cue.wav());
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

    const ALL: [Cue; 4] = [Cue::Start, Cue::Pop, Cue::Done, Cue::Fail];

    /// 16비트 모노 44.1kHz PCM WAV의 샘플 (-1.0 ~ 1.0)
    fn samples(cue: Cue) -> Vec<f32> {
        let bytes = cue.wav();
        assert_eq!(&bytes[..4], b"RIFF", "{cue:?}: RIFF 파일이 아닙니다");
        assert_eq!(&bytes[8..12], b"WAVE", "{cue:?}: WAVE 파일이 아닙니다");
        let mut at = 12;
        let mut data = None;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            let body = &bytes[at + 8..at + 8 + size];
            match id {
                b"fmt " => {
                    let field = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
                    assert_eq!(field(0), 1, "{cue:?}: PCM이 아닙니다");
                    assert_eq!(field(2), 1, "{cue:?}: 모노가 아닙니다");
                    let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                    assert_eq!(rate, 44_100, "{cue:?}: 표본 빈도");
                    assert_eq!(field(14), 16, "{cue:?}: 16비트가 아닙니다");
                }
                b"data" => data = Some(body),
                _ => {}
            }
            at += 8 + size + size % 2;
        }
        let data = data.unwrap_or_else(|| panic!("{cue:?}: data 조각이 없습니다"));
        data.as_chunks::<2>()
            .0
            .iter()
            .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32767.0)
            .collect()
    }

    #[test]
    fn cues_are_short_quiet_and_click_free() {
        for cue in ALL {
            let samples = samples(cue);
            let seconds = samples.len() as f32 / 44_100.0;
            assert!((0.2..0.8).contains(&seconds), "{cue:?}: 길이 {seconds}초");
            let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
            // 시작음은 녹음과 함께 나므로 마이크에 크게 들어가지 않게 작게 둡니다.
            let limit = if cue == Cue::Start { 0.18 } else { 0.21 };
            assert!(peak > 0.05 && peak <= limit, "{cue:?}: 최대 음량 {peak}");
            assert!(samples[0].abs() < 0.01, "{cue:?}: 시작이 0에 가까워야 함");
            assert!(
                samples.last().unwrap().abs() < 0.01,
                "{cue:?}: 끝이 0에 가까워야 함"
            );
        }
    }

    #[test]
    fn cues_are_distinct() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.wav(), b.wav(), "{a:?}와 {b:?}의 소리가 같습니다");
            }
        }
    }
}
