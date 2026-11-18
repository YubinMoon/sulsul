//! 말소리 찾기: 녹음 앞뒤의 말소리가 없는 부분을 잘라 내고, 말소리가 없는 녹음을 가려냅니다.
//!
//! 소리 크기나 울림을 재는 규칙은 방마다 다른 잡음(키보드, 선풍기, 마이크 자동 음량 조절 등)에 자주 속아서,
//! 말소리와 잡음을 가르도록 학습된 [Silero VAD](https://github.com/snakers4/silero-vad) v5.1.2 모델을 씁니다.
//!
//! - 모델(`model/silero_vad.onnx`, 2.3MB)은 실행 파일에 넣어 두어 내려받지 않습니다.
//!   MIT 라이선스이고 원문은 `model/silero_vad-LICENSE.txt`에 있습니다.
//! - 소리를 16kHz로 줄여 32ms(512샘플)씩 넣고, 조각마다 말소리일 확률을 받습니다.
//! - 확률이 [`SPEECH_THRESHOLD`] 이상이면 말이 시작되고, [`SILENCE_THRESHOLD`] 미만이
//!   [`MIN_SILENCE_SECONDS`] 넘게 이어지면 끝납니다 (Silero의 `get_speech_timestamps`와 같은 방식).
//!   [`MIN_SPEECH_SECONDS`]보다 짧은 말은 버립니다.
//! - 첫 말 앞에 [`LEAD_SECONDS`], 끝 말 뒤에 [`TRAIL_SECONDS`]를 남기고 바깥을 잘라 냅니다.
//!   작게 끝나는 받침이나 숨소리를 잘라 먹지 않도록 여유는 넉넉히 두고, 말 사이의 쉼은 그대로 둡니다.
//!
//! ONNX Runtime 환경은 프로세스에 하나만 만들 수 있어서 SenseVoice 엔진의 환경을 함께 씁니다.
//! 세션은 처음 부를 때 한 번 만들고 계속 씁니다.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use ndarray::{Array0, Array2, Array3, ArrayViewD};
use ort::inputs;
use ort::session::Session;
use ort::value::TensorRef;
use sulsul_core::audio::{AudioClip, AudioError};

/// Silero VAD v5.1.2 (`src/silero_vad/data/silero_vad.onnx`,
/// SHA-256 `2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f`)
static MODEL: &[u8] = include_bytes!("../model/silero_vad.onnx");

/// 모델이 받는 샘플레이트
const VAD_RATE: u32 = 16_000;
/// 한 번에 넣는 샘플 수 (32ms)
const CHUNK: usize = 512;
/// 앞 조각 끝에서 가져와 붙이는 샘플 수 (모델이 요구함)
const CONTEXT: usize = 64;
/// 모델의 상태 크기 `[2, 1, 128]`
const STATE_SHAPE: (usize, usize, usize) = (2, 1, 128);

/// 확률이 이 이상이면 말이 시작됩니다
pub const SPEECH_THRESHOLD: f32 = 0.5;
/// 말하는 중에 확률이 이 미만으로 내려가면 말이 끝난 것으로 봅니다
pub const SILENCE_THRESHOLD: f32 = 0.35;
/// 말이 끝났다고 보려면 낮은 확률이 이만큼 이어져야 합니다
pub const MIN_SILENCE_SECONDS: f64 = 0.1;
/// 이보다 짧은 말은 버립니다
pub const MIN_SPEECH_SECONDS: f64 = 0.1;
/// 첫 말 앞에 남기는 여유
pub const LEAD_SECONDS: f64 = 0.3;
/// 끝 말 뒤에 남기는 여유
pub const TRAIL_SECONDS: f64 = 0.5;

