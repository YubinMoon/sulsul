//! SenseVoice ONNX 추론과 CTC 탐욕 디코딩.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ndarray::{Array1, ArrayView3, Ix3};
use ort::environment::Environment;
use ort::inputs;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::TensorRef;

use sulsul_core::audio::AudioClip;
use sulsul_core::stt::{
    ComputeDevice, Language, Segment, SttEngine, SttError, TranscribeRequest, Transcript,
    WHISPER_SAMPLE_RATE,
};

use crate::accel::{self, DetectedDevices, DevicePreference};
use crate::features;

static ORT_ENV: OnceLock<Result<Environment, String>> = OnceLock::new();

fn ort_environment() -> Result<&'static Environment, SttError> {
    match ORT_ENV.get_or_init(|| ort::init().build().map_err(|e| e.to_string())) {
        Ok(env) => Ok(env),
        Err(message) => Err(SttError::Backend(format!(
            "ONNX Runtime 초기화 실패: {message}"
        ))),
    }
}

pub struct EngineOptions {
    pub use_itn: bool,
    pub device: DevicePreference,
}

/// 세션을 만들어 볼 장치. 이름은 감지한 어댑터 이름입니다.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Npu(String),
    Gpu(String),
    Cpu,
}

impl Target {
    fn compute_device(&self) -> ComputeDevice {
        match self {
            Self::Npu(name) => ComputeDevice::Npu(format!("DirectML ({name})")),
            Self::Gpu(name) => ComputeDevice::Gpu(format!("DirectML ({name})")),
            Self::Cpu => ComputeDevice::Cpu,
        }
    }
}

/// 설정과 감지 결과로 시도할 장치 순서를 정합니다. 마지막은 항상 CPU입니다.
///
/// 사용자가 고른 장치를 쓸 수 없으면 그 이유를 `tracing::warn`으로 남깁니다.
/// `auto`는 이 PC에 있는 장치만 NPU → GPU 순서로 넣습니다.
fn plan_targets(
    preference: DevicePreference,
    detected: &DetectedDevices,
    directml_built: bool,
) -> Vec<Target> {
    let mut targets = Vec::new();
    let mut explicit = |kind: &str, found: Option<&str>, make: fn(String) -> Target| {
        if !directml_built {
            tracing::warn!(
                "{kind} 가속을 골랐지만 이 빌드는 DirectML을 지원하지 않아 CPU로 실행합니다"
            );
        } else if let Some(name) = found {
            targets.push(make(name.to_owned()));
        } else {
            tracing::warn!("{kind} 가속을 골랐지만 이 PC에서 {kind}를 찾지 못해 CPU로 실행합니다");
        }
    };
    match preference {
        DevicePreference::Cpu => {}
        DevicePreference::Npu => explicit("NPU", detected.npu(), Target::Npu),
        DevicePreference::Gpu => explicit("GPU", detected.gpu(), Target::Gpu),
        DevicePreference::Auto if directml_built => {
            match detected.npu() {
                Some(name) => targets.push(Target::Npu(name.to_owned())),
                None => tracing::debug!("NPU가 없어 auto에서 건너뜁니다"),
            }
            match detected.gpu() {
                Some(name) => targets.push(Target::Gpu(name.to_owned())),
                None => tracing::debug!("GPU가 없어 auto에서 건너뜁니다"),
            }
        }
        DevicePreference::Auto => {}
    }
    targets.push(Target::Cpu);
    targets
}

