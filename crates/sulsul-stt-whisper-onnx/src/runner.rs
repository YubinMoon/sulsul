//! ONNX Runtime 세션: 인코더 실행과 디코더 한 단계 실행.
//!
//! 모델은 sherpa-onnx가 내보낸 형식입니다.
//! - 인코더: `mel [1, n_mels, 3000]` → `n_layer_cross_k`, `n_layer_cross_v` (`[n_text_layer, 1, 1500, n_text_state]`)
//! - 디코더: `tokens [1, n]`, `in_n_layer_self_{k,v}_cache [n_text_layer, 1, 448, n_text_state]`,
//!   `n_layer_cross_{k,v}`, `offset [1]` → `logits [1, n, n_vocab]`, `out_n_layer_self_{k,v}_cache`
//!
//! # 장치 나누기
//! GPU를 쓸 때도 DirectML에는 인코더만 올리고 디코더는 CPU에서 돌립니다. 디코더는 토큰 하나마다 작은 연산 수백 개를
//! 돌리는데, DirectML에서는 그래프 중간의 모양 계산(위치 `offset`으로 캐시를 자르는 부분)이 CPU에 배치되어 단계마다
//! GPU를 기다려야 해서 CPU보다 4배쯤 느렸습니다(RTX 3060, 8초 한국어 기준 tiny 단계당 약 45 ms ↔ CPU 11 ms).
//! 인코더는 창마다 한 번뿐이고 계산량이 커서 GPU가 훨씬 빠릅니다(base 313 ms → 93 ms). 둘을 나눈 쪽이 tiny 1.45 s,
//! base-int8 2.09 s(모두 DirectML)에서 0.54 s, 0.77 s로 가장 빨랐습니다.
//!
//! 디코더 입출력은 IoBinding으로 묶어, 자기 어텐션 캐시 두 벌을 단계마다 번갈아 입력·출력으로 씁니다
//! (단계마다 캐시를 새로 할당하지 않음). cross K·V는 창마다 한 번 CPU로 받아 모든 단계가 함께 씁니다.

use std::path::Path;
use std::time::Instant;

use ort::memory::{AllocationDevice, Allocator, AllocatorType, MemoryInfo, MemoryType};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{IoBinding, Session, SessionOutputs};
use ort::value::{DynValue, Tensor};

use sulsul_core::stt::{ComputeDevice, SttError};

use crate::accel::{self, DevicePreference, Target};
use crate::engine::Metadata;
use crate::features::WINDOW_FRAMES;
use crate::models::ModelPaths;

/// 인코더 입출력 이름
const ENCODER_INPUT: &str = "mel";
const CROSS_K: &str = "n_layer_cross_k";
const CROSS_V: &str = "n_layer_cross_v";
/// 디코더 입출력 이름
const TOKENS: &str = "tokens";
const SELF_K_IN: &str = "in_n_layer_self_k_cache";
const SELF_V_IN: &str = "in_n_layer_self_v_cache";
const OFFSET: &str = "offset";
const LOGITS: &str = "logits";
const SELF_K_OUT: &str = "out_n_layer_self_k_cache";
const SELF_V_OUT: &str = "out_n_layer_self_v_cache";

/// 한 창의 cross-attention K·V (CPU 메모리)
pub(crate) struct Cross {
    k: DynValue,
    v: DynValue,
}

/// 한 창을 디코딩하는 동안의 상태
pub(crate) struct DecodeState {
    binding: IoBinding,
    /// 지금까지 돌린 단계 수. 짝수 단계는 캐시 0번에, 홀수 단계는 1번에 씁니다.
    step: usize,
    /// 디코더가 지금까지 본 토큰 수 (다음 입력의 위치)
    offset: usize,
}

impl DecodeState {
    pub(crate) fn offset(&self) -> usize {
        self.offset
    }
}

/// 자기 어텐션 캐시 (K, V)
type CachePair = (Tensor<f32>, Tensor<f32>);

pub(crate) struct Runner {
    encoder: Session,
    decoder: Session,
    cpu_memory: MemoryInfo<'static>,
    /// 첫 단계 입력: 0으로 채운 캐시
    zeros: CachePair,
    /// 단계마다 번갈아 쓰는 캐시 두 벌
    buffers: [CachePair; 2],
    n_mels: usize,
}

