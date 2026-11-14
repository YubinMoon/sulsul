//! whisper.cpp로 실제 인식을 수행하는 엔진.

use std::borrow::Cow;
use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use std::path::Path;
use std::time::Duration;

use sulsul_core::CancelToken;
use sulsul_core::audio::AudioClip;
use sulsul_core::stt::{
    ComputeDevice, Language, Segment, SttEngine, SttError, TranscribeRequest, Transcript,
    WHISPER_SAMPLE_RATE,
};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    whisper_rs_sys as sys,
};

/// 자동으로 정할 때 쓰는 스레드 수의 상한. 그보다 많이 써도 거의 빨라지지 않습니다.
const MAX_AUTO_THREADS: usize = 8;

/// 이보다 짧은 오디오는 뒤에 무음을 붙여 이 길이로 맞춥니다 (1초).
///
/// whisper.cpp 1.8.3의 `whisper_full_with_state`는 입력이 100ms(멜 프레임 10개)보다
/// 짧으면 "input is too short" 경고만 남기고 결과 없이 0을 돌려주고, 처리 루프도
/// 남은 길이가 100ms 이하면 그대로 끝납니다. 어차피 내부에서 30초 창으로 채워
/// 처리하므로 무음을 붙여도 계산량은 같습니다.
const MIN_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize;

/// 엔진 동작을 정하는 값 (설정에서 옴)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EngineOptions {
    pub use_gpu: bool,
    /// 0이면 자동
    pub threads: usize,
    /// 1이면 greedy, 2 이상이면 beam search
    pub beam_size: i32,
}

pub(crate) struct WhisperEngine {
    // 상태가 컨텍스트를 붙잡고 있지만, 모델의 수명이 엔진과 같다는 것을 드러내려고 함께 둡니다.
    _context: WhisperContext,
    state: WhisperState,
    device: ComputeDevice,
    threads: usize,
    beam_size: i32,
}

impl WhisperEngine {
    /// 모델을 읽어 엔진을 만듭니다. GPU로 만들지 못하면 CPU로 한 번 더 시도합니다.
    pub(crate) fn load(model_path: &Path, options: EngineOptions) -> Result<Self, SttError> {
        // GPU 백엔드 없이 빌드했으면 켜 봐야 쓸 장치가 없습니다.
        let use_gpu = options.use_gpu && crate::compiled_gpu_backend().is_some();

        let (context, state, used_gpu) = match open(model_path, use_gpu) {
            Ok((context, state)) => (context, state, use_gpu),
            Err(error) if use_gpu => {
                tracing::warn!(
                    "failed to load whisper model with GPU ({error}), falling back to CPU"
                );
                let (context, state) = open(model_path, false).map_err(backend_error)?;
                (context, state, false)
            }
            Err(error) => return Err(backend_error(error)),
        };

        let device = if used_gpu {
            gpu_device_name().map_or(ComputeDevice::Cpu, ComputeDevice::Gpu)
        } else {
            ComputeDevice::Cpu
        };
        tracing::info!(
            "whisper model loaded: {} (device: {device:?})",
            model_path.display()
        );

        Ok(Self {
            _context: context,
            state,
            device,
            threads: resolve_threads(options.threads),
            beam_size: options.beam_size,
        })
    }
}

fn open(
    model_path: &Path,
    use_gpu: bool,
) -> Result<(WhisperContext, WhisperState), whisper_rs::WhisperError> {
    let mut params = WhisperContextParameters::default();
    params.use_gpu(use_gpu);
    let context = WhisperContext::new_with_params(model_path, params)?;
    let state = context.create_state()?;
    Ok((context, state))
}

fn backend_error(error: whisper_rs::WhisperError) -> SttError {
    SttError::Backend(error.to_string())
}

