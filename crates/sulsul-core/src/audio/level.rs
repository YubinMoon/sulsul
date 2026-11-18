//! 구간 음량 재기: 클립을 20ms 구간으로 나눠 음량을 재고, 배경 소음과 말소리 구간을 가릅니다.
//!
//! 자동 음량 조절([`AudioClip::auto_gain`](super::AudioClip::auto_gain))이 씁니다.
//!
//! 1. 20ms 구간마다 RMS와 최대치를 잽니다.
//! 2. 구간 RMS의 하위 10%를 배경 소음으로 보고, 소음보다 10dB 넘게 큰 구간을 말소리로 봅니다.

/// 음량을 재는 구간 길이
const FRAME_SECONDS: f64 = 0.02;
/// 배경 소음으로 볼 구간 RMS 백분위
const NOISE_PERCENTILE: f32 = 0.1;
/// 배경 소음보다 이만큼 큰 구간을 말소리로 봅니다 (dB)
const SPEECH_OVER_NOISE_DB: f32 = 10.0;
/// 말소리 평균 음량이 이보다 작으면 말소리가 없다고 봅니다 (dBFS)
pub(super) const MIN_SPEECH_DBFS: f32 = -70.0;
/// 소리가 전혀 없을 때 쓰는 음량 (dBFS)
pub(super) const SILENCE_DBFS: f32 = -120.0;

/// 구간 하나의 음량
pub(super) struct Frame {
    pub rms: f32,
    pub peak: f32,
}

impl Frame {
    pub fn measure(samples: &[f32]) -> Self {
        let power =
            samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64;
        let peak = samples.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()));
        Self {
            rms: power.sqrt() as f32,
            peak,
        }
    }

    pub fn dbfs(&self) -> f32 {
        to_db(self.rms)
    }
}

/// 클립 전체의 구간 음량
pub(super) struct Levels {
    /// 앞에서부터 잰 구간들. 끝에 남는 짧은 조각은 재지 않습니다.
    pub frames: Vec<Frame>,
    /// 배경 소음 음량 (dBFS). 구간이 하나도 없으면 [`SILENCE_DBFS`]
    pub noise_dbfs: f32,
}

impl Levels {
    pub fn measure(samples: &[f32], sample_rate: u32) -> Self {
        let frame_len = ((f64::from(sample_rate) * FRAME_SECONDS).round() as usize).max(1);
        let frames: Vec<Frame> = samples
            .chunks_exact(frame_len)
            .map(Frame::measure)
            .collect();
        let noise_dbfs = if frames.is_empty() {
            SILENCE_DBFS
        } else {
            let mut levels: Vec<f32> = frames.iter().map(Frame::dbfs).collect();
            levels.sort_by(f32::total_cmp);
            percentile(&levels, NOISE_PERCENTILE)
        };
        Self { frames, noise_dbfs }
    }

    /// 말소리로 볼 만큼 큰 구간인지
    pub fn is_speech(&self, frame: &Frame) -> bool {
        frame.dbfs() > self.noise_dbfs + SPEECH_OVER_NOISE_DB
    }
}

/// 구간들의 평균 음량 (dBFS). 구간이 없으면 [`f32::NEG_INFINITY`]
pub(super) fn mean_dbfs<'a>(frames: impl IntoIterator<Item = &'a Frame>) -> f32 {
    let (sum, count) = frames.into_iter().fold((0.0_f64, 0_usize), |(sum, n), f| {
        (sum + f64::from(f.rms).powi(2), n + 1)
    });
    if count == 0 {
        return f32::NEG_INFINITY;
    }
    to_db((sum / count as f64).sqrt() as f32)
}

pub(super) fn to_db(amplitude: f32) -> f32 {
    if amplitude > 0.0 {
        (20.0 * amplitude.log10()).max(SILENCE_DBFS)
    } else {
        SILENCE_DBFS
    }
}

/// 정렬된 값에서 백분위 값
pub(super) fn percentile(sorted: &[f32], p: f32) -> f32 {
    let index = ((sorted.len() - 1) as f32 * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// 테스트 신호를 만드는 도구
#[cfg(test)]
pub(super) mod signal {
    pub const RATE: u32 = 16_000;
    /// 20ms에 정확히 5주기가 들어가 구간 RMS가 진폭/√2가 됩니다.
    pub const TONE_HZ: f32 = 250.0;

    /// 테스트마다 같은 값을 내는 -1.0 ~ 1.0 난수
    pub struct Lcg(pub u32);

    impl Lcg {
        pub fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1 << 23) as f32 - 1.0
        }
    }
}