/// 설정한 장치부터 차례로 세션을 만들어 보고, 처음 성공한 것과 인코더가 쓰는 장치를 돌려줍니다.
///
/// GPU는 세션을 만든 뒤 한 번 미리 돌려 봅니다. 실행 중에만 드러나는 실패(지원하지 않는 연산 등)도 여기서 CPU로
/// 넘어가게 하고, DirectML이 처음 실행할 때 하는 준비(셰이더 컴파일)를 첫 받아쓰기에서 빼기 위해서입니다.
pub(crate) fn open(
    paths: &ModelPaths,
    device: DevicePreference,
) -> Result<(Runner, Metadata, ComputeDevice), SttError> {
    let targets = accel::plan_targets(
        device,
        sulsul_stt_sensevoice::accel::detected_devices(),
        sulsul_stt_sensevoice::accel::DIRECTML_BUILT,
    );
    for target in targets {
        let started = Instant::now();
        let opened = Runner::new(paths, &target).and_then(|(mut runner, metadata)| {
            if target != Target::Cpu {
                runner.warm_up(&metadata)?;
            }
            Ok((runner, metadata))
        });
        match opened {
            Ok((runner, metadata)) => {
                let device = target.compute_device();
                tracing::info!(
                    ?device,
                    open_ms = started.elapsed().as_millis(),
                    "created Whisper ONNX sessions (decoder on CPU)"
                );
                return Ok((runner, metadata, device));
            }
            Err(error) if target != Target::Cpu => {
                tracing::warn!(
                    ?target,
                    "failed to run Whisper ONNX on the accelerator, trying the next device: {error}"
                );
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("plan_targets always ends with the CPU")
}

impl Runner {
    /// 인코더는 `target`에, 디코더는 CPU에 올립니다 (모듈 설명 참고).
    fn new(paths: &ModelPaths, target: &Target) -> Result<(Self, Metadata), SttError> {
        let encoder = build_session(&paths.encoder, target)?;
        let decoder = build_session(&paths.decoder, &Target::Cpu)?;
        check_names(&encoder, &[ENCODER_INPUT], &[CROSS_K, CROSS_V], "encoder")?;
        check_names(
            &decoder,
            &[TOKENS, SELF_K_IN, SELF_V_IN, CROSS_K, CROSS_V, OFFSET],
            &[LOGITS, SELF_K_OUT, SELF_V_OUT],
            "decoder",
        )?;
        let metadata = {
            let meta = encoder.metadata().map_err(ort_error)?;
            Metadata::parse(|key| meta.custom(key))
                .map_err(|e| SttError::Backend(format!("Whisper ONNX encoder: {e}")))?
        };

        let cpu_memory = MemoryInfo::new(
            AllocationDevice::CPU,
            0,
            AllocatorType::Device,
            MemoryType::Default,
        )
        .map_err(ort_error)?;
        let allocator = Allocator::default();
        let shape = [
            metadata.n_text_layer,
            1,
            metadata.n_text_ctx,
            metadata.n_text_state,
        ];
        // 캐시 버퍼는 디코더가 늘 전체를 써 넣은 뒤에만 읽으므로 초기화하지 않아도 됩니다.
        let buffer = || Tensor::<f32>::new(&allocator, shape).map_err(ort_error);
        let zeros = || {
            Tensor::from_array((shape, vec![0.0_f32; shape.iter().product()])).map_err(ort_error)
        };
        let runner = Self {
            encoder,
            decoder,
            cpu_memory,
            zeros: (zeros()?, zeros()?),
            buffers: [(buffer()?, buffer()?), (buffer()?, buffer()?)],
            n_mels: metadata.n_mels,
        };
        Ok((runner, metadata))
    }

    /// 무음 창 하나를 인코딩하고 디코더를 한 단계 돌려 봅니다.
    fn warm_up(&mut self, metadata: &Metadata) -> Result<(), SttError> {
        let started = Instant::now();
        let cross = self.encode(vec![0.0; self.n_mels * WINDOW_FRAMES])?;
        let mut state = self.start(&cross)?;
        self.step(&mut state, &metadata.prompt(None))?;
        tracing::debug!(
            warm_up_ms = started.elapsed().as_millis(),
            "Whisper ONNX warm-up"
        );
        Ok(())
    }

    /// 인코더를 한 번 돌려 cross-attention K·V를 얻습니다. `mel`은 `[n_mels][3000]` 순서입니다.
    pub(crate) fn encode(&mut self, mel: Vec<f32>) -> Result<Cross, SttError> {
        let input =
            Tensor::from_array(([1, self.n_mels, WINDOW_FRAMES], mel)).map_err(ort_error)?;
        let mut binding = self.encoder.create_binding().map_err(ort_error)?;
        binding
            .bind_input(ENCODER_INPUT, &input)
            .map_err(ort_error)?;
        for name in [CROSS_K, CROSS_V] {
            binding
                .bind_output_to_device(name, &self.cpu_memory)
                .map_err(ort_error)?;
        }
        let mut outputs = run_synchronized(&mut self.encoder, &binding)?;
        match (outputs.remove(CROSS_K), outputs.remove(CROSS_V)) {
            (Some(k), Some(v)) => Ok(Cross { k, v }),
            _ => Err(SttError::Backend(
                "Whisper ONNX encoder returned no cross K/V".into(),
            )),
        }
    }

    /// `cross`로 한 창의 디코딩을 시작합니다. 캐시는 0에서 시작합니다.
    pub(crate) fn start(&mut self, cross: &Cross) -> Result<DecodeState, SttError> {
        let mut binding = self.decoder.create_binding().map_err(ort_error)?;
        binding.bind_input(CROSS_K, &cross.k).map_err(ort_error)?;
        binding.bind_input(CROSS_V, &cross.v).map_err(ort_error)?;
        Ok(DecodeState {
            binding,
            step: 0,
            offset: 0,
        })
    }

    /// 디코더 한 단계. `tokens`를 지금 위치에 넣고 마지막 토큰의 logits를 돌려줍니다.
    pub(crate) fn step(
        &mut self,
        state: &mut DecodeState,
        tokens: &[i64],
    ) -> Result<Vec<f32>, SttError> {
        let binding = &mut state.binding;
        // logits는 단계마다 모양이 달라지므로(시작 토큰 여러 개 → 1개) 매번 새로 묶어 새 버퍼를 받습니다.
        // 한 번만 묶으면 ONNX Runtime이 첫 실행의 버퍼를 다시 쓰려다 모양이 안 맞아 실패합니다.
        binding
            .bind_output_to_device(LOGITS, &self.cpu_memory)
            .map_err(ort_error)?;
        let tokens_input =
            Tensor::from_array(([1, tokens.len()], tokens.to_vec())).map_err(ort_error)?;
        let offset_input =
            Tensor::from_array(([1], vec![state.offset as i64])).map_err(ort_error)?;
        binding
            .bind_input(TOKENS, &tokens_input)
            .map_err(ort_error)?;
        binding
            .bind_input(OFFSET, &offset_input)
            .map_err(ort_error)?;

        let output = state.step % 2;
        let input = if state.step == 0 {
            &self.zeros
        } else {
            &self.buffers[1 - output]
        };
        binding.bind_input(SELF_K_IN, &input.0).map_err(ort_error)?;
        binding.bind_input(SELF_V_IN, &input.1).map_err(ort_error)?;
        let output = &mut self.buffers[output];
        binding
            .bind_output(SELF_K_OUT, &mut output.0)
            .map_err(ort_error)?;
        binding
            .bind_output(SELF_V_OUT, &mut output.1)
            .map_err(ort_error)?;

        let outputs = run_synchronized(&mut self.decoder, binding)?;
        let (shape, logits) = outputs[LOGITS]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        let vocab = shape.last().copied().unwrap_or(0) as usize;
        if vocab == 0 || logits.len() < vocab {
            return Err(SttError::Backend(
                "Whisper ONNX decoder returned empty logits".into(),
            ));
        }
        let last = logits[logits.len() - vocab..].to_vec();
        state.step += 1;
        state.offset += tokens.len();
        Ok(last)
    }
}

/// 바인딩한 입출력을 동기화해 실행합니다.
///
/// ONNX Runtime의 `RunWithBinding`은 스스로 동기화하지 않습니다. DirectML은 장치로 옮기는 복사와 계산을
/// 비동기로 줄 세우므로, 입력 복사가 끝나기 전에 계산하거나 계산이 끝나기 전에 결과를 읽지 않도록 실행 앞뒤로
/// 기다립니다(빠뜨리면 인코더 결과가 덜 채워진 채로 디코더에 들어가 엉뚱한 글자가 나옵니다). CPU에서는 아무 일도
/// 하지 않습니다.
fn run_synchronized<'b>(
    session: &'b mut Session,
    binding: &'b IoBinding,
) -> Result<SessionOutputs<'b>, SttError> {
    binding.synchronize_inputs().map_err(ort_error)?;
    let outputs = session.run_binding(binding).map_err(ort_error)?;
    binding.synchronize_outputs().map_err(ort_error)?;
    Ok(outputs)
}

/// CPU 스레드 수는 ONNX Runtime 기본값(물리 코어 수)에 맡깁니다.
fn build_session(model_path: &Path, target: &Target) -> Result<Session, SttError> {
    let env = sulsul_stt_sensevoice::ort_environment()?;
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut builder = Session::builder(env)
        .map_err(ort_error)?
        .with_optimization_level(optimization_level(target))
        .map_err(ort_error)?;
    match target {
        Target::Cpu => {}
        #[cfg(windows)]
        Target::Gpu(_) => {
            use ort::ep::directml::{DeviceFilter, DirectML, PerformancePreference};
            // DirectML EP는 메모리 패턴 최적화를 지원하지 않습니다 (ONNX Runtime 문서).
            // error_on_failure: 등록에 실패하면 조용히 CPU로 넘어가지 않고 오류를 돌려받아
            // device()가 실제로 쓴 장치를 정확히 보고하게 합니다.
            builder = builder
                .with_memory_pattern(false)
                .map_err(ort_error)?
                .with_execution_providers([DirectML::default()
                    .with_device_filter(DeviceFilter::Gpu)
                    .with_performance_preference(PerformancePreference::HighPerformance)
                    .build()
                    .error_on_failure()])
                .map_err(ort_error)?;
        }
        #[cfg(not(windows))]
        Target::Gpu(_) => {
            return Err(SttError::Backend(
                "this build has no DirectML support".into(),
            ));
        }
    }
    builder.commit_from_file(model_path).map_err(ort_error)
}

/// 그래프 최적화 수준.
///
/// DirectML은 Level1만 씁니다. Level2 이상에서는 int8(동적 양자화) 모델 세션을 만들다 프로세스가 비정상 종료
/// (0xc0000005)됩니다(SenseVoice와 같은 문제, ONNX Runtime 1.31·RTX 3060에서 tiny-int8·base-int8로 확인).
/// fp32 모델도 Level3이 Level1보다 10%쯤 빠를 뿐이었습니다.
fn optimization_level(target: &Target) -> GraphOptimizationLevel {
    match target {
        Target::Cpu => GraphOptimizationLevel::Level3,
        Target::Gpu(_) => GraphOptimizationLevel::Level1,
    }
}

fn check_names(
    session: &Session,
    inputs: &[&str],
    outputs: &[&str],
    what: &str,
) -> Result<(), SttError> {
    let has_input = |name: &str| session.inputs().iter().any(|i| i.name() == name);
    let has_output = |name: &str| session.outputs().iter().any(|o| o.name() == name);
    let missing: Vec<&str> = inputs
        .iter()
        .copied()
        .filter(|n| !has_input(n))
        .chain(outputs.iter().copied().filter(|n| !has_output(n)))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(SttError::Backend(format!(
            "Whisper ONNX {what} is not a sherpa-onnx export (missing {})",
            missing.join(", ")
        )))
    }
}

pub(crate) fn ort_error(error: impl std::fmt::Display) -> SttError {
    SttError::Backend(format!("Whisper ONNX error: {error}"))
}
