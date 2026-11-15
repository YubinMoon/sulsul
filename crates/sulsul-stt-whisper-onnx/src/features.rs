//! Whisper 규격의 로그 멜 특징 추출.
//!
//! OpenAI Whisper의 `log_mel_spectrogram`과 같은 계산입니다.
//! - 16 kHz, `n_fft` 400(25 ms), `hop` 160(10 ms), 주기형 Hann 창, 양끝 반사 패딩(`torch.stft(center=True)`)
//! - 파워 스펙트럼에 Slaney 멜 필터(`librosa.filters.mel`, 80 또는 128 밴드)를 곱함
//! - `log10(max(x, 1e-10))` → 전체 최댓값 − 8 아래는 잘라 냄 → `(x + 4) / 4`
//!
//! 오디오 뒤에 30초 무음을 붙여 계산하고(원본과 같음), 창마다 [`LogMel::window`]로 3000프레임(30초)을 꺼냅니다.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

pub const SAMPLE_RATE: usize = 16_000;
pub const N_FFT: usize = 400;
pub const HOP_LENGTH: usize = 160;
/// 한 창(30초)의 샘플 수
pub const WINDOW_SAMPLES: usize = 30 * SAMPLE_RATE;
/// 한 창(30초)의 멜 프레임 수. 인코더 입력 길이입니다.
pub const WINDOW_FRAMES: usize = WINDOW_SAMPLES / HOP_LENGTH;
const N_BINS: usize = N_FFT / 2 + 1;
/// 로그를 취하기 전 에너지의 하한 (원본 `clamp(min=1e-10)`)
const SILENCE_ENERGY: f32 = 1e-10;

/// 멜 밴드 하나의 필터: `start`부터 이어지는 주파수 칸의 가중치
#[derive(Debug, Clone)]
struct MelBand {
    start: usize,
    weights: Vec<f32>,
}

/// 로그 멜 특징 계산기. 모델을 불러올 때 한 번 만들어 재사용합니다.
pub struct MelExtractor {
    n_mels: usize,
    bands: Vec<MelBand>,
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
}

/// 오디오 전체의 정규화된 로그 멜. 값은 `[멜 밴드][프레임]` 순서입니다.
#[derive(Debug, Clone)]
pub struct LogMel {
    n_mels: usize,
    n_frames: usize,
    data: Vec<f32>,
}

impl MelExtractor {
    pub fn new(n_mels: usize) -> Self {
        let mut planner = FftPlanner::new();
        Self {
            n_mels,
            bands: mel_bands(n_mels),
            window: hann_window(N_FFT),
            fft: planner.plan_fft_forward(N_FFT),
        }
    }

    /// `samples`(16 kHz 모노) 뒤에 무음 `padding` 샘플을 붙여 로그 멜을 계산합니다.
    /// 프레임 수는 `(samples.len() + padding) / 160`입니다(원본처럼 마지막 프레임은 버림).
    ///
    /// 창(25 ms)이 모두 무음 패딩에 들어가는 프레임은 FFT를 돌리지 않고 무음 값(`log10(1e-10)`)으로 채웁니다.
    /// 그 값은 어떤 최댓값보다도 크지 않으므로 결과는 전부 계산한 것과 같습니다.
    pub fn log_mel(&self, samples: &[f32], padding: usize) -> LogMel {
        let len = samples.len() + padding;
        let n_frames = len / HOP_LENGTH;
        let silence = SILENCE_ENERGY.log10();
        let mut data = vec![silence; self.n_mels * n_frames];
        if n_frames == 0 {
            return LogMel {
                n_mels: self.n_mels,
                n_frames,
                data,
            };
        }

        let half = N_FFT / 2;
        let sample = |i: usize| samples.get(i).copied().unwrap_or(0.0);
        // 반사 패딩: 앞은 x[half - i], 뒤는 x[len - 2 - (i - len)]
        let padded = |i: usize| -> f32 {
            if i < half {
                sample(half - i)
            } else if i - half < len {
                sample(i - half)
            } else {
                let over = i - half - len;
                sample(len.saturating_sub(2 + over))
            }
        };

        let mut buffer = vec![Complex::new(0.0_f32, 0.0); N_FFT];
        let mut scratch = vec![Complex::new(0.0_f32, 0.0); self.fft.get_inplace_scratch_len()];
        let mut power = [0.0_f32; N_BINS];
        // 프레임 f의 창은 원래 오디오의 [160f − 200, 160f + 200)이므로, 160f − 200 ≥ 오디오 길이면 모두 무음입니다.
        // 패딩이 짧아 끝의 반사 패딩이 오디오를 되비추는 경우에도 이 범위는 모든 프레임을 덮습니다.
        let active = n_frames.min((samples.len() + half).div_ceil(HOP_LENGTH) + 1);
        let mut max = if active < n_frames {
            silence
        } else {
            f32::NEG_INFINITY
        };
        for frame in 0..active {
            let start = frame * HOP_LENGTH;
            for (i, value) in buffer.iter_mut().enumerate() {
                *value = Complex::new(padded(start + i) * self.window[i], 0.0);
            }
            self.fft.process_with_scratch(&mut buffer, &mut scratch);
            for (p, value) in power.iter_mut().zip(&buffer) {
                *p = value.norm_sqr();
            }
            for (mel, band) in self.bands.iter().enumerate() {
                let energy: f32 = band
                    .weights
                    .iter()
                    .zip(&power[band.start..])
                    .map(|(w, p)| w * p)
                    .sum();
                let log = energy.max(SILENCE_ENERGY).log10();
                max = max.max(log);
                data[mel * n_frames + frame] = log;
            }
        }

        let floor = max - 8.0;
        for value in &mut data {
            *value = (value.max(floor) + 4.0) / 4.0;
        }
        LogMel {
            n_mels: self.n_mels,
            n_frames,
            data,
        }
    }
}

