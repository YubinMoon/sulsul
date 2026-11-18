//! 녹음이 끝난 클립의 말소리 크기를 알맞게 키우는 자동 음량 조절.
//!
//! 실시간으로 조절하는 AGC(디스코드 등)와 달리 클립 전체를 본 뒤 증폭량을 한 번만 정합니다.
//! 그래서 녹음 앞부분이 덜 커지거나 음량이 출렁이지 않습니다.
//!
//! 1. 20ms 구간마다 음량을 재고, 배경 소음보다 뚜렷하게 큰 구간을 말소리로 봅니다 ([`super::level`]).
//! 2. 말소리 구간이 너무 짧거나 너무 작으면 키우지 않습니다 (소음만 키우면 환각이 늘어남).
//! 3. 말소리 평균 음량을 목표에 맞추되, 최대 증폭량과 말소리 최대치가 넘지 않을 선을 지킵니다.
//! 4. 그래도 튀는 소리는 부드럽게 눌러 상한 아래로 둡니다.
//!
//! 이미 충분히 큰 소리는 줄이지 않습니다.

use super::AudioClip;
use super::level::{Frame, Levels, MIN_SPEECH_DBFS, mean_dbfs, percentile, to_db};

/// 말소리 평균 음량 목표 (dBFS)
const TARGET_SPEECH_DBFS: f32 = -22.0;
/// 최대 증폭량 (dB)
const MAX_GAIN_DB: f32 = 30.0;
/// 이보다 작은 증폭은 하지 않습니다 (dB)
const MIN_GAIN_DB: f32 = 1.0;
/// 말소리 구간이 이보다 적으면(200ms) 키우지 않습니다
const MIN_SPEECH_FRAMES: usize = 10;
/// 말소리 구간 최대치의 이 백분위가 [`KNEE`]를 넘지 않게 증폭량을 줄입니다
const PEAK_PERCENTILE: f32 = 0.98;
/// 이 크기까지는 그대로 두고, 넘는 부분만 부드럽게 누릅니다 (약 -3 dBFS)
const KNEE: f32 = 0.7;
/// 눌러도 넘지 않는 상한 (약 -1 dBFS)
const CEILING: f32 = 0.89;

/// [`AudioClip::auto_gain`]의 결과
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoGain {
    /// 실제로 키운 양 (dB). 키우지 않았으면 0
    pub gain_db: f32,
    /// 키우기 전 말소리 평균 음량 (dBFS). 말소리 구간이 없으면 [`f32::NEG_INFINITY`]
    pub speech_dbfs: f32,
    /// 키우기 전 배경 소음 음량 (dBFS)
    pub noise_dbfs: f32,
}

impl AutoGain {
    /// 소리를 키웠는지
    pub fn applied(&self) -> bool {
        self.gain_db > 0.0
    }
}

impl AudioClip {
    /// 말소리가 작으면 알맞은 크기로 키웁니다. 키우지 않았으면 샘플은 그대로입니다.
    pub fn auto_gain(&mut self) -> AutoGain {
        let result = auto_gain(&mut self.samples, self.sample_rate);
        tracing::info!(
            gain_db = result.gain_db,
            speech_dbfs = result.speech_dbfs,
            noise_dbfs = result.noise_dbfs,
            "auto gain {}",
            if result.applied() {
                "applied"
            } else {
                "skipped"
            }
        );
        result
    }
}

fn auto_gain(samples: &mut [f32], sample_rate: u32) -> AutoGain {
    let levels = Levels::measure(samples, sample_rate);
    let noise_dbfs = levels.noise_dbfs;
    let mut skipped = AutoGain {
        gain_db: 0.0,
        speech_dbfs: f32::NEG_INFINITY,
        noise_dbfs,
    };

    let speech: Vec<&Frame> = levels
        .frames
        .iter()
        .filter(|f| levels.is_speech(f))
        .collect();
    if speech.len() < MIN_SPEECH_FRAMES {
        return skipped;
    }
    let speech_dbfs = mean_dbfs(speech.iter().copied());
    skipped.speech_dbfs = speech_dbfs;
    if speech_dbfs < MIN_SPEECH_DBFS {
        return skipped;
    }

    let mut peaks: Vec<f32> = speech.iter().map(|f| to_db(f.peak)).collect();
    peaks.sort_by(f32::total_cmp);
    let peak_dbfs = percentile(&peaks, PEAK_PERCENTILE);
    let gain_db = (TARGET_SPEECH_DBFS - speech_dbfs)
        .min(to_db(KNEE) - peak_dbfs)
        .min(MAX_GAIN_DB);
    if gain_db < MIN_GAIN_DB {
        return skipped;
    }

    let gain = 10.0_f32.powf(gain_db / 20.0);
    for sample in samples.iter_mut() {
        *sample = soft_limit(*sample * gain);
    }
    AutoGain {
        gain_db,
        speech_dbfs,
        noise_dbfs,
    }
}

/// [`KNEE`]까지는 그대로, 넘는 부분은 [`CEILING`]에 점점 가까워지게 누릅니다.
fn soft_limit(x: f32) -> f32 {
    let magnitude = x.abs();
    if magnitude <= KNEE {
        return x;
    }
    let range = CEILING - KNEE;
    (KNEE + range * ((magnitude - KNEE) / range).tanh()).copysign(x)
}

#[cfg(test)]
mod tests {
    use super::super::level::signal::{Lcg, RATE, TONE_HZ};
    use super::*;
    use std::f32::consts::{SQRT_2, TAU};

