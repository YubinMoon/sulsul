//! SenseVoice ONNX 추론을 위한 80차원 FBANK + LFR + CMVN 특징 추출.

use std::f32::consts::PI;

use ndarray::{Array1, Array2};
use rustfft::{FftPlanner, num_complex::Complex};

const SAMPLE_RATE: f32 = 16_000.0;
const NUM_MELS: usize = 80;
const FRAME_LENGTH: usize = 400; // 25ms @ 16kHz
const FRAME_SHIFT: usize = 160; // 10ms @ 16kHz
const PRE_EMPHASIS: f32 = 0.97;
const F_MIN: f32 = 20.0;

/// SenseVoice 입력 텐서에 맞게 `[T_lfr, 80 * lfr_window_size]` 특징 행렬을 만듭니다.
pub fn extract_features(
    samples: &[f32],
    normalize_samples: bool,
    lfr_window_size: usize,
    lfr_window_shift: usize,
    neg_mean: &Array1<f32>,
    inv_stddev: &Array1<f32>,
) -> Array2<f32> {
    let fbank = compute_fbank(samples, normalize_samples);
    let mut stacked = apply_lfr(&fbank, lfr_window_size, lfr_window_shift);
    if stacked.nrows() == 0 {
        return stacked;
    }
    if !neg_mean.is_empty()
        && neg_mean.len() == stacked.ncols()
        && inv_stddev.len() == stacked.ncols()
    {
        apply_cmvn(&mut stacked, neg_mean, inv_stddev);
    }
    stacked
}

fn compute_fbank(samples: &[f32], normalize_samples: bool) -> Array2<f32> {
    if samples.len() < FRAME_LENGTH {
        return Array2::zeros((0, NUM_MELS));
    }

    let scaled: Vec<f32> = if normalize_samples {
        samples.to_vec()
    } else {
        samples.iter().map(|&s| s * 32_768.0).collect()
    };

    let num_frames = 1 + (scaled.len() - FRAME_LENGTH) / FRAME_SHIFT;
    let fft_size = FRAME_LENGTH.next_power_of_two();
    let num_fft_bins = fft_size / 2 + 1;

    let window = hamming_window(FRAME_LENGTH);
    let mel_banks = mel_filterbank(NUM_MELS, fft_size, SAMPLE_RATE, F_MIN, SAMPLE_RATE / 2.0);
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(fft_size);

    let mut features = Array2::zeros((num_frames, NUM_MELS));
    let mut frame = vec![0.0_f32; FRAME_LENGTH];
    let mut fft_buf = vec![Complex::new(0.0_f32, 0.0_f32); fft_size];

    for i in 0..num_frames {
        let start = i * FRAME_SHIFT;
        frame.copy_from_slice(&scaled[start..start + FRAME_LENGTH]);

        for j in (1..FRAME_LENGTH).rev() {
            frame[j] -= PRE_EMPHASIS * frame[j - 1];
        }
        frame[0] *= 1.0 - PRE_EMPHASIS;

        for j in 0..FRAME_LENGTH {
            frame[j] *= window[j];
            fft_buf[j] = Complex::new(frame[j], 0.0);
        }
        fft_buf[FRAME_LENGTH..fft_size].fill(Complex::new(0.0, 0.0));
        fft.process(&mut fft_buf);

        for m in 0..NUM_MELS {
            let energy: f32 = (0..num_fft_bins)
                .map(|k| mel_banks[[m, k]] * fft_buf[k].norm_sqr())
                .sum::<f32>()
                .max(1.0e-10);
            features[[i, m]] = energy.ln();
        }
    }

    features
}

fn apply_lfr(features: &Array2<f32>, window_size: usize, window_shift: usize) -> Array2<f32> {
    let in_frames = features.nrows();
    let in_dim = features.ncols();
    if in_frames < window_size || window_size == 0 || window_shift == 0 {
        return Array2::zeros((0, in_dim * window_size));
    }

    let out_frames = (in_frames - window_size) / window_shift + 1;
    let out_dim = in_dim * window_size;
    let mut out = Array2::zeros((out_frames, out_dim));

    for i in 0..out_frames {
        let src_start = i * window_shift;
        for w in 0..window_size {
            let src_row = features.row(src_start + w);
            let dst_start = w * in_dim;
            for (j, &val) in src_row.iter().enumerate() {
                out[[i, dst_start + j]] = val;
            }
        }
    }

    out
}

fn apply_cmvn(features: &mut Array2<f32>, neg_mean: &Array1<f32>, inv_stddev: &Array1<f32>) {
    let dim = features.ncols();
    for mut row in features.rows_mut() {
        for j in 0..dim {
            row[j] = (row[j] + neg_mean[j]) * inv_stddev[j];
        }
    }
}

fn hamming_window(length: usize) -> Vec<f32> {
    (0..length)
        .map(|i| 0.54 - 0.46 * (2.0 * PI * i as f32 / (length as f32 - 1.0)).cos())
        .collect()
}

fn mel_filterbank(
    num_mels: usize,
    fft_size: usize,
    sample_rate: f32,
    low_freq: f32,
    high_freq: f32,
) -> Array2<f32> {
    let num_fft_bins = fft_size / 2 + 1;
    let mel_low = hz_to_mel(low_freq);
    let mel_high = hz_to_mel(high_freq);

    let num_points = num_mels + 2;
    let bin_points: Vec<f32> = (0..num_points)
        .map(|i| {
            let mel = mel_low + (mel_high - mel_low) * i as f32 / (num_points - 1) as f32;
            mel_to_hz(mel) * fft_size as f32 / sample_rate
        })
        .collect();

    let mut banks = Array2::zeros((num_mels, num_fft_bins));
    for m in 0..num_mels {
        let (left, center, right) = (bin_points[m], bin_points[m + 1], bin_points[m + 2]);
        for k in 0..num_fft_bins {
            let kf = k as f32;
            if kf > left && kf < center {
                banks[[m, k]] = (kf - left) / (center - left);
            } else if kf >= center && kf < right {
                banks[[m, k]] = (right - kf) / (right - center);
            }
        }
    }
    banks
}

fn hz_to_mel(hz: f32) -> f32 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * ((mel / 1127.0).exp() - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_features_matches_expected_lfr_dimensions() {
        let samples = vec![0.01_f32; 16_000];
        let feats = extract_features(&samples, false, 7, 6, &Array1::zeros(0), &Array1::zeros(0));
        assert_eq!(feats.ncols(), 80 * 7);
        assert!(feats.nrows() > 0);
        assert!(feats.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn too_short_audio_returns_empty_features() {
        let feats = extract_features(
            &[0.0; 100],
            false,
            7,
            6,
            &Array1::zeros(0),
            &Array1::zeros(0),
        );
        assert_eq!(feats.nrows(), 0);
    }
}
