use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

use super::{AudioClip, AudioError};

/// 고정 비율 FFT 리샘플러로 샘플레이트를 바꿉니다 (녹음이 끝난 클립 전체를 한 번에 처리).
pub fn resample(clip: &AudioClip, target_rate: u32) -> Result<AudioClip, AudioError> {
    if target_rate == 0 || clip.sample_rate == 0 {
        return Err(AudioError::Resample(format!(
            "invalid sample rate: {} -> {target_rate}",
            clip.sample_rate
        )));
    }
    if clip.sample_rate == target_rate || clip.is_empty() {
        return Ok(AudioClip::new(clip.samples.clone(), target_rate));
    }

    let error = |e: &dyn std::fmt::Display| AudioError::Resample(e.to_string());
    let frames = clip.samples.len();
    let mut resampler = Fft::<f32>::new(
        clip.sample_rate as usize,
        target_rate as usize,
        1024,
        1,
        FixedSync::Input,
    )
    .map_err(|e| error(&e))?;
    let input = InterleavedSlice::new(&clip.samples, 1, frames).map_err(|e| error(&e))?;
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|e| error(&e))?;
    Ok(AudioClip::new(output.take_data(), target_rate))
}

impl AudioClip {
    /// 샘플레이트를 바꾼 사본
    pub fn resampled(&self, target_rate: u32) -> Result<AudioClip, AudioError> {
        resample(self, target_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn sine(freq: f32, rate: u32, seconds: f32) -> AudioClip {
        let n = (rate as f32 * seconds) as usize;
        let samples = (0..n)
            .map(|i| (TAU * freq * i as f32 / rate as f32).sin() * 0.5)
            .collect();
        AudioClip::new(samples, rate)
    }

    #[test]
    fn downsample_keeps_duration_and_level() {
        let clip = sine(440.0, 48_000, 1.0);
        let out = clip.resampled(16_000).unwrap();
        assert_eq!(out.sample_rate, 16_000);
        assert!((out.samples.len() as i64 - 16_000).abs() <= 1);
        assert!((out.rms() - clip.rms()).abs() < 0.01);
    }

    #[test]
    fn odd_ratio() {
        let clip = sine(1_000.0, 44_100, 0.5);
        let out = clip.resampled(16_000).unwrap();
        assert!((out.samples.len() as i64 - 8_000).abs() <= 1);
    }

    #[test]
    fn same_rate_is_copy() {
        let clip = sine(440.0, 16_000, 0.1);
        assert_eq!(clip.resampled(16_000).unwrap(), clip);
    }
}