impl SttEngine for WhisperEngine {
    fn transcribe(
        &mut self,
        audio: &AudioClip,
        request: &TranscribeRequest,
    ) -> Result<Transcript, SttError> {
        if audio.sample_rate != WHISPER_SAMPLE_RATE {
            return Err(SttError::SampleRateMismatch {
                expected: WHISPER_SAMPLE_RATE,
                actual: audio.sample_rate,
            });
        }
        if request.cancel.is_cancelled() {
            return Err(SttError::Cancelled);
        }
        if audio.is_empty() {
            return Ok(Transcript::default());
        }

        let language = match &request.language {
            Language::Auto => None,
            Language::Code(code) => {
                // whisper-rs는 널 바이트가 든 문자열에서 패닉하므로 먼저 걸러 냅니다.
                if code.contains('\0') || whisper_rs::get_lang_id(code).is_none() {
                    return Err(SttError::UnsupportedLanguage(code.clone()));
                }
                Some(code.as_str())
            }
        };

        let strategy = if self.beam_size <= 1 {
            // best_of는 온도를 올려 다시 시도할 때만 쓰입니다 (whisper.cpp 기본값).
            SamplingStrategy::Greedy { best_of: 5 }
        } else {
            SamplingStrategy::BeamSearch {
                beam_size: self.beam_size,
                patience: -1.0,
            }
        };
        let mut params = FullParams::new(strategy);
        params.set_n_threads(self.threads as i32);
        params.set_no_context(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // `None`이면 whisper.cpp가 언어를 자동 감지합니다.
        params.set_language(language);
        if let Some(prompt) = request.prompt.as_deref() {
            let prompt = prompt.replace('\0', " ");
            if !prompt.trim().is_empty() {
                params.set_initial_prompt(&prompt);
            }
        }
        // whisper-rs 0.16의 `set_abort_callback_safe`는 클로저를 두 번 감싼 포인터를
        // 클로저 자체로 읽어서(이중 포인터 불일치) 엉뚱한 메모리를 보고 곧바로 중단해
        // 버립니다. 그래서 C 콜백을 직접 연결합니다.
        // SAFETY: 콜백은 `full`이 도는 동안에만 불리고, 그동안 `request.cancel`은 살아
        // 있습니다. 토큰은 원자적 값이라 다른 스레드에서 읽어도 안전합니다.
        unsafe {
            params.set_abort_callback(Some(abort_trampoline));
            params.set_abort_callback_user_data(
                std::ptr::from_ref(&request.cancel)
                    .cast_mut()
                    .cast::<c_void>(),
            );
        }

        let samples = pad_short(&audio.samples);
        let result = self.state.full(params, &samples);

        // 취소하면 whisper.cpp는 인코딩·디코딩 실패로 돌아오므로 오류보다 취소를 먼저 봅니다.
        if request.cancel.is_cancelled() {
            return Err(SttError::Cancelled);
        }
        result.map_err(backend_error)?;

        let mut text = String::new();
        let mut segments = Vec::new();
        for segment in self.state.as_iter() {
            let segment_text = segment.to_str_lossy().map_err(backend_error)?;
            text.push_str(&segment_text);
            segments.push(Segment {
                start: timestamp_to_duration(segment.start_timestamp()),
                end: timestamp_to_duration(segment.end_timestamp()),
                text: segment_text.trim().to_owned(),
            });
        }

        let language = match language {
            Some(code) => Some(code.to_owned()),
            None => {
                whisper_rs::get_lang_str(self.state.full_lang_id_from_state()).map(str::to_owned)
            }
        };

        Ok(Transcript {
            text: text.trim().to_owned(),
            language,
            segments,
        })
    }

    fn device(&self) -> ComputeDevice {
        self.device.clone()
    }
}

/// whisper.cpp가 계산 중간중간 부르는 중단 확인 콜백. `true`를 돌려주면 멈춥니다.
///
/// # Safety
/// `user_data`는 살아 있는 [`CancelToken`]을 가리켜야 합니다.
unsafe extern "C" fn abort_trampoline(user_data: *mut c_void) -> bool {
    // SAFETY: 호출하는 쪽(`transcribe`)이 유효한 토큰 포인터를 넘깁니다.
    let cancel = unsafe { &*user_data.cast::<CancelToken>() };
    cancel.is_cancelled()
}

/// 설정의 스레드 수를 실제 값으로 바꿉니다. 0이면 코어 수와 8 중 작은 쪽입니다.
fn resolve_threads(configured: usize) -> usize {
    if configured > 0 {
        return configured;
    }
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, MAX_AUTO_THREADS)
}

