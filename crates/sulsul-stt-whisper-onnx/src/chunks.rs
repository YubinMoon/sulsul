//! 긴 오디오를 30초 이하 창으로 나누기.
//!
//! Whisper 인코더는 한 번에 30초까지만 봅니다. 30초보다 긴 오디오는 창 끝 5초 안에서 가장 조용한 곳을 골라
//! 자르므로 낱말이 창 경계에서 잘리는 일이 줄어듭니다. 창은 겹치지 않고 빈틈 없이 이어집니다.

use std::ops::Range;

use crate::features::{SAMPLE_RATE, WINDOW_SAMPLES};

/// 자를 곳을 찾는 구간 길이: 창 끝에서 이만큼 앞까지 봅니다.
const SEARCH_SAMPLES: usize = 5 * SAMPLE_RATE;
/// 소리 크기를 재는 단위 (20 ms)
const ENERGY_FRAME: usize = SAMPLE_RATE / 50;

/// `samples`를 차례로 인식할 창(샘플 범위)으로 나눕니다. 비어 있으면 빈 목록입니다.
pub fn split(samples: &[f32]) -> Vec<Range<usize>> {
    let mut windows = Vec::new();
    let mut start = 0;
    while samples.len() - start > WINDOW_SAMPLES {
        let cut = quietest_cut(
            samples,
            start + WINDOW_SAMPLES - SEARCH_SAMPLES,
            start + WINDOW_SAMPLES,
        );
        windows.push(start..cut);
        start = cut;
    }
    if start < samples.len() {
        windows.push(start..samples.len());
    }
    windows
}

/// `[from, to)`에서 20 ms 단위 에너지가 가장 작은 곳의 끝을 자를 위치로 돌려줍니다.
/// 같으면 뒤쪽(창이 더 긴 쪽)을 고릅니다. 결과는 `(from, to]` 안입니다.
fn quietest_cut(samples: &[f32], from: usize, to: usize) -> usize {
    let mut best = to;
    let mut best_energy = f32::INFINITY;
    let mut end = to;
    while end >= from + ENERGY_FRAME {
        let energy: f32 = samples[end - ENERGY_FRAME..end].iter().map(|s| s * s).sum();
        if energy < best_energy {
            best_energy = energy;
            best = end;
        }
        end -= ENERGY_FRAME;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(len: usize) -> Vec<f32> {
        (0..len).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect()
    }

    fn assert_contiguous(windows: &[Range<usize>], len: usize) {
        assert_eq!(windows.first().map(|w| w.start), Some(0));
        assert_eq!(windows.last().map(|w| w.end), Some(len));
        for pair in windows.windows(2) {
            assert_eq!(
                pair[0].end, pair[1].start,
                "창 사이에 빈틈이나 겹침이 있습니다"
            );
        }
        for window in windows {
            assert!(!window.is_empty());
            assert!(
                window.len() <= WINDOW_SAMPLES,
                "창이 30초보다 깁니다: {window:?}"
            );
        }
    }

    #[test]
    fn short_audio_is_one_window() {
        assert!(split(&[]).is_empty());
        assert_eq!(split(&[0.1; 100]), vec![0..100]);
        let exact = vec![0.1; WINDOW_SAMPLES];
        assert_eq!(split(&exact), vec![0..WINDOW_SAMPLES]);
    }

    #[test]
    fn long_audio_is_cut_at_the_quietest_point() {
        // 27초 지점에 0.2초 무음이 있으면 거기서 자릅니다.
        let len = 70 * SAMPLE_RATE;
        let mut samples = tone(len);
        let gap = 27 * SAMPLE_RATE;
        samples[gap..gap + SAMPLE_RATE / 5].fill(0.0);
        let windows = split(&samples);
        assert_contiguous(&windows, len);
        assert_eq!(windows.len(), 3);
        let cut = windows[0].end;
        assert!(
            (gap + ENERGY_FRAME..=gap + SAMPLE_RATE / 5).contains(&cut),
            "무음 구간이 아닌 곳에서 잘랐습니다: {cut}"
        );
    }

    #[test]
    fn uniform_audio_uses_full_windows() {
        // 어디나 같은 크기면 창을 가득 채웁니다 (같으면 뒤쪽을 고름).
        let len = 2 * WINDOW_SAMPLES + 10;
        let windows = split(&vec![0.0; len]);
        assert_eq!(
            windows,
            vec![
                0..WINDOW_SAMPLES,
                WINDOW_SAMPLES..2 * WINDOW_SAMPLES,
                2 * WINDOW_SAMPLES..len
            ]
        );
    }

    #[test]
    fn max_recording_length_is_covered() {
        // 최대 녹음 길이(기본 120초)보다 조금 긴 오디오도 모두 덮습니다.
        let len = 125 * SAMPLE_RATE + 123;
        let windows = split(&tone(len));
        assert_contiguous(&windows, len);
        assert!(windows.len() >= 5);
    }
}
