//! 인식 실행 흐름: 엔진 준비(재사용) → 언어 확인 → 리샘플 → 인식.
//!
//! 화면, 명령줄 도구, 단축키 흐름이 같은 [`Transcriber`]를 씁니다. 모델 로딩처럼 비싼
//! 준비를 반복하지 않도록 엔진 인스턴스를 보관하고, 엔진 ID와 설정이 그대로면 다시 씁니다.

use std::time::{Duration, Instant};

use super::{
    ComputeDevice, EngineContext, Language, SttEngine, SttError, SttRegistry, TranscribeRequest,
    Transcript,
};
use crate::audio::{AudioClip, AudioError};
use crate::cancel::CancelToken;
use crate::config::SttConfig;
use crate::settings::ResolvedSettings;

/// 엔진을 보관하며 인식을 실행합니다.
///
/// 준비와 인식은 오래 걸릴 수 있으므로 UI 스레드가 아닌 작업 스레드에서 호출하세요.
/// 작업 스레드로 통째로 넘겼다가 돌려받아 다음 작업에 다시 쓰면 됩니다.
pub struct Transcriber {
    registry: SttRegistry,
    context: EngineContext,
    loaded: Option<Loaded>,
}

/// 준비해 둔 엔진
struct Loaded {
    id: String,
    /// 엔진을 만들 때 쓴 설정. 다음 작업의 설정과 같아야 다시 씁니다.
    settings: ResolvedSettings,
    engine: Box<dyn SttEngine>,
}

/// 인식 한 번에 필요한 값
#[derive(Debug, Clone)]
pub struct TranscribeJob<'a> {
    /// 설정 파일의 `[stt]` (엔진별 설정 포함)
    pub stt: &'a SttConfig,
    pub engine_id: &'a str,
    /// `ko`, `en`, `auto` 등
    pub language: &'a str,
    pub cancel: CancelToken,
}

/// 인식 결과와 걸린 시간
#[derive(Debug, Clone, PartialEq)]
pub struct Transcription {
    pub transcript: Transcript,
    /// 엔진이 계산에 쓴 장치
    pub device: ComputeDevice,
    /// 엔진을 만드는 데 걸린 시간 (다시 쓴 경우 0)
    pub prepare: Duration,
    /// 이전 엔진 인스턴스를 다시 썼는지
    pub reused: bool,
    /// 인식(`transcribe`)에 걸린 시간
    pub recognize: Duration,
}

