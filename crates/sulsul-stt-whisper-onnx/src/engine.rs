//! Whisper ONNX 받아쓰기 흐름: 특징 → 창 나누기 → 창마다 인코더 → 디코더 greedy 디코딩 → 토큰 → 텍스트.
//!
//! 세션과 장치 메모리는 [`crate::runner`]가 맡습니다. 특수 토큰 ID와 언어 목록은 인코더 메타데이터
//! (`sot_sequence`, `eot`, `all_language_tokens` 등)에 있습니다.

use std::time::{Duration, Instant};

use sulsul_core::CancelToken;
use sulsul_core::audio::AudioClip;
use sulsul_core::stt::{
    ComputeDevice, Language, Segment, SttEngine, SttError, TranscribeRequest, Transcript,
    WHISPER_SAMPLE_RATE,
};

use crate::accel::DevicePreference;
use crate::chunks;
use crate::decode::{TokenPicker, best_language, repetition_cut};
use crate::features::{HOP_LENGTH, MelExtractor, WINDOW_SAMPLES};
use crate::models::ModelPaths;
use crate::runner::{self, Cross, Runner};
use crate::tokens::TokenTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EngineOptions {
    pub device: DevicePreference,
}

/// 인코더 메타데이터에서 읽은 모델 정보
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Metadata {
    pub n_mels: usize,
    pub n_text_layer: usize,
    pub n_text_ctx: usize,
    pub n_text_state: usize,
    pub n_vocab: usize,
    /// 디코딩 시작 토큰 (다국어: `[sot, 언어, transcribe]`, 영어 전용: `[sot]`)
    pub sot_sequence: Vec<i64>,
    pub sot: i64,
    pub eot: i64,
    pub blank: i64,
    pub no_timestamps: i64,
    pub multilingual: bool,
    /// (언어 코드, 언어 토큰)
    pub languages: Vec<(String, i64)>,
    pub non_speech: Vec<i64>,
}

impl Metadata {
    /// `custom`은 메타데이터 키 → 값입니다.
    pub(crate) fn parse(custom: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let text =
            |key: &str| custom(key).ok_or_else(|| format!("model metadata '{key}' is missing"));
        let int = |key: &str| -> Result<i64, String> {
            text(key)?
                .trim()
                .parse()
                .map_err(|e| format!("model metadata '{key}' is not an integer: {e}"))
        };
        let size = |key: &str| -> Result<usize, String> {
            usize::try_from(int(key)?)
                .ok()
                .filter(|&v| v > 0)
                .ok_or_else(|| format!("model metadata '{key}' must be positive"))
        };
        let ints = |key: &str| -> Result<Vec<i64>, String> {
            text(key)?
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.parse()
                        .map_err(|e| format!("model metadata '{key}' has a bad entry '{s}': {e}"))
                })
                .collect()
        };

        let multilingual = int("is_multilingual")? != 0;
        let sot_sequence = ints("sot_sequence")?;
        let sot = int("sot")?;
        if sot_sequence.first() != Some(&sot) {
            return Err("model metadata 'sot_sequence' does not start with 'sot'".into());
        }
        if multilingual && sot_sequence.len() < 3 {
            return Err("multilingual model needs [sot, language, task] in 'sot_sequence'".into());
        }
        let language_tokens = ints("all_language_tokens")?;
        let language_codes: Vec<String> = text("all_language_codes")?
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        if language_tokens.len() != language_codes.len() {
            return Err("model metadata language codes and tokens differ in length".into());
        }
        // 영어 전용 모델에는 언어 토큰을 넣지 않습니다.
        let languages = if multilingual {
            language_codes.into_iter().zip(language_tokens).collect()
        } else {
            Vec::new()
        };
        let non_speech = match custom("non_speech_tokens") {
            Some(_) => ints("non_speech_tokens")?,
            None => Vec::new(),
        };

        Ok(Self {
            n_mels: size("n_mels")?,
            n_text_layer: size("n_text_layer")?,
            n_text_ctx: size("n_text_ctx")?,
            n_text_state: size("n_text_state")?,
            n_vocab: size("n_vocab")?,
            sot_sequence,
            sot,
            eot: int("eot")?,
            blank: int("blank_id")?,
            no_timestamps: int("no_timestamps")?,
            multilingual,
            languages,
            non_speech,
        })
    }

    pub(crate) fn language_token(&self, code: &str) -> Option<i64> {
        self.languages
            .iter()
            .find(|(c, _)| c == code)
            .map(|&(_, token)| token)
    }

    pub(crate) fn language_code(&self, token: i64) -> Option<&str> {
        self.languages
            .iter()
            .find(|&&(_, t)| t == token)
            .map(|(c, _)| c.as_str())
    }

    /// 한 창의 디코딩 시작 토큰: `[sot, (언어, transcribe)], notimestamps`
    pub(crate) fn prompt(&self, language: Option<i64>) -> Vec<i64> {
        let mut prompt = self.sot_sequence.clone();
        if let (true, Some(language)) = (self.multilingual, language) {
            prompt[1] = language;
        }
        prompt.push(self.no_timestamps);
        prompt
    }

    /// 한 창에서 새로 만들 수 있는 최대 토큰 수 (원본 `sample_len = n_text_ctx / 2`).
    /// 시작 토큰과 합쳐 `n_text_ctx`를 넘지 않습니다.
    pub(crate) fn max_new_tokens(&self, prompt_len: usize) -> usize {
        (self.n_text_ctx / 2).min(self.n_text_ctx.saturating_sub(prompt_len + 1))
    }
}