    /// 말소리 대신 쓰는 신호: 200ms 소리와 200ms 조용한 구간(약 -85 dBFS의 배경 소음)을 번갈아 둡니다.
    fn speech_like(rms_dbfs: f32, bursts: usize) -> AudioClip {
        with_background(rms_dbfs, bursts, 1e-4)
    }

    /// 조용한 구간의 배경 소음 최대 진폭(`noise_amplitude`)을 정할 수 있는 [`speech_like`]
    fn with_background(rms_dbfs: f32, bursts: usize, noise_amplitude: f32) -> AudioClip {
        let amplitude = 10.0_f32.powf(rms_dbfs / 20.0) * SQRT_2;
        let half = (RATE / 5) as usize;
        let mut noise = Lcg(1);
        let mut samples = Vec::with_capacity(half * 2 * bursts);
        for _ in 0..bursts {
            for i in 0..half {
                samples.push((TAU * TONE_HZ * i as f32 / RATE as f32).sin() * amplitude);
            }
            samples.extend((0..half).map(|_| noise.next() * noise_amplitude));
        }
        AudioClip::new(samples, RATE)
    }

    fn max_abs(clip: &AudioClip) -> f32 {
        clip.samples.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()))
    }

    #[test]
    fn quiet_speech_is_raised_to_target() {
        let mut clip = speech_like(-40.0, 5);
        let result = clip.auto_gain();
        assert!(result.applied(), "작은 말소리는 키워야 합니다: {result:?}");
        assert!((result.speech_dbfs - -40.0).abs() < 0.1, "{result:?}");
        assert!((result.gain_db - 18.0).abs() < 0.1, "{result:?}");
        let after = Frame::measure(&clip.samples[..(RATE / 5) as usize]);
        assert!((to_db(after.rms) - TARGET_SPEECH_DBFS).abs() < 0.1);
    }

    #[test]
    fn gain_is_capped() {
        let mut clip = speech_like(-58.0, 5);
        let result = clip.auto_gain();
        assert_eq!(result.gain_db, MAX_GAIN_DB);
    }

    #[test]
    fn loud_peaks_reduce_gain() {
        let mut clip = speech_like(-40.0, 5);
        // 말소리 구간마다 -14 dBFS 정도의 튀는 소리를 넣습니다.
        let frame = (RATE / 50) as usize;
        for start in (0..clip.samples.len()).step_by(frame) {
            if clip.samples[start + 1].abs() > 1e-3 {
                clip.samples[start + 1] = 0.2;
            }
        }
        let result = clip.auto_gain();
        let expected = to_db(KNEE) - to_db(0.2);
        assert!((result.gain_db - expected).abs() < 0.1, "{result:?}");
        assert!(max_abs(&clip) <= KNEE + 1e-4);
    }

    #[test]
    fn rare_spikes_are_limited() {
        let mut clip = speech_like(-40.0, 5);
        clip.samples[1] = 0.5;
        let result = clip.auto_gain();
        // 튐 하나가 평균 음량을 조금 올리지만, 최대치 때문에 증폭량이 크게 줄지는 않아야 합니다.
        assert!(
            result.gain_db > 15.0,
            "드문 튐은 증폭량을 줄이지 않아야 합니다: {result:?}"
        );
        assert!(max_abs(&clip) <= CEILING + 1e-6);
    }

    #[test]
    fn loud_speech_is_unchanged() {
        let original = speech_like(-15.0, 5);
        let mut clip = original.clone();
        assert!(!clip.auto_gain().applied());
        assert_eq!(clip, original);
    }

    #[test]
    fn steady_noise_is_unchanged() {
        let mut noise = Lcg(7);
        let samples = (0..RATE * 2).map(|_| noise.next() * 0.003).collect();
        let original = AudioClip::new(samples, RATE);
        let mut clip = original.clone();
        let result = clip.auto_gain();
        assert!(
            !result.applied(),
            "소음만 있으면 키우지 않아야 합니다: {result:?}"
        );
        assert_eq!(clip, original);
    }

    #[test]
    fn silence_and_short_clips_are_unchanged() {
        for original in [
            AudioClip::new(vec![0.0; RATE as usize], RATE),
            AudioClip::new(Vec::new(), RATE),
            // 말소리가 100ms뿐인 클립
            AudioClip::new(
                speech_like(-40.0, 1).samples[..(RATE / 10) as usize].to_vec(),
                RATE,
            ),
        ] {
            let mut clip = original.clone();
            assert!(!clip.auto_gain().applied());
            assert_eq!(clip, original);
        }
    }

    #[test]
    fn too_quiet_speech_is_unchanged() {
        // 배경이 완전히 조용해 소음 기준으로는 거를 수 없는 경우
        let mut clip = with_background(-75.0, 5, 0.0);
        let result = clip.auto_gain();
        assert!(!result.applied(), "{result:?}");
        assert!((result.speech_dbfs - -75.0).abs() < 0.1, "{result:?}");

        // -65 dBFS는 작아도 말소리로 보고 키웁니다.
        let mut clip = with_background(-65.0, 5, 0.0);
        assert_eq!(clip.auto_gain().gain_db, MAX_GAIN_DB);
    }

    #[test]
    fn soft_limit_is_smooth_and_bounded() {
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-KNEE), -KNEE);
        assert!(soft_limit(0.8) > soft_limit(0.75));
        assert!(soft_limit(10.0) <= CEILING + 1e-6);
        assert!(soft_limit(-10.0) >= -CEILING - 1e-6);
    }
}