#[derive(Debug, thiserror::Error)]
pub enum VadError {
    #[error("could not load the speech detector: {0}")]
    Load(String),
    #[error("speech detection failed: {0}")]
    Run(String),
    #[error(transparent)]
    Audio(#[from] AudioError),
}

/// [`find_speech`]의 결과
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechSpan {
    /// 앞뒤 여유를 넣은 말소리 범위 (클립의 샘플 위치). [`AudioClip::keep_range`]에 넘깁니다.
    pub range: Range<usize>,
    /// 말로 본 구간들 (클립의 샘플 위치, 여유 없음)
    pub segments: Vec<Range<usize>>,
}

/// 말소리가 있는 범위를 찾습니다. 말소리가 없으면 `None`입니다.
pub fn find_speech(clip: &AudioClip) -> Result<Option<SpeechSpan>, VadError> {
    let started = Instant::now();
    let samples = to_vad_rate(clip)?;
    let probabilities = with_detector(|detector| detector.probabilities(&samples))?;
    let span = span_from_probabilities(&probabilities, clip.sample_rate, clip.samples.len());
    let rate = f64::from(clip.sample_rate);
    match &span {
        Some(span) => tracing::info!(
            start_seconds = span.range.start as f64 / rate,
            end_seconds = span.range.end as f64 / rate,
            segments = span.segments.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "speech found"
        ),
        None => tracing::info!(
            seconds = clip.duration().as_secs_f64(),
            max_probability = probabilities.iter().copied().fold(0.0, f32::max),
            elapsed_ms = started.elapsed().as_millis(),
            "no speech found"
        ),
    }
    Ok(span)
}

/// 소리를 모델이 받는 16kHz로 바꿉니다.
///
/// 줄일 때는 출력 샘플 하나에 해당하는 입력 샘플들을 평균 냅니다. 평균 내기가 높은 소리를 대강 걸러 주니
/// 판정에는 이 정도로 충분하고, 받아쓰기용 고품질 리샘플링([`AudioClip::resampled`])보다 열 배 넘게 빠릅니다.
fn to_vad_rate(clip: &AudioClip) -> Result<Cow<'_, [f32]>, VadError> {
    let rate = clip.sample_rate;
    if rate == VAD_RATE {
        return Ok(Cow::Borrowed(&clip.samples));
    }
    if rate < VAD_RATE {
        return Ok(Cow::Owned(clip.resampled(VAD_RATE)?.samples));
    }
    let ratio = f64::from(rate) / f64::from(VAD_RATE);
    let samples = &clip.samples;
    let len = (samples.len() as f64 / ratio) as usize;
    Ok(Cow::Owned(
        (0..len)
            .map(|index| {
                let start = (index as f64 * ratio) as usize;
                let end = (((index + 1) as f64 * ratio) as usize).clamp(start + 1, samples.len());
                samples[start..end].iter().sum::<f32>() / (end - start) as f32
            })
            .collect(),
    ))
}

/// 모델을 미리 불러 둡니다. 처음 [`find_speech`]를 부를 때 불러오는 시간(0.1~0.2초)을 아낍니다.
pub fn preload() -> Result<(), VadError> {
    with_detector(|_| Ok(()))
}

/// 조각별 확률을 클립의 말소리 범위로 바꿉니다.
fn span_from_probabilities(
    probabilities: &[f32],
    sample_rate: u32,
    len: usize,
) -> Option<SpeechSpan> {
    let segments = speech_segments(probabilities);
    let (first, last) = (segments.first()?, segments.last()?);
    // 조각 번호 → 클립의 샘플 위치
    let to_clip = |chunk: usize| {
        let position = chunk as u64 * CHUNK as u64 * u64::from(sample_rate) / u64::from(VAD_RATE);
        (position as usize).min(len)
    };
    let margin = |seconds: f64| (seconds * f64::from(sample_rate)).round() as usize;
    let start = to_clip(first.start).saturating_sub(margin(LEAD_SECONDS));
    let end = (to_clip(last.end) + margin(TRAIL_SECONDS)).min(len);
    Some(SpeechSpan {
        range: start..end,
        segments: segments
            .iter()
            .map(|segment| to_clip(segment.start)..to_clip(segment.end))
            .collect(),
    })
}

/// 조각별 확률에서 말로 볼 구간들 (조각 번호 범위)
fn speech_segments(probabilities: &[f32]) -> Vec<Range<usize>> {
    let chunks = |seconds: f64| (seconds * f64::from(VAD_RATE) / CHUNK as f64).round() as usize;
    let (min_silence, min_speech) = (chunks(MIN_SILENCE_SECONDS), chunks(MIN_SPEECH_SECONDS));
    let mut segments = Vec::new();
    // 말하는 중이면 그 말이 시작한 조각
    let mut start: Option<usize> = None;
    // 말하는 중에 확률이 내려갔으면 내려간 첫 조각
    let mut quiet_since: Option<usize> = None;
    let mut push = |segment: Range<usize>| {
        if segment.len() >= min_speech.max(1) {
            segments.push(segment);
        }
    };
    for (index, &probability) in probabilities.iter().enumerate() {
        match start {
            None if probability >= SPEECH_THRESHOLD => start = Some(index),
            None => {}
            Some(_) if probability >= SPEECH_THRESHOLD => quiet_since = None,
            Some(begin) if probability < SILENCE_THRESHOLD => {
                let since = *quiet_since.get_or_insert(index);
                if index + 1 - since >= min_silence.max(1) {
                    push(begin..since);
                    start = None;
                    quiet_since = None;
                }
            }
            // 두 기준 사이의 애매한 확률은 말이 이어지는 것으로 봅니다.
            Some(_) => {}
        }
    }
    if let Some(begin) = start {
        push(begin..quiet_since.unwrap_or(probabilities.len()));
    }
    segments
}