/// 장치마다 세션을 만들어 보고, 처음 성공한 세션과 그 장치를 돌려줍니다.
fn create_session(
    model_path: &Path,
    options: &EngineOptions,
) -> Result<(Session, ComputeDevice), SttError> {
    let targets = plan_targets(
        options.device,
        accel::detected_devices(),
        accel::DIRECTML_BUILT,
    );
    for target in targets {
        match build_session(model_path, &target) {
            Ok(session) => {
                let device = target.compute_device();
                tracing::info!(?device, "SenseVoice 세션을 만들었습니다");
                return Ok((session, device));
            }
            Err(error) if target != Target::Cpu => {
                tracing::warn!(
                    ?target,
                    "가속 장치로 SenseVoice 세션을 만들지 못해 다음 장치로 넘어갑니다: {error}"
                );
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("plan_targets는 항상 CPU로 끝납니다")
}

/// CPU 스레드 수는 ONNX Runtime 기본값(물리 코어 수)에 맡깁니다.
fn build_session(model_path: &Path, target: &Target) -> Result<Session, SttError> {
    let env = ort_environment()?;
    // DirectML을 붙일 때만 다시 대입합니다 (Windows 전용).
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut builder = Session::builder(env)
        .map_err(ort_error)?
        .with_optimization_level(optimization_level(target))
        .map_err(ort_error)?;
    match target {
        Target::Cpu => {}
        #[cfg(windows)]
        Target::Npu(_) | Target::Gpu(_) => {
            use ort::ep::directml::{DeviceFilter, DirectML, PerformancePreference};
            let filter = if matches!(target, Target::Npu(_)) {
                DeviceFilter::Npu
            } else {
                DeviceFilter::Gpu
            };
            // DirectML EP는 메모리 패턴 최적화를 지원하지 않습니다 (ONNX Runtime 문서).
            // error_on_failure: 등록에 실패하면 조용히 CPU로 넘어가지 않고 오류를 돌려받아
            // device()가 실제로 쓴 장치를 정확히 보고하게 합니다.
            builder = builder
                .with_memory_pattern(false)
                .map_err(ort_error)?
                .with_execution_providers([DirectML::default()
                    .with_device_filter(filter)
                    .with_performance_preference(PerformancePreference::HighPerformance)
                    .build()
                    .error_on_failure()])
                .map_err(ort_error)?;
        }
        #[cfg(not(windows))]
        Target::Npu(_) | Target::Gpu(_) => {
            return Err(SttError::Backend(
                "이 빌드는 DirectML을 지원하지 않습니다".into(),
            ));
        }
    }
    let session = builder.commit_from_file(model_path).map_err(ort_error)?;
    #[cfg(windows)]
    if *target != Target::Cpu {
        log_directml_module();
    }
    Ok(session)
}

/// 그래프 최적화 수준.
///
/// DirectML 세션에 Level2 이상을 쓰면 int8 모델(동적 양자화)에서 세션을 만드는 도중
/// 프로세스가 비정상 종료(access violation)됩니다. 오류로 돌아오지 않아 CPU로 넘어갈 수도 없으므로
/// DirectML은 Level1만 씁니다 (ONNX Runtime 1.31, RTX 3060에서 확인). Level1과 Level3의 GPU 인식 속도
/// 차이는 측정 오차 수준이었습니다.
fn optimization_level(target: &Target) -> GraphOptimizationLevel {
    match target {
        Target::Cpu => GraphOptimizationLevel::Level3,
        Target::Npu(_) | Target::Gpu(_) => GraphOptimizationLevel::Level1,
    }
}

/// 실제로 불러온 DirectML.dll 경로를 남깁니다. System32의 구버전이 잡혔는지 확인하는 용도입니다.
#[cfg(windows)]
fn log_directml_module() {
    use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
    use windows::core::w;

    // SAFETY: 이미 불러온 모듈의 핸들을 얻고 고정 크기 버퍼에 경로를 받습니다.
    unsafe {
        let Ok(module) = GetModuleHandleW(w!("DirectML.dll")) else {
            tracing::debug!("DirectML.dll이 로드되지 않았습니다");
            return;
        };
        let mut buffer = [0u16; 1024];
        let len = GetModuleFileNameW(Some(module), &mut buffer) as usize;
        let path = String::from_utf16_lossy(&buffer[..len]);
        tracing::debug!(path, "DirectML.dll 경로");
    }
}

struct Metadata {
    blank_id: i64,
    lfr_window_size: usize,
    lfr_window_shift: usize,
    normalize_samples: bool,
    with_itn_id: i32,
    without_itn_id: i32,
    lang2id: HashMap<String, i32>,
    neg_mean: Array1<f32>,
    inv_stddev: Array1<f32>,
}

pub struct SenseVoiceEngine {
    session: Session,
    metadata: Metadata,
    symbols: HashMap<i64, String>,
    input_names: Vec<String>,
    use_itn: bool,
    device: ComputeDevice,
}

impl SenseVoiceEngine {
    pub fn load(
        model_path: &Path,
        tokens_path: &Path,
        options: EngineOptions,
    ) -> Result<Self, SttError> {
        if !model_path.is_file() {
            return Err(SttError::ModelNotFound(model_path.to_path_buf()));
        }
        if !tokens_path.is_file() {
            return Err(SttError::ModelNotFound(tokens_path.to_path_buf()));
        }

        let (session, device) = create_session(model_path, &options)?;
        let metadata = parse_metadata(&session)?;
        let symbols = load_symbols(tokens_path).map_err(|e| {
            SttError::Backend(format!(
                "tokens.txt를 읽지 못했습니다 ({}): {e}",
                tokens_path.display()
            ))
        })?;
        let input_names: Vec<String> = session
            .inputs()
            .iter()
            .map(|input| input.name().to_owned())
            .collect();
        if input_names.len() < 4 {
            return Err(SttError::Backend(format!(
                "SenseVoice 모델 입력 텐서가 예상보다 적습니다 ({})",
                input_names.len()
            )));
        }

        Ok(Self {
            session,
            metadata,
            symbols,
            input_names,
            use_itn: options.use_itn,
            device,
        })
    }
}

impl SttEngine for SenseVoiceEngine {
    fn device(&self) -> ComputeDevice {
        self.device.clone()
    }

    fn transcribe(
        &mut self,
        clip: &AudioClip,
        request: &TranscribeRequest,
    ) -> Result<Transcript, SttError> {
        if clip.sample_rate != WHISPER_SAMPLE_RATE {
            return Err(SttError::SampleRateMismatch {
                expected: WHISPER_SAMPLE_RATE,
                actual: clip.sample_rate,
            });
        }
        if request.cancel.is_cancelled() {
            return Err(SttError::Cancelled);
        }
        if clip.samples.is_empty() {
            return Ok(Transcript::default());
        }

        let started = Instant::now();
        let feats = features::extract_features(
            &clip.samples,
            self.metadata.normalize_samples,
            self.metadata.lfr_window_size,
            self.metadata.lfr_window_shift,
            &self.metadata.neg_mean,
            &self.metadata.inv_stddev,
        );
        if feats.nrows() == 0 {
            return Ok(Transcript::default());
        }
        if request.cancel.is_cancelled() {
            return Err(SttError::Cancelled);
        }

        let lang_code = match &request.language {
            Language::Auto => "auto",
            Language::Code(code) => code.as_str(),
        };
        let lang_id = *self
            .metadata
            .lang2id
            .get(lang_code)
            .ok_or_else(|| SttError::UnsupportedLanguage(lang_code.to_owned()))?;
        let text_norm_id = if self.use_itn {
            self.metadata.with_itn_id
        } else {
            self.metadata.without_itn_id
        };

        let num_frames = feats.nrows() as i32;
        let feat_3d = feats
            .into_shape_with_order((
                1,
                num_frames as usize,
                feats_cols_fix(num_frames, &self.metadata),
            ))
            .map_err(|e| SttError::Backend(format!("입력 특징 텐서 변환 실패: {e}")))?;
        let x_length = ndarray::arr1(&[num_frames]);
        let language_arr = ndarray::arr1(&[lang_id]);
        let norm_arr = ndarray::arr1(&[text_norm_id]);

        let t_feat = TensorRef::from_array_view(feat_3d.view()).map_err(ort_error)?;
        let t_len = TensorRef::from_array_view(x_length.view()).map_err(ort_error)?;
        let t_lang = TensorRef::from_array_view(language_arr.view()).map_err(ort_error)?;
        let t_norm = TensorRef::from_array_view(norm_arr.view()).map_err(ort_error)?;

        let features_done = Instant::now();
        let outputs = self
            .session
            .run(inputs![
                self.input_names[0].as_str() => t_feat,
                self.input_names[1].as_str() => t_len,
                self.input_names[2].as_str() => t_lang,
                self.input_names[3].as_str() => t_norm,
            ])
            .map_err(ort_error)?;
        tracing::debug!(
            features_ms = (features_done - started).as_millis(),
            inference_ms = features_done.elapsed().as_millis(),
            "SenseVoice 인식 단계별 시간"
        );

        if request.cancel.is_cancelled() {
            return Err(SttError::Cancelled);
        }

        let logits = outputs[0].try_extract_array::<f32>().map_err(ort_error)?;
        let logits_3d = logits
            .into_dimensionality::<Ix3>()
            .map_err(|e| SttError::Backend(format!("출력 텐서 차원 오류: {e}")))?;

        let max_len = (num_frames as usize + 4).min(logits_3d.shape()[1]);
        let (tokens, frame_indices) =
            ctc_greedy_decode(&logits_3d, max_len, self.metadata.blank_id);

        Ok(decode_transcript(
            &tokens,
            &frame_indices,
            &self.symbols,
            self.metadata.lfr_window_shift,
        ))
    }
}

fn feats_cols_fix(_num_frames: i32, metadata: &Metadata) -> usize {
    80 * metadata.lfr_window_size
}

fn ctc_greedy_decode(
    logits: &ArrayView3<'_, f32>,
    num_frames: usize,
    blank_id: i64,
) -> (Vec<i64>, Vec<i32>) {
    let vocab_size = logits.shape()[2];
    let mut tokens = Vec::new();
    let mut timestamps = Vec::new();
    let mut prev_id = -1_i64;

    for t in 0..num_frames {
        let mut max_val = f32::NEG_INFINITY;
        let mut max_id = 0_i64;
        for v in 0..vocab_size {
            let val = logits[[0, t, v]];
            if val > max_val {
                max_val = val;
                max_id = v as i64;
            }
        }
        if max_id != blank_id && max_id != prev_id {
            tokens.push(max_id);
            timestamps.push(t as i32);
        }
        prev_id = max_id;
    }

    (tokens, timestamps)
}

fn decode_transcript(
    tokens: &[i64],
    timestamps: &[i32],
    symbols: &HashMap<i64, String>,
    lfr_window_shift: usize,
) -> Transcript {
    // SenseVoice 출력 앞 4개 토큰은 [언어, 감정, 이벤트, ITN] 특수 태그입니다.
    const PROMPT_PREFIX_TOKENS: usize = 4;

    let detected_language = tokens
        .first()
        .and_then(|id| symbols.get(id))
        .and_then(|sym| sym.strip_prefix("<|")?.strip_suffix("|>"))
        .filter(|code| {
            !matches!(
                *code,
                "nospeech" | "Speech" | "BGM" | "Applause" | "Laughter"
            )
        })
        .map(str::to_owned);

    let mut text = String::new();
    let mut segments = Vec::new();
    let frame_shift_s = 0.01 * lfr_window_shift as f32;

    let content_tokens: Vec<(usize, &str)> = tokens
        .iter()
        .enumerate()
        .skip(PROMPT_PREFIX_TOKENS)
        .filter_map(|(idx, id)| symbols.get(id).map(|s| (idx, s.as_str())))
        .collect();

    for (pos, &(tok_idx, sym)) in content_tokens.iter().enumerate() {
        let piece = sym.replace('\u{2581}', " ");
        text.push_str(&piece);

        let start_frame =
            (timestamps.get(tok_idx).copied().unwrap_or(0) - PROMPT_PREFIX_TOKENS as i32).max(0);
        let next_frame = content_tokens
            .get(pos + 1)
            .and_then(|(next_idx, _)| timestamps.get(*next_idx).copied())
            .map(|f| (f - PROMPT_PREFIX_TOKENS as i32).max(start_frame));
        let start = Duration::from_secs_f32(start_frame as f32 * frame_shift_s);
        let end = match next_frame {
            Some(nf) => Duration::from_secs_f32(nf as f32 * frame_shift_s),
            None => start + Duration::from_millis(60),
        };
        if !piece.is_empty() {
            segments.push(Segment {
                start,
                end,
                text: piece,
            });
        }
    }

    let cleaned = text.trim().replace(" '", "'").replace(" \u{2581}'", "'");
    Transcript {
        text: cleaned,
        language: detected_language,
        segments,
    }
}

fn parse_metadata(session: &Session) -> Result<Metadata, SttError> {
    let meta = session.metadata().map_err(ort_error)?;

    let read_i32 = |key: &str, default: i32| -> Result<i32, SttError> {
        match meta.custom(key) {
            Some(v) => v
                .trim()
                .parse::<i32>()
                .map_err(|e| SttError::Backend(format!("모델 메타데이터 '{key}' 파싱 오류: {e}"))),
            None => Ok(default),
        }
    };
    let read_float_vec = |key: &str| -> Result<Vec<f32>, SttError> {
        let Some(raw) = meta.custom(key) else {
            return Ok(Vec::new());
        };
        raw.split([',', ' '])
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.trim().parse::<f32>().map_err(|e| {
                    SttError::Backend(format!("모델 메타데이터 '{key}' 실수 파싱 오류: {e}"))
                })
            })
            .collect()
    };

    let blank_id = i64::from(read_i32("blank_id", 0)?);
    let lfr_window_size = read_i32("lfr_window_size", 7)?.max(1) as usize;
    let lfr_window_shift = read_i32("lfr_window_shift", 6)?.max(1) as usize;
    let normalize_samples = read_i32("normalize_samples", 0)? != 0;
    let with_itn_id = read_i32("with_itn", 14)?;
    let without_itn_id = read_i32("without_itn", 15)?;

    let mut lang2id = HashMap::new();
    for (lang, key, fallback) in [
        ("auto", "lang_auto", 0),
        ("zh", "lang_zh", 3),
        ("en", "lang_en", 4),
        ("yue", "lang_yue", 7),
        ("ja", "lang_ja", 11),
        ("ko", "lang_ko", 12),
    ] {
        let id = if meta.custom(key).is_some() {
            read_i32(key, fallback)?
        } else {
            fallback
        };
        lang2id.insert(lang.to_owned(), id);
    }

    let neg_mean = Array1::from_vec(read_float_vec("neg_mean")?);
    let inv_stddev = Array1::from_vec(read_float_vec("inv_stddev")?);

    Ok(Metadata {
        blank_id,
        lfr_window_size,
        lfr_window_shift,
        normalize_samples,
        with_itn_id,
        without_itn_id,
        lang2id,
        neg_mean,
        inv_stddev,
    })
}

fn load_symbols(path: &Path) -> Result<HashMap<i64, String>, std::io::Error> {
    let contents = fs::read_to_string(path)?;
    let mut map = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.rsplitn(2, char::is_whitespace);
        let (Some(id_str), Some(sym)) = (parts.next(), parts.next()) else {
            continue;
        };
        if let Ok(id) = id_str.parse::<i64>() {
            map.insert(id, sym.to_owned());
        }
    }
    Ok(map)
}