/// [`Transcriber::prepare`]의 결과
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub device: ComputeDevice,
    /// 엔진을 만드는 데 걸린 시간 (다시 쓴 경우 0)
    pub elapsed: Duration,
    pub reused: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TranscribeError {
    #[error(transparent)]
    Stt(#[from] SttError),
    #[error(transparent)]
    Audio(#[from] AudioError),
}

impl Transcriber {
    pub fn new(registry: SttRegistry, context: EngineContext) -> Self {
        Self {
            registry,
            context,
            loaded: None,
        }
    }

    pub fn registry(&self) -> &SttRegistry {
        &self.registry
    }

    pub fn context(&self) -> &EngineContext {
        &self.context
    }

    /// 엔진만 미리 준비합니다(모델 로딩). 이미 같은 엔진·설정이면 그대로 둡니다.
    pub fn prepare(&mut self, stt: &SttConfig, engine_id: &str) -> Result<Prepared, SttError> {
        let (loaded, elapsed, reused) = ensure(
            &self.registry,
            &self.context,
            &mut self.loaded,
            stt,
            engine_id,
        )?;
        Ok(Prepared {
            device: loaded.engine.device(),
            elapsed,
            reused,
        })
    }

    /// 엔진을 준비하고(가능하면 다시 쓰고) `clip`을 인식합니다.
    ///
    /// 엔진을 만들지 못하면 보관한 엔진을 비우고, 인식 단계의 오류에서는 그대로 보관합니다.
    pub fn transcribe(
        &mut self,
        clip: &AudioClip,
        job: &TranscribeJob<'_>,
    ) -> Result<Transcription, TranscribeError> {
        // 이미 취소된 작업 때문에 모델을 읽지 않습니다.
        if job.cancel.is_cancelled() {
            return Err(SttError::Cancelled.into());
        }

        let (loaded, prepare, reused) = ensure(
            &self.registry,
            &self.context,
            &mut self.loaded,
            job.stt,
            job.engine_id,
        )?;
        let capabilities = &self
            .registry
            .descriptor(job.engine_id)
            .ok_or_else(|| SttError::UnknownEngine(job.engine_id.to_owned()))?
            .capabilities;

        let language = Language::from_config(job.language);
        if !capabilities.languages.supports(&language) {
            return Err(SttError::UnsupportedLanguage(
                language.code().unwrap_or("auto").to_owned(),
            )
            .into());
        }

        // 엔진이 받는 샘플레이트로 맞춥니다 (같으면 그대로 복사).
        let audio = clip.resampled(capabilities.input_sample_rate)?;

        let prompt = job.stt.initial_prompt.trim();
        let request = TranscribeRequest {
            language,
            prompt: (capabilities.supports_prompt && !prompt.is_empty()).then(|| prompt.to_owned()),
            cancel: job.cancel.clone(),
        };

        let started = Instant::now();
        let transcript = loaded.engine.transcribe(&audio, &request)?;
        Ok(Transcription {
            transcript,
            device: loaded.engine.device(),
            prepare,
            reused,
            recognize: started.elapsed(),
        })
    }

    /// 준비해 둔 엔진을 내려놓아 메모리를 돌려줍니다.
    pub fn unload(&mut self) {
        self.loaded = None;
    }
}

/// `slot`에 `engine_id` 엔진이 지금 설정으로 준비돼 있게 합니다.
/// 돌려주는 값은 (엔진, 만드는 데 걸린 시간, 다시 썼는지)입니다.
fn ensure<'a>(
    registry: &SttRegistry,
    context: &EngineContext,
    slot: &'a mut Option<Loaded>,
    stt: &SttConfig,
    engine_id: &str,
) -> Result<(&'a mut Loaded, Duration, bool), SttError> {
    let settings = match registry.resolve_settings(stt, engine_id) {
        Ok(settings) => settings,
        Err(error) => {
            *slot = None;
            return Err(error);
        }
    };

    // 엔진 ID와 설정이 그대로면 이전 인스턴스를 다시 씁니다.
    let (loaded, elapsed, reused) = match slot.take() {
        Some(loaded) if loaded.id == engine_id && loaded.settings == settings => {
            (loaded, Duration::ZERO, true)
        }
        previous => {
            // 새 모델을 읽기 전에 이전 엔진의 메모리를 먼저 돌려줍니다.
            drop(previous);
            let started = Instant::now();
            let engine = registry.create(context, stt, engine_id)?;
            let loaded = Loaded {
                id: engine_id.to_owned(),
                settings,
                engine,
            };
            (loaded, started.elapsed(), false)
        }
    };
    Ok((slot.insert(loaded), elapsed, reused))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::paths::AppPaths;
    use crate::stt::mock::{MOCK_ENGINE_ID, MockEngineFactory};

    fn transcriber() -> Transcriber {
        let mut registry = SttRegistry::new();
        registry.register(Arc::new(MockEngineFactory::new()));
        Transcriber::new(
            registry,
            EngineContext {
                paths: AppPaths::under("unused"),
            },
        )
    }

    fn mock_config(delay_ms: i64, text: &str) -> SttConfig {
        let mut stt = SttConfig::default();
        let table = stt.engines.entry(MOCK_ENGINE_ID.to_owned()).or_default();
        table.insert("delay_ms".into(), toml::Value::Integer(delay_ms));
        table.insert("text".into(), toml::Value::String(text.to_owned()));
        stt
    }

    /// 엔진이 16kHz를 받으므로 48kHz 오디오는 변환되어야 합니다.
    fn clip() -> AudioClip {
        AudioClip::new(vec![0.1; 48_000], 48_000)
    }

    fn job<'a>(stt: &'a SttConfig, engine_id: &'a str) -> TranscribeJob<'a> {
        TranscribeJob {
            stt,
            engine_id,
            language: "ko",
            cancel: CancelToken::new(),
        }
    }

    #[test]
    fn transcriber_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Transcriber>();
    }

    #[test]
    fn transcribes_resampled_audio() {
        let stt = mock_config(0, "안녕하세요");
        let result = transcriber()
            .transcribe(&clip(), &job(&stt, MOCK_ENGINE_ID))
            .unwrap();
        assert_eq!(result.transcript.text, "안녕하세요");
        assert_eq!(result.transcript.language.as_deref(), Some("ko"));
        assert_eq!(result.device, ComputeDevice::Unknown);
        assert!(!result.reused);
    }

    #[test]
    fn engine_is_reused_only_while_settings_match() {
        let mut transcriber = transcriber();
        let one = mock_config(0, "하나");
        transcriber
            .transcribe(&clip(), &job(&one, MOCK_ENGINE_ID))
            .unwrap();
        let second = transcriber
            .transcribe(&clip(), &job(&one, MOCK_ENGINE_ID))
            .unwrap();
        assert!(second.reused);
        assert_eq!(second.prepare, Duration::ZERO);

        // 설정이 바뀌면 새로 만들어야 바뀐 값이 반영됩니다.
        let two = mock_config(0, "둘");
        let third = transcriber
            .transcribe(&clip(), &job(&two, MOCK_ENGINE_ID))
            .unwrap();
        assert!(!third.reused);
        assert_eq!(third.transcript.text, "둘");
    }

    #[test]
    fn unknown_engine_is_reported_and_clears_the_engine() {
        let mut transcriber = transcriber();
        let stt = mock_config(0, "");
        transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap();

        let error = transcriber
            .transcribe(&clip(), &job(&stt, "nope"))
            .unwrap_err();
        assert!(matches!(
            error,
            TranscribeError::Stt(SttError::UnknownEngine(id)) if id == "nope"
        ));
        // 만들지 못했으므로 보관한 엔진도 비워졌습니다.
        assert!(!transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap().reused);
    }

    #[test]
    fn invalid_settings_clear_the_engine() {
        let mut transcriber = transcriber();
        let stt = mock_config(0, "");
        transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap();

        let error = transcriber
            .prepare(&mock_config(-1, ""), MOCK_ENGINE_ID)
            .unwrap_err();
        assert!(matches!(error, SttError::InvalidSettings { .. }));
        assert!(!transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap().reused);
    }

    #[test]
    fn transcribe_reuses_prepared_engine() {
        let mut transcriber = transcriber();
        let stt = mock_config(0, "준비");
        let prepared = transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap();
        assert!(!prepared.reused);
        assert!(transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap().reused);

        let result = transcriber
            .transcribe(&clip(), &job(&stt, MOCK_ENGINE_ID))
            .unwrap();
        assert!(result.reused);
        assert_eq!(result.transcript.text, "준비");
    }

    #[test]
    fn unload_drops_the_engine() {
        let mut transcriber = transcriber();
        let stt = mock_config(0, "");
        transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap();
        transcriber.unload();
        let result = transcriber
            .transcribe(&clip(), &job(&stt, MOCK_ENGINE_ID))
            .unwrap();
        assert!(!result.reused);
    }

    #[test]
    fn already_cancelled_job_does_not_create_an_engine() {
        let mut transcriber = transcriber();
        let stt = mock_config(0, "");
        let job = job(&stt, MOCK_ENGINE_ID);
        job.cancel.cancel();

        let error = transcriber.transcribe(&clip(), &job).unwrap_err();
        assert!(matches!(error, TranscribeError::Stt(SttError::Cancelled)));
        assert!(!transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap().reused);
    }

    #[test]
    fn cancelling_from_another_thread_stops_early() {
        let mut transcriber = transcriber();
        let stt = mock_config(10_000, "");
        let job = job(&stt, MOCK_ENGINE_ID);

        let token = job.cancel.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            token.cancel();
        });
        let started = Instant::now();
        let error = transcriber.transcribe(&clip(), &job).unwrap_err();
        canceller.join().unwrap();

        assert!(matches!(error, TranscribeError::Stt(SttError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(3));
        // 인식 단계의 오류이므로 엔진은 그대로 보관합니다.
        assert!(transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap().reused);
    }
}