static DETECTOR: Mutex<Option<Detector>> = Mutex::new(None);

/// 공용 감지기로 `run`을 실행합니다. 처음 부를 때 모델을 불러옵니다.
fn with_detector<T>(run: impl FnOnce(&mut Detector) -> Result<T, VadError>) -> Result<T, VadError> {
    let mut detector = DETECTOR.lock().unwrap_or_else(PoisonError::into_inner);
    if detector.is_none() {
        *detector = Some(Detector::load()?);
    }
    run(detector.as_mut().expect("방금 넣었습니다"))
}

struct Detector {
    session: Session,
}

impl Detector {
    fn load() -> Result<Self, VadError> {
        let started = Instant::now();
        let env = sulsul_stt_sensevoice::ort_environment().map_err(load_error)?;
        // 작은 모델을 조금씩 여러 번 돌리므로 스레드 하나가 가장 빠릅니다 (Silero 안내).
        let session = Session::builder(env)
            .map_err(load_error)?
            .with_intra_threads(1)
            .map_err(load_error)?
            .with_inter_threads(1)
            .map_err(load_error)?
            .commit_from_memory(MODEL)
            .map_err(load_error)?;
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            "speech detector loaded"
        );
        Ok(Self { session })
    }

    /// 16kHz 소리의 조각(512샘플)마다 말소리일 확률. 마지막 조각은 0으로 채워 넣습니다.
    fn probabilities(&mut self, samples: &[f32]) -> Result<Vec<f32>, VadError> {
        let mut input = Array2::<f32>::zeros((1, CONTEXT + CHUNK));
        let mut state = Array3::<f32>::zeros(STATE_SHAPE);
        let rate = Array0::from_elem((), i64::from(VAD_RATE));
        let mut probabilities = Vec::with_capacity(samples.len().div_ceil(CHUNK));
        for chunk in samples.chunks(CHUNK) {
            {
                let buffer = input.as_slice_mut().expect("표준 배치입니다");
                // 앞 조각의 끝을 맥락으로 앞에 둡니다 (처음에는 0).
                buffer.copy_within(CHUNK.., 0);
                buffer[CONTEXT..CONTEXT + chunk.len()].copy_from_slice(chunk);
                buffer[CONTEXT + chunk.len()..].fill(0.0);
            }
            let outputs = self
                .session
                .run(inputs![
                    "input" => TensorRef::from_array_view(input.view()).map_err(run_error)?,
                    "state" => TensorRef::from_array_view(state.view()).map_err(run_error)?,
                    "sr" => TensorRef::from_array_view(rate.view()).map_err(run_error)?,
                ])
                .map_err(run_error)?;
            let probability: ArrayViewD<f32> = outputs["output"]
                .try_extract_array::<f32>()
                .map_err(run_error)?;
            probabilities.push(probability.iter().next().copied().unwrap_or(0.0));
            let next_state = outputs["stateN"]
                .try_extract_array::<f32>()
                .map_err(run_error)?;
            state
                .as_slice_mut()
                .expect("표준 배치입니다")
                .iter_mut()
                .zip(next_state.iter())
                .for_each(|(slot, value)| *slot = *value);
        }
        Ok(probabilities)
    }
}

fn load_error(error: impl std::fmt::Display) -> VadError {
    VadError::Load(error.to_string())
}