pub(crate) struct WhisperOnnxEngine {
    runner: Runner,
    metadata: Metadata,
    tokens: TokenTable,
    mel: MelExtractor,
    picker: TokenPicker,
    device: ComputeDevice,
}

impl WhisperOnnxEngine {
    pub(crate) fn load(paths: &ModelPaths, options: EngineOptions) -> Result<Self, SttError> {
        for path in [&paths.encoder, &paths.decoder, &paths.tokens] {
            if !path.is_file() {
                return Err(SttError::ModelNotFound(path.clone()));
            }
        }
        let started = Instant::now();
        let tokens = TokenTable::load(&paths.tokens).map_err(|e| {
            SttError::Backend(format!(
                "failed to read Whisper tokens ({}): {e}",
                paths.tokens.display()
            ))
        })?;
        let (runner, metadata, device) = runner::open(paths, options.device)?;
        let picker = TokenPicker::new(
            metadata.n_vocab,
            metadata.eot,
            metadata.blank,
            &metadata.non_speech,
        );
        tracing::info!(
            encoder = %paths.encoder.display(),
            multilingual = metadata.multilingual,
            n_mels = metadata.n_mels,
            layers = metadata.n_text_layer,
            load_ms = started.elapsed().as_millis(),
            "Whisper ONNX model loaded"
        );

        Ok(Self {
            runner,
            mel: MelExtractor::new(metadata.n_mels),
            metadata,
            tokens,
            picker,
            device,
        })
    }

    /// `[sot]` 한 토큰으로 디코더를 돌려 언어 토큰 가운데 가장 그럴듯한 것을 고릅니다.
    fn detect_language(&mut self, cross: &Cross) -> Result<i64, SttError> {
        let mut state = self.runner.start(cross)?;
        let logits = self.runner.step(&mut state, &[self.metadata.sot])?;
        let tokens: Vec<i64> = self.metadata.languages.iter().map(|&(_, t)| t).collect();
        let index = best_language(&logits, &tokens)
            .ok_or_else(|| SttError::Backend("Whisper ONNX language detection failed".into()))?;
        Ok(tokens[index])
    }

    /// 한 창을 greedy로 디코딩해 텍스트 토큰을 돌려줍니다. 단계마다 취소를 확인합니다.
    fn decode_window(
        &mut self,
        cross: &Cross,
        language: Option<i64>,
        cancel: &CancelToken,
    ) -> Result<Vec<i64>, SttError> {
        let prompt = self.metadata.prompt(language);
        let max_new = self.metadata.max_new_tokens(prompt.len());
        let mut state = self.runner.start(cross)?;
        let mut generated: Vec<i64> = Vec::new();
        let mut input = prompt;
        while generated.len() < max_new && state.offset() + input.len() <= self.metadata.n_text_ctx
        {
            if cancel.is_cancelled() {
                return Err(SttError::Cancelled);
            }
            let logits = self.runner.step(&mut state, &input)?;
            let token = self.picker.pick(&logits, generated.is_empty());
            if token == self.picker.eot() {
                break;
            }
            generated.push(token);
            if let Some(keep) = repetition_cut(&generated) {
                tracing::debug!(
                    dropped = generated.len() - keep,
                    "Whisper ONNX output repeats itself, stopping this window"
                );
                generated.truncate(keep);
                break;
            }
            input = vec![token];
        }
        Ok(generated)
    }
}

