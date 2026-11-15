//! 개발·UI 테스트용 가짜 엔진. 실제 인식 없이 오디오 정보를 텍스트로 돌려줍니다.

use std::time::{Duration, Instant};

use super::{
    EngineCapabilities, EngineContext, EngineDescriptor, EngineKind, LanguageSupport, SttEngine,
    SttEngineFactory, SttError, TranscribeRequest, Transcript, WHISPER_SAMPLE_RATE,
    keep_loaded_field,
};
use crate::audio::AudioClip;
use crate::settings::{ResolvedSettings, SettingField};

pub const MOCK_ENGINE_ID: &str = "mock";

pub struct MockEngineFactory {
    descriptor: EngineDescriptor,
}

impl MockEngineFactory {
    pub fn new() -> Self {
        Self {
            descriptor: EngineDescriptor {
                id: MOCK_ENGINE_ID.into(),
                name_key: "engine.mock.name".into(),
                description_key: "engine.mock.description".into(),
                kind: EngineKind::Local,
                capabilities: EngineCapabilities {
                    input_sample_rate: WHISPER_SAMPLE_RATE,
                    languages: LanguageSupport::Any,
                    supports_prompt: false,
                    streaming: false,
                },
                settings: vec![
                    SettingField::integer(
                        "delay_ms",
                        "engine.mock.settings.delay_ms",
                        300,
                        0,
                        10_000,
                    )
                    .with_help("engine.mock.settings.delay_ms_help"),
                    SettingField::text("text", "engine.mock.settings.text", "")
                        .with_help("engine.mock.settings.text_help"),
                    keep_loaded_field(),
                ],
                retired_keys: Vec::new(),
            },
        }
    }
}

impl Default for MockEngineFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl SttEngineFactory for MockEngineFactory {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    fn create(
        &self,
        _ctx: &EngineContext,
        settings: &ResolvedSettings,
    ) -> Result<Box<dyn SttEngine>, SttError> {
        Ok(Box::new(MockEngine {
            delay: Duration::from_millis(settings.int("delay_ms").max(0) as u64),
            text: settings.str("text").to_owned(),
        }))
    }
}

struct MockEngine {
    delay: Duration,
    text: String,
}

impl SttEngine for MockEngine {
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
        // 취소 흐름도 확인할 수 있게 조금씩 끊어서 기다립니다.
        let deadline = Instant::now() + self.delay;
        loop {
            if request.cancel.is_cancelled() {
                return Err(SttError::Cancelled);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(Duration::from_millis(10)));
        }
        let text = if self.text.is_empty() {
            format!(
                "[mock] {:.2}s of audio, peak {:.2}, language {}",
                audio.duration().as_secs_f64(),
                audio.peak(),
                request.language.code().unwrap_or("auto"),
            )
        } else {
            self.text.clone()
        };
        Ok(Transcript {
            text,
            language: request.language.code().map(str::to_owned),
            segments: Vec::new(),
        })
    }
}