impl LogMel {
    #[cfg(test)]
    pub fn n_frames(&self) -> usize {
        self.n_frames
    }

    #[cfg(test)]
    pub fn get(&self, mel: usize, frame: usize) -> f32 {
        self.data[mel * self.n_frames + frame]
    }

    /// 인코더 입력 한 창: 프레임 `frames`를 앞에서부터 채우고, 남는 칸(30초까지)은 0으로 둡니다.
    /// 원본 `transcribe`가 마지막 조각을 `pad_or_trim`으로 채우는 것과 같습니다.
    /// 값은 `[멜 밴드][3000 프레임]` 순서입니다.
    pub fn window(&self, frames: std::ops::Range<usize>) -> Vec<f32> {
        let start = frames.start.min(self.n_frames);
        let end = frames.end.min(self.n_frames).min(start + WINDOW_FRAMES);
        let mut out = vec![0.0_f32; self.n_mels * WINDOW_FRAMES];
        for mel in 0..self.n_mels {
            let row = &self.data[mel * self.n_frames..(mel + 1) * self.n_frames];
            out[mel * WINDOW_FRAMES..mel * WINDOW_FRAMES + (end - start)]
                .copy_from_slice(&row[start..end]);
        }
        out
    }
}

/// 주기형 Hann 창 (`torch.hann_window(n)`)
fn hann_window(length: usize) -> Vec<f32> {
    (0..length)
        .map(|i| {
            let phase = 2.0 * std::f64::consts::PI * i as f64 / length as f64;
            (0.5 - 0.5 * phase.cos()) as f32
        })
        .collect()
}