impl SttEngine for WhisperOnnxEngine {
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

        // 다국어 모델은 고른 언어의 토큰을, 자동 감지면 첫 창에서 감지한 언어를 모든 창에 씁니다.
        // 영어 전용 모델은 언어 토큰이 없고 항상 영어입니다.
        let mut language: Option<(String, i64)> = None;
        if self.metadata.multilingual {
            if let Language::Code(code) = &request.language {
                let token = self
                    .metadata
                    .language_token(code)
                    .ok_or_else(|| SttError::UnsupportedLanguage(code.clone()))?;
                language = Some((code.clone(), token));
            }
        } else if request.language.code().is_some_and(|code| code != "en") {
            tracing::warn!(
                requested = request.language.as_config(),
                "English-only Whisper model transcribes in English"
            );
        }

        let started = Instant::now();
        let mel = self.mel.log_mel(&clip.samples, WINDOW_SAMPLES);
        let windows = chunks::split(&clip.samples);
        let features_ms = started.elapsed().as_millis();

        let mut text = String::new();
        let mut segments = Vec::new();
        let mut encode_time = Duration::ZERO;
        let mut decode_time = Duration::ZERO;
        let mut generated_tokens = 0;
        for window in &windows {
            if request.cancel.is_cancelled() {
                return Err(SttError::Cancelled);
            }
            let frames = window.start / HOP_LENGTH..window.end.div_ceil(HOP_LENGTH);
            let step = Instant::now();
            let cross = self.runner.encode(mel.window(frames))?;
            encode_time += step.elapsed();
            if request.cancel.is_cancelled() {
                return Err(SttError::Cancelled);
            }

            let step = Instant::now();
            if self.metadata.multilingual && language.is_none() {
                let token = self.detect_language(&cross)?;
                let code = self
                    .metadata
                    .language_code(token)
                    .unwrap_or("en")
                    .to_owned();
                tracing::debug!(language = %code, "Whisper ONNX detected language");
                language = Some((code, token));
            }
            let tokens =
                self.decode_window(&cross, language.as_ref().map(|l| l.1), &request.cancel)?;
            decode_time += step.elapsed();
            generated_tokens += tokens.len();

            let piece = self.tokens.decode(&tokens);
            let trimmed = piece.trim();
            if !trimmed.is_empty() {
                segments.push(Segment {
                    start: samples_to_duration(window.start),
                    end: samples_to_duration(window.end),
                    text: trimmed.to_owned(),
                });
                text.push_str(&piece);
            }
        }
        tracing::debug!(
            windows = windows.len(),
            tokens = generated_tokens,
            features_ms,
            encode_ms = encode_time.as_millis(),
            decode_ms = decode_time.as_millis(),
            "Whisper ONNX transcription timings"
        );

        let language = if self.metadata.multilingual {
            language.map(|(code, _)| code)
        } else {
            Some("en".to_owned())
        };
        Ok(Transcript {
            text: text.trim().to_owned(),
            language,
            segments,
        })
    }
}