fn ort_error(error: impl std::fmt::Display) -> SttError {
    SttError::Backend(format!("SenseVoice ONNX 오류: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;

    #[test]
    fn ort_env_can_be_obtained_multiple_times() {
        assert!(ort_environment().is_ok());
        assert!(ort_environment().is_ok());
    }

    fn devices(npu: bool, gpu: bool) -> DetectedDevices {
        DetectedDevices {
            npus: npu.then(|| "AI Boost".to_owned()).into_iter().collect(),
            gpus: gpu.then(|| "RTX".to_owned()).into_iter().collect(),
        }
    }

    #[test]
    fn auto_tries_npu_then_gpu_then_cpu() {
        let npu = Target::Npu("AI Boost".into());
        let gpu = Target::Gpu("RTX".into());
        let auto = DevicePreference::Auto;
        assert_eq!(
            plan_targets(auto, &devices(true, true), true),
            vec![npu, gpu.clone(), Target::Cpu]
        );
        // NPU가 없으면 건너뜁니다.
        assert_eq!(
            plan_targets(auto, &devices(false, true), true),
            vec![gpu, Target::Cpu]
        );
        assert_eq!(
            plan_targets(auto, &devices(false, false), true),
            vec![Target::Cpu]
        );
        // DirectML이 없는 빌드는 감지 결과와 상관없이 CPU만 씁니다.
        assert_eq!(
            plan_targets(auto, &devices(true, true), false),
            vec![Target::Cpu]
        );
    }

    #[test]
    fn explicit_choice_falls_back_to_cpu_only() {
        let both = devices(true, true);
        assert_eq!(
            plan_targets(DevicePreference::Gpu, &both, true),
            vec![Target::Gpu("RTX".into()), Target::Cpu]
        );
        assert_eq!(
            plan_targets(DevicePreference::Npu, &both, true),
            vec![Target::Npu("AI Boost".into()), Target::Cpu]
        );
        // 고른 장치가 없으면 다른 가속 장치가 아니라 CPU로 넘어갑니다.
        assert_eq!(
            plan_targets(DevicePreference::Npu, &devices(false, true), true),
            vec![Target::Cpu]
        );
        assert_eq!(
            plan_targets(DevicePreference::Gpu, &both, false),
            vec![Target::Cpu]
        );
        assert_eq!(
            plan_targets(DevicePreference::Cpu, &both, true),
            vec![Target::Cpu]
        );
    }

    #[test]
    fn target_reports_device_kind_and_name() {
        assert_eq!(
            Target::Npu("Intel(R) AI Boost".into()).compute_device(),
            ComputeDevice::Npu("DirectML (Intel(R) AI Boost)".into())
        );
        assert_eq!(
            Target::Gpu("RTX".into()).compute_device(),
            ComputeDevice::Gpu("DirectML (RTX)".into())
        );
        assert_eq!(Target::Cpu.compute_device(), ComputeDevice::Cpu);
    }

    #[test]
    fn ctc_decode_collapses_blanks_and_repeats() {
        // frames=6, vocab=5 (blank=0, <|ko|>=1, <|=|>=2, <|Speech|>=3, <|wo|>=4, wait vocab=6)
        let mut logits = Array3::<f32>::zeros((1, 8, 6));
        let path = [1, 1, 0, 2, 3, 2, 4, 5]; // -> tokens [1, 2, 3, 2, 4, 5]
        for (t, &tok) in path.iter().enumerate() {
            logits[[0, t, tok]] = 10.0;
        }
        let (tokens, _) = ctc_greedy_decode(&logits.view(), 8, 0);
        assert_eq!(tokens, vec![1, 2, 3, 2, 4, 5]);
    }

    #[test]
    fn decode_transcript_extracts_language_and_strips_prefix_tokens() {
        let symbols = HashMap::from([
            (1, "<|ko|>".into()),
            (2, "<|NEUTRAL|>".into()),
            (3, "<|Speech|>".into()),
            (4, "<|withitn|>".into()),
            (5, "\u{2581}안녕".into()),
            (6, "\u{2581}하세요".into()),
        ]);
        let transcript = decode_transcript(&[1, 2, 3, 4, 5, 6], &[0, 1, 2, 3, 6, 10], &symbols, 6);
        assert_eq!(transcript.language.as_deref(), Some("ko"));
        assert_eq!(transcript.text, "안녕 하세요");
        assert_eq!(transcript.segments.len(), 2);
    }
}
