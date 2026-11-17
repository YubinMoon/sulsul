//! llama.cpp로 GGUF 모델을 올리고 글을 만드는 엔진.
//!
//! 모델은 하나만 올려 둡니다. 요청마다 새 컨텍스트를 만들어, 앞 요청의 상태(KV 캐시, Qwen3.5 같은
//! 혼합 구조 모델의 순환 상태)가 남지 않게 합니다. 받아쓰기를 고치는 일이라 무작위성 없이 가장 그럴듯한
//! 토큰만 고릅니다(greedy).

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Instant;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::{LlamaBackendDeviceType, list_llama_ggml_backend_devices};
use sulsul_llm::runner::Failure;

use crate::{Engine, Generation};

/// 한 번에 모델에 넣는 프롬프트 토큰 수
const BATCH: usize = 512;

/// 만들 글의 최대 토큰 수 범위. 프롬프트 토큰 수의 두 배를 이 범위로 자릅니다
/// (다듬은 글은 받아쓴 글과 길이가 비슷하고, 지시문까지 센 값이라 넉넉함).
const MIN_NEW_TOKENS: usize = 256;
const MAX_NEW_TOKENS: usize = 4096;

/// GPU에 모든 층을 올리라는 값 (모델 층 수보다 크면 됨)
const ALL_LAYERS: u32 = 999;

pub struct LlamaEngine {
    backend: LlamaBackend,
    loaded: Option<Loaded>,
}

struct Loaded {
    path: PathBuf,
    use_gpu: bool,
    model: LlamaModel,
    device: String,
}

impl LlamaEngine {
    pub fn new() -> llama_cpp_2::Result<Self> {
        Ok(Self {
            backend: LlamaBackend::init()?,
            loaded: None,
        })
    }

    fn load_model(&self, path: &Path, gpu: bool) -> Result<LlamaModel, Failure> {
        // 메모리 매핑을 끄면 모델 파일을 열어 둔 채로 두지 않습니다. Windows에서 열려 있는 파일은 지울 수 없어,
        // 모델 관리 화면에서 쓰던 모델을 지우지 못하게 됩니다.
        let params = LlamaModelParams::default()
            .with_n_gpu_layers(if gpu { ALL_LAYERS } else { 0 })
            .with_use_mmap(false);
        LlamaModel::load_from_file(&self.backend, path, &params)
            .map_err(|error| Failure::Load(error.to_string()))
    }
}

impl Engine for LlamaEngine {
    fn load(&mut self, path: &Path, use_gpu: bool) -> Result<String, Failure> {
        if let Some(loaded) = &self.loaded
            && loaded.path == path
            && loaded.use_gpu == use_gpu
        {
            return Ok(loaded.device.clone());
        }
        // 새 모델을 올리기 전에 앞 모델의 메모리를 먼저 돌려줍니다.
        self.loaded = None;
        if !path.is_file() {
            return Err(Failure::ModelMissing(path.display().to_string()));
        }

        let started = Instant::now();
        let gpu = use_gpu && self.backend.supports_gpu_offload() && gpu_device().is_some();
        let (model, on_gpu) = if gpu {
            match self.load_model(path, true) {
                Ok(model) => (model, true),
                Err(error) => {
                    tracing::warn!("could not load the model on the GPU, using the CPU: {error}");
                    (self.load_model(path, false)?, false)
                }
            }
        } else {
            (self.load_model(path, false)?, false)
        };
        let device = if on_gpu {
            gpu_device().unwrap_or_else(|| "GPU".into())
        } else {
            "CPU".into()
        };
        tracing::info!(
            model = %path.display(),
            %device,
            seconds = started.elapsed().as_secs_f32(),
            "model loaded"
        );
        self.loaded = Some(Loaded {
            path: path.to_path_buf(),
            use_gpu,
            model,
            device: device.clone(),
        });
        Ok(device)
    }

    fn generate(&mut self, prompt: &str) -> Result<Generation, Failure> {
        let Some(loaded) = &self.loaded else {
            return Err(Failure::BadRequest("no model is loaded".into()));
        };
        let model = &loaded.model;
        let vocab = model.vocab();
        let tokens = vocab.tokenize(prompt.as_bytes(), true, true);
        if tokens.is_empty() {
            return Err(Failure::BadRequest("the prompt is empty".into()));
        }

        let train_ctx = model.n_ctx_train() as usize;
        let max_new = (tokens.len() * 2).clamp(MIN_NEW_TOKENS, MAX_NEW_TOKENS);
        let n_ctx = (tokens.len() + max_new).min(train_ctx);
        if tokens.len() + 16 > n_ctx {
            return Err(Failure::PromptTooLong(tokens.len() as u32));
        }
        let max_new = n_ctx - tokens.len();

        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get().div_ceil(2))
            .clamp(1, 16) as i32;
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx as u32))
            .with_n_batch(BATCH as u32)
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
        let mut ctx = model
            .new_context(&self.backend, params)
            .map_err(|error| Failure::Generate(format!("could not create a context: {error}")))?;

        let started = Instant::now();
        let mut batch = LlamaBatch::new(BATCH, 1);
        let last = tokens.len() - 1;
        for (chunk_index, chunk) in tokens.chunks(BATCH).enumerate() {
            batch.clear();
            for (i, token) in chunk.iter().enumerate() {
                let pos = chunk_index * BATCH + i;
                batch
                    .add(*token, pos as i32, &[0], pos == last)
                    .map_err(|error| Failure::Generate(error.to_string()))?;
            }
            ctx.decode(&mut batch)
                .map_err(|error| Failure::Generate(format!("prompt decode failed: {error}")))?;
        }
        let prompt_seconds = started.elapsed().as_secs_f32();

        let mut sampler = LlamaSampler::greedy();
        let mut logits_index = batch.n_tokens() - 1;
        let mut pos = tokens.len() as i32;
        let mut bytes = Vec::new();
        let mut output_tokens = 0_usize;
        let mut truncated = true;
        let generate_started = Instant::now();
        while output_tokens < max_new {
            let token = sampler.sample(&ctx, logits_index);
            if vocab.is_eog(token) {
                truncated = false;
                break;
            }
            bytes.extend(vocab.token_to_piece(token, true, None));
            output_tokens += 1;

            batch.clear();
            batch
                .add(token, pos, &[0], true)
                .map_err(|error| Failure::Generate(error.to_string()))?;
            ctx.decode(&mut batch)
                .map_err(|error| Failure::Generate(format!("decode failed: {error}")))?;
            pos += 1;
            logits_index = 0;
        }
        let generate_seconds = generate_started.elapsed().as_secs_f32();
        tracing::info!(
            prompt_tokens = tokens.len(),
            output_tokens,
            prompt_seconds,
            generate_seconds,
            tokens_per_second = output_tokens as f32 / generate_seconds.max(f32::EPSILON),
            truncated,
            "text generated"
        );
        Ok(Generation {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            prompt_tokens: tokens.len() as u32,
            output_tokens: output_tokens as u32,
            truncated,
        })
    }
}

/// 모델을 올릴 GPU 이름 (예: `"Vulkan0 (NVIDIA GeForce RTX 4070)"`). 쓸 수 있는 GPU가 없으면 `None`
fn gpu_device() -> Option<String> {
    list_llama_ggml_backend_devices()
        .into_iter()
        .find(|device| {
            matches!(
                device.device_type,
                LlamaBackendDeviceType::Gpu | LlamaBackendDeviceType::IntegratedGpu
            )
        })
        .map(|device| format!("{} ({})", device.name, device.description))
}