/// Slaney 멜 눈금 (librosa `htk=False`)
fn hz_to_mel(hz: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    let min_log_mel = MIN_LOG_HZ / F_SP;
    let logstep = 6.4_f64.ln() / 27.0;
    if hz >= MIN_LOG_HZ {
        min_log_mel + (hz / MIN_LOG_HZ).ln() / logstep
    } else {
        hz / F_SP
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    let min_log_mel = MIN_LOG_HZ / F_SP;
    let logstep = 6.4_f64.ln() / 27.0;
    if mel >= min_log_mel {
        MIN_LOG_HZ * (logstep * (mel - min_log_mel)).exp()
    } else {
        F_SP * mel
    }
}

/// `librosa.filters.mel(sr=16000, n_fft=400, n_mels, norm="slaney")`와 같은 필터를 0이 아닌 구간만 남겨 만듭니다.
fn mel_bands(n_mels: usize) -> Vec<MelBand> {
    let nyquist = SAMPLE_RATE as f64 / 2.0;
    let mel_max = hz_to_mel(nyquist);
    let points: Vec<f64> = (0..n_mels + 2)
        .map(|i| mel_to_hz(mel_max * i as f64 / (n_mels + 1) as f64))
        .collect();
    let bin_hz = |k: usize| k as f64 * SAMPLE_RATE as f64 / N_FFT as f64;

    (0..n_mels)
        .map(|m| {
            let (left, center, right) = (points[m], points[m + 1], points[m + 2]);
            let norm = 2.0 / (right - left);
            let weights: Vec<(usize, f32)> = (0..N_BINS)
                .filter_map(|k| {
                    let hz = bin_hz(k);
                    let lower = (hz - left) / (center - left);
                    let upper = (right - hz) / (right - center);
                    let weight = lower.min(upper).max(0.0) * norm;
                    (weight > 0.0).then_some((k, weight as f32))
                })
                .collect();
            match (weights.first(), weights.last()) {
                (Some(&(start, _)), Some(&(end, _))) => {
                    let mut dense = vec![0.0_f32; end - start + 1];
                    for (k, w) in weights {
                        dense[k - start] = w;
                    }
                    MelBand {
                        start,
                        weights: dense,
                    }
                }
                _ => MelBand {
                    start: 0,
                    weights: Vec::new(),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weight(bands: &[MelBand], mel: usize, bin: usize) -> f32 {
        let band = &bands[mel];
        bin.checked_sub(band.start)
            .and_then(|i| band.weights.get(i))
            .copied()
            .unwrap_or(0.0)
    }

    fn close(actual: f32, expected: f32, tolerance: f32) -> bool {
        (actual - expected).abs() <= tolerance
    }

    /// 기준값은 `librosa.filters.mel(sr=16000, n_fft=400, n_mels=…)`(librosa 0.11)의 각 행 최댓값입니다.
    #[test]
    fn mel_filters_match_librosa() {
        // (밴드 수, [(멜, 주파수 칸, 가중치)])
        type Case = (usize, &'static [(usize, usize, f32)]);
        let cases: [Case; 2] = [
            (
                80,
                &[
                    (0, 1, 2.486_259_5e-2),
                    (1, 2, 2.287_177_2e-2),
                    (10, 10, 1.990_821_8e-2),
                    (40, 43, 1.473_556_6e-2),
                    (79, 192, 3.164_711_9e-3),
                ],
            ),
            (
                128,
                &[
                    (0, 1, 1.237_398_7e-2),
                    (1, 1, 3.039_256_5e-2),
                    (10, 6, 1.128_918_4e-2),
                    (64, 43, 1.809_151_8e-2),
                    (127, 195, 5.041_601_6e-3),
                ],
            ),
        ];
        for (n_mels, expected) in cases {
            let bands = mel_bands(n_mels);
            assert_eq!(bands.len(), n_mels);
            for &(mel, bin, value) in expected {
                let actual = weight(&bands, mel, bin);
                assert!(
                    close(actual, value, value * 1e-4),
                    "{n_mels}밴드 [{mel}, {bin}]: {actual} ≠ {value}"
                );
            }
            // 0번 밴드는 0 Hz 칸에 가중치가 없고, 모든 밴드에 가중치가 하나 이상 있습니다.
            assert_eq!(weight(&bands, 0, 0), 0.0);
            assert!(bands.iter().all(|b| !b.weights.is_empty()));
        }
    }

    /// 1초 신호(440 Hz + 1250 Hz, 뒤 절반은 1/10 크기)의 로그 멜.
    /// 기준값은 librosa `stft(center=True, pad_mode="reflect", window="hann")`에 Whisper 정규화를 한 값입니다.
    fn test_signal() -> Vec<f32> {
        (0..SAMPLE_RATE)
            .map(|i| {
                let t = i as f64 / SAMPLE_RATE as f64;
                let tau = 2.0 * std::f64::consts::PI;
                let value = 0.5 * (tau * 440.0 * t).sin() + 0.25 * (tau * 1250.0 * t).sin();
                let gain = if i >= SAMPLE_RATE / 2 { 0.1 } else { 1.0 };
                (value * gain) as f32
            })
            .collect()
    }

    #[test]
    fn log_mel_matches_whisper_reference() {
        // (밴드 수, 최솟값, [(멜, 프레임, 값)])
        type Case = (usize, f32, &'static [(usize, usize, f32)]);
        let cases: [Case; 2] = [
            (
                80,
                -0.561_794,
                &[
                    (10, 0, 1.343_484),
                    (26, 0, 0.694_939),
                    (11, 10, 1.438_201),
                    (26, 10, -0.187_793),
                    (26, 49, 0.038_728),
                    (11, 50, 1.311_78),
                    (26, 50, 0.521_593),
                    (11, 75, 0.938_206),
                    (26, 99, -0.272_702),
                ],
            ),
            (
                128,
                -0.514_643,
                &[
                    (16, 0, 1.414_11),
                    (42, 0, 0.705_789),
                    (18, 10, 1.485_352),
                    (42, 10, -0.188_381),
                    (42, 49, 0.052_426),
                    (18, 50, 1.355_726),
                    (42, 50, 0.532_438),
                    (18, 75, 0.985_357),
                    (42, 99, -0.257_687),
                ],
            ),
        ];
        let signal = test_signal();
        for (n_mels, floor, expected) in cases {
            let mel = MelExtractor::new(n_mels).log_mel(&signal, 0);
            assert_eq!(mel.n_frames(), 100);
            for &(m, frame, value) in expected {
                let actual = mel.get(m, frame);
                assert!(
                    close(actual, value, 1e-3),
                    "{n_mels}밴드 [{m}, {frame}]: {actual} ≠ {value}"
                );
            }
            let min = mel.data.iter().copied().fold(f32::INFINITY, f32::min);
            assert!(close(min, floor, 1e-4), "{n_mels}밴드 최솟값: {min}");
        }
    }

    #[test]
    fn padding_adds_silent_frames() {
        let extractor = MelExtractor::new(80);
        let signal = test_signal();
        let mel = extractor.log_mel(&signal, WINDOW_SAMPLES);
        assert_eq!(mel.n_frames(), 100 + WINDOW_FRAMES);
        // 무음 구간은 바닥값(최댓값 − 8)입니다.
        let floor = mel.get(0, mel.n_frames() - 1);
        assert!(mel.data.iter().all(|&v| v >= floor - 1e-6));
        assert!(mel.data.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn skipped_silent_frames_match_full_computation() {
        let extractor = MelExtractor::new(80);
        let signal = test_signal();
        let skipped = extractor.log_mel(&signal, WINDOW_SAMPLES);
        // 무음을 오디오에 직접 붙이면 모든 프레임을 FFT로 계산합니다.
        let mut padded = signal.clone();
        padded.resize(signal.len() + WINDOW_SAMPLES, 0.0);
        let full = extractor.log_mel(&padded, 0);
        assert_eq!(skipped.n_frames(), full.n_frames());
        for (i, (a, b)) in skipped.data.iter().zip(&full.data).enumerate() {
            assert!(close(*a, *b, 1e-6), "{i}번째 값: {a} ≠ {b}");
        }
    }

    #[test]
    fn window_is_zero_padded_to_thirty_seconds() {
        let extractor = MelExtractor::new(80);
        let mel = extractor.log_mel(&test_signal(), WINDOW_SAMPLES);
        let window = mel.window(10..60);
        assert_eq!(window.len(), 80 * WINDOW_FRAMES);
        assert_eq!(window[0], mel.get(0, 10));
        assert_eq!(window[49], mel.get(0, 59));
        assert_eq!(window[WINDOW_FRAMES + 3], mel.get(1, 13));
        assert!(window[50..WINDOW_FRAMES].iter().all(|&v| v == 0.0));
        // 범위가 30초보다 길거나 끝을 넘으면 잘라 냅니다.
        assert_eq!(
            mel.window(0..usize::MAX)[WINDOW_FRAMES - 1],
            mel.get(0, WINDOW_FRAMES - 1)
        );
    }

    #[test]
    fn empty_or_tiny_audio_does_not_panic() {
        let extractor = MelExtractor::new(80);
        assert_eq!(extractor.log_mel(&[], 0).n_frames(), 0);
        let mel = extractor.log_mel(&[0.1; 5], WINDOW_SAMPLES);
        assert_eq!(mel.n_frames(), WINDOW_FRAMES);
        assert!(mel.window(0..1).iter().all(|v| v.is_finite()));
    }
}