fn samples_to_duration(samples: usize) -> Duration {
    Duration::from_secs_f64(samples as f64 / WHISPER_SAMPLE_RATE as f64)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn tiny_metadata(multilingual: bool) -> HashMap<&'static str, &'static str> {
        let mut map = HashMap::from([
            ("n_mels", "80"),
            ("n_text_layer", "4"),
            ("n_text_ctx", "448"),
            ("n_text_state", "384"),
            ("blank_id", "220"),
            ("non_speech_tokens", "1,2,7"),
        ]);
        if multilingual {
            map.extend([
                ("n_vocab", "51865"),
                ("is_multilingual", "1"),
                ("sot", "50258"),
                ("sot_sequence", "50258,50259,50359"),
                ("eot", "50257"),
                ("no_timestamps", "50363"),
                ("all_language_tokens", "50259,50264,50260"),
                ("all_language_codes", "en,ko,zh"),
            ]);
        } else {
            map.extend([
                ("n_vocab", "51864"),
                ("is_multilingual", "0"),
                ("sot", "50257"),
                ("sot_sequence", "50257"),
                ("eot", "50256"),
                ("no_timestamps", "50362"),
                ("all_language_tokens", "50258,50263"),
                ("all_language_codes", "en,ko"),
            ]);
        }
        map
    }

    fn parse(map: &HashMap<&str, &str>) -> Result<Metadata, String> {
        Metadata::parse(|key| map.get(key).map(|v| (*v).to_owned()))
    }

    #[test]
    fn multilingual_prompt_uses_language_token() {
        let meta = parse(&tiny_metadata(true)).unwrap();
        assert!(meta.multilingual);
        assert_eq!(meta.language_token("ko"), Some(50264));
        assert_eq!(meta.language_code(50260), Some("zh"));
        assert_eq!(meta.language_token("fr"), None);
        // [sot, 언어, transcribe, notimestamps]
        assert_eq!(meta.prompt(Some(50264)), [50258, 50264, 50359, 50363]);
        // 언어를 모르면 메타데이터의 기본(영어) 그대로입니다.
        assert_eq!(meta.prompt(None), [50258, 50259, 50359, 50363]);
        assert_eq!(meta.non_speech, [1, 2, 7]);
        assert_eq!(meta.max_new_tokens(4), 224);
    }

    #[test]
    fn english_only_prompt_has_no_language_token() {
        let meta = parse(&tiny_metadata(false)).unwrap();
        assert!(!meta.multilingual);
        assert!(meta.languages.is_empty());
        assert_eq!(meta.language_token("en"), None);
        assert_eq!(meta.prompt(Some(50263)), [50257, 50362]);
        assert_eq!(meta.eot, 50256);
    }

    #[test]
    fn metadata_errors_are_reported() {
        let mut map = tiny_metadata(true);
        map.remove("eot");
        assert!(parse(&map).unwrap_err().contains("'eot'"));

        let mut map = tiny_metadata(true);
        map.insert("all_language_codes", "en,ko");
        assert!(parse(&map).unwrap_err().contains("length"));

        let mut map = tiny_metadata(true);
        map.insert("sot_sequence", "50258");
        assert!(parse(&map).is_err());

        let mut map = tiny_metadata(true);
        map.insert("n_mels", "0");
        assert!(parse(&map).is_err());

        // 기호 토큰 목록은 없어도 됩니다.
        let mut map = tiny_metadata(true);
        map.remove("non_speech_tokens");
        assert!(parse(&map).unwrap().non_speech.is_empty());
    }

    #[test]
    fn token_budget_never_exceeds_context() {
        let meta = parse(&tiny_metadata(true)).unwrap();
        assert_eq!(meta.max_new_tokens(447), 0);
        assert!(meta.max_new_tokens(300) + 300 < meta.n_text_ctx);
    }

    #[test]
    fn missing_files_are_reported_before_loading() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ModelPaths {
            encoder: dir.path().join("tiny-encoder.onnx"),
            decoder: dir.path().join("tiny-decoder.onnx"),
            tokens: dir.path().join("tiny-tokens.txt"),
        };
        std::fs::write(&paths.encoder, b"onnx").unwrap();
        let options = EngineOptions {
            device: DevicePreference::Cpu,
        };
        match WhisperOnnxEngine::load(&paths, options) {
            Err(SttError::ModelNotFound(path)) => assert_eq!(path, paths.decoder),
            Err(other) => panic!("다른 오류: {other}"),
            Ok(_) => panic!("파일 없이 엔진이 만들어졌습니다"),
        }
    }

    #[test]
    fn window_duration() {
        assert_eq!(samples_to_duration(16_000), Duration::from_secs(1));
        assert_eq!(samples_to_duration(8_000), Duration::from_millis(500));
    }
}