fn run_error(error: impl std::fmt::Display) -> VadError {
    VadError::Run(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 앞뒤에 무음을 붙인 시험용 말소리 (Windows 음성 합성으로 만든 파일)
    fn speech(name: &str, before: f32, after: f32) -> AudioClip {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name);
        let clip = sulsul_core::audio::read_wav(&path).expect("시험용 말소리를 읽어야 합니다");
        let rate = clip.sample_rate as f32;
        let mut samples = vec![0.0; (before * rate) as usize];
        samples.extend(&clip.samples);
        samples.extend(std::iter::repeat_n(0.0, (after * rate) as usize));
        AudioClip::new(samples, clip.sample_rate)
    }

    /// 범위들을 비교하기 쉬운 (시작, 끝) 쌍으로
    fn pairs(ranges: &[Range<usize>]) -> Vec<(usize, usize)> {
        ranges
            .iter()
            .map(|range| (range.start, range.end))
            .collect()
    }

    fn ranges(probabilities: &[f32]) -> Vec<(usize, usize)> {
        pairs(&speech_segments(probabilities))
    }

    #[test]
    fn speech_segments_follow_the_thresholds() {
        let (hi, mid, lo) = (0.9, 0.4, 0.1);
        // 말 사이에 짧게 내려간 확률(2조각)은 이어진 말, 애매한 확률은 말이 이어지는 것으로 봅니다.
        assert_eq!(
            ranges(&[lo, hi, hi, lo, lo, hi, mid, mid, hi, lo, lo, lo, lo]),
            [(1, 9)]
        );
        // 3조각(약 0.1초) 넘게 내려가면 끊습니다.
        assert_eq!(
            ranges(&[hi, hi, hi, lo, lo, lo, hi, hi, hi]),
            [(0, 3), (6, 9)]
        );
        // 끝까지 말하면 끝까지
        assert_eq!(ranges(&[lo, hi, hi, hi]), [(1, 4)]);
    }

    #[test]
    fn short_bursts_are_dropped() {
        let (hi, lo) = (0.9, 0.1);
        assert!(ranges(&[lo, hi, hi, lo, lo, lo, lo]).is_empty());
        assert_eq!(ranges(&[lo, hi, hi, hi, lo, lo, lo]), [(1, 4)]);
        assert!(ranges(&[]).is_empty());
    }

    #[test]
    fn margins_are_added_and_clamped() {
        let (hi, lo) = (0.9, 0.1);
        let mut probabilities = vec![lo; 100];
        probabilities[40..60].fill(hi);
        let span = span_from_probabilities(&probabilities, VAD_RATE, 100 * CHUNK).unwrap();
        assert_eq!(pairs(&span.segments), [(40 * CHUNK, 60 * CHUNK)]);
        assert_eq!(span.range, 40 * CHUNK - 4_800..60 * CHUNK + 8_000);

        // 48kHz 클립이면 위치를 세 배로 바꿉니다. 여유는 클립 끝에서 멈춥니다.
        let mut probabilities = vec![hi; 10];
        probabilities.extend([lo; 3]);
        let len = 13 * CHUNK * 3;
        let span = span_from_probabilities(&probabilities, 48_000, len).unwrap();
        assert_eq!(pairs(&span.segments), [(0, 10 * CHUNK * 3)]);
        assert_eq!(span.range, 0..len);
    }

    #[test]
    fn speech_is_found_in_a_recording() {
        let clip = speech("hello_ko.wav", 1.5, 1.5);
        let span = find_speech(&clip).unwrap().expect("말소리를 찾아야 합니다");
        let seconds = |position: usize| position as f64 / f64::from(clip.sample_rate);
        let total = clip.duration().as_secs_f64();
        let (first, last) = (&span.segments[0], span.segments.last().unwrap());
        assert!(
            seconds(first.start) >= 1.5 && seconds(last.end) <= total - 1.5,
            "앞뒤에 붙인 무음은 말이 아닙니다: {span:?}"
        );
        assert!(
            seconds(span.range.start) >= 1.0 && seconds(span.range.end) <= total - 1.0,
            "앞뒤의 무음은 거의 잘라 내야 합니다: {span:?}"
        );
    }

    #[test]
    fn short_word_is_found() {
        let clip = speech("yes_ko.wav", 1.0, 1.0);
        assert!(find_speech(&clip).unwrap().is_some());
    }

    #[test]
    fn speech_at_another_rate_is_found() {
        let clip = speech("hello_ko.wav", 1.0, 1.0).resampled(48_000).unwrap();
        let span = find_speech(&clip).unwrap().expect("말소리를 찾아야 합니다");
        assert!(span.range.start > 24_000, "{span:?}");
    }

    #[test]
    fn silence_has_no_speech() {
        for (what, clip) in [
            ("무음", AudioClip::new(vec![0.0; 32_000], VAD_RATE)),
            ("빈 클립", AudioClip::new(Vec::new(), VAD_RATE)),
            (
                "조각 하나보다 짧은 클립",
                AudioClip::new(vec![0.0; 100], VAD_RATE),
            ),
        ] {
            assert_eq!(find_speech(&clip).unwrap(), None, "{what}");
        }
    }
}