/// whisper 타임스탬프(10ms 단위)를 `Duration`으로 바꿉니다.
fn timestamp_to_duration(timestamp: i64) -> Duration {
    Duration::from_millis(timestamp.max(0) as u64 * 10)
}

/// 너무 짧은 오디오는 건너뛰어지지 않게 뒤에 무음을 붙입니다 ([`MIN_SAMPLES`] 참고).
fn pad_short(samples: &[f32]) -> Cow<'_, [f32]> {
    if samples.len() >= MIN_SAMPLES {
        return Cow::Borrowed(samples);
    }
    let mut padded = samples.to_vec();
    padded.resize(MIN_SAMPLES, 0.0);
    Cow::Owned(padded)
}

/// whisper.cpp가 고르는 GPU 장치의 이름 (예: `CUDA (NVIDIA GeForce RTX 4070)`).
///
/// whisper.cpp(`whisper_backend_init_gpu`)는 `gpu_device = 0`일 때 등록된 장치 중
/// 첫 번째 GPU·내장 GPU를 쓰므로 같은 순서로 찾습니다. GPU 장치가 없으면 `None`입니다.
fn gpu_device_name() -> Option<String> {
    // SAFETY: 장치 목록은 ggml이 프로세스 수명 동안 유지하는 전역 레지스트리이고,
    // 돌려받는 문자열 포인터도 그동안 유효한 널 종료 문자열입니다.
    unsafe {
        for index in 0..sys::ggml_backend_dev_count() {
            let device = sys::ggml_backend_dev_get(index);
            if device.is_null() {
                continue;
            }
            let kind = sys::ggml_backend_dev_type(device);
            if kind != sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU
                && kind != sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU
            {
                continue;
            }
            // 장치 이름은 `CUDA0`, `Vulkan0` 같은 백엔드 이름 + 번호입니다.
            let name = c_string(sys::ggml_backend_dev_name(device));
            let description = c_string(sys::ggml_backend_dev_description(device));
            let backend = crate::compiled_gpu_backend()
                .map(str::to_owned)
                .or(name)
                .unwrap_or_else(|| "GPU".to_owned());
            return Some(match description {
                Some(description) => format!("{backend} ({description})"),
                None => backend,
            });
        }
    }
    None
}

/// # Safety
/// `ptr`은 널이거나 유효한 널 종료 문자열을 가리켜야 합니다.
unsafe fn c_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: 호출하는 쪽이 유효한 널 종료 문자열임을 보장합니다.
    let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_ten_milliseconds() {
        assert_eq!(timestamp_to_duration(0), Duration::ZERO);
        assert_eq!(timestamp_to_duration(1), Duration::from_millis(10));
        assert_eq!(timestamp_to_duration(1100), Duration::from_secs(11));
        // whisper.cpp가 드물게 음수를 줄 때는 0으로 봅니다.
        assert_eq!(timestamp_to_duration(-5), Duration::ZERO);
    }

    #[test]
    fn short_audio_is_padded_with_silence() {
        let padded = pad_short(&[0.5; 100]);
        assert_eq!(padded.len(), MIN_SAMPLES);
        assert!(padded[..100].iter().all(|s| *s == 0.5));
        assert!(padded[100..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn long_audio_is_left_alone() {
        let samples = vec![0.1; MIN_SAMPLES + 1];
        let padded = pad_short(&samples);
        assert!(matches!(padded, Cow::Borrowed(_)));
        assert_eq!(padded.len(), MIN_SAMPLES + 1);
    }

    #[test]
    fn auto_threads_are_bounded() {
        let auto = resolve_threads(0);
        assert!((1..=MAX_AUTO_THREADS).contains(&auto));
        assert_eq!(resolve_threads(12), 12);
    }

    #[test]
    fn cpu_build_has_no_gpu_device() {
        if crate::compiled_gpu_backend().is_none() {
            assert_eq!(gpu_device_name(), None);
        }
    }
}
