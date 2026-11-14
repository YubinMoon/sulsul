//! 받아쓰기 작업 스레드.
//!
//! 엔진 준비(모델 로딩)와 인식은 오래 걸릴 수 있어 화면 스레드 밖에서 돌립니다.
//! 엔진 인스턴스는 작업이 끝나면 결과와 함께 돌려받아 다음 작업에 다시 씁니다.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use sulsul_core::audio::{AudioClip, AudioError};
use sulsul_core::config::SttConfig;
use sulsul_core::settings::ResolvedSettings;
use sulsul_core::stt::{
    EngineContext, Language, SttEngine, SttError, SttRegistry, TranscribeRequest,
};

use crate::session::Recorded;

pub struct JobInput {
    pub registry: SttRegistry,
    pub context: EngineContext,
    /// 설정 파일의 `[stt]` (엔진별 설정 포함)
    pub stt: SttConfig,
    pub engine_id: String,
    /// `ko`, `en`, `auto` 등
    pub language: String,
    pub audio: JobAudio,
    /// 이전 작업에서 쓴 엔진. 엔진 ID와 설정이 같으면 다시 씁니다.
    pub cached: Option<CachedEngine>,
}

pub enum JobAudio {
    Recorded(Arc<Recorded>),
    File(Arc<AudioClip>),
}

impl JobAudio {
    pub fn clip(&self) -> &AudioClip {
        match self {
            Self::Recorded(recorded) => &recorded.clip,
            Self::File(clip) => clip,
        }
    }
}

pub struct CachedEngine {
    id: String,
    settings: ResolvedSettings,
    engine: Box<dyn SttEngine>,
}

pub struct JobOutput {
    /// 다음 작업에 다시 쓸 엔진 (엔진을 만들지 못했으면 없음)
    pub cached: Option<CachedEngine>,
    pub result: Result<Success, Failure>,
}

#[derive(Debug)]
pub struct Success {
    pub text: String,
    /// 엔진이 판단한 언어 (알 수 있을 때)
    pub detected_language: Option<String>,
    /// 엔진을 만드는 데 걸린 시간 (다시 쓴 경우 0)
    pub prepare: Duration,
    /// 이전 엔진 인스턴스를 다시 썼는지
    pub reused: bool,
    /// 인식(`transcribe`)에 걸린 시간
    pub recognize: Duration,
}

#[derive(Debug)]
pub enum Failure {
    Stt(SttError),
    Audio(AudioError),
    /// 작업 스레드가 결과 없이 끝남
    Crashed,
}

/// 작업 스레드를 시작합니다. 끝나면 결과를 보내고 화면을 다시 그리게 합니다.
pub fn spawn(input: JobInput, ctx: egui::Context) -> Receiver<JobOutput> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("sulsul-transcribe".into())
        .spawn(move || {
            let output = run(input);
            // 받는 쪽이 사라졌다면 화면이 닫힌 것이므로 결과를 버립니다.
            let _ = tx.send(output);
            ctx.request_repaint();
        });
    if let Err(error) = spawned {
        // 보내는 쪽이 버려져서 받는 쪽은 "결과 없이 끝남"으로 처리합니다.
        tracing::error!("could not start the transcription thread: {error}");
    }
    rx
}

fn run(input: JobInput) -> JobOutput {
    let JobInput {
        registry,
        context,
        stt,
        engine_id,
        language,
        audio,
        cached,
    } = input;

    let fail = |cached, error: SttError| JobOutput {
        cached,
        result: Err(Failure::Stt(error)),
    };

    let Some(descriptor) = registry.descriptor(&engine_id) else {
        return fail(None, SttError::UnknownEngine(engine_id));
    };
    let settings = match registry.resolve_settings(&stt, &engine_id) {
        Ok(settings) => settings,
        Err(error) => return fail(None, error),
    };

    // 엔진 준비: 엔진 ID와 설정이 그대로면 이전 인스턴스를 다시 씁니다.
    let started = Instant::now();
    let (mut engine, reused) = match cached {
        Some(cached) if cached.id == engine_id && cached.settings == settings => {
            (cached.engine, true)
        }
        _ => match registry.create(&context, &stt, &engine_id) {
            Ok(engine) => (engine, false),
            Err(error) => return fail(None, error),
        },
    };
    let prepare = if reused {
        Duration::ZERO
    } else {
        started.elapsed()
    };

    let result = recognize(engine.as_mut(), descriptor, &stt, &language, audio.clip()).map(
        |(transcript, recognize)| Success {
            text: transcript.text,
            detected_language: transcript.language,
            prepare,
            reused,
            recognize,
        },
    );
    tracing::info!(
        engine = %engine_id,
        reused,
        prepare_ms = prepare.as_millis(),
        ok = result.is_ok(),
        "transcription finished"
    );
    JobOutput {
        cached: Some(CachedEngine {
            id: engine_id,
            settings,
            engine,
        }),
        result,
    }
}

fn recognize(
    engine: &mut dyn SttEngine,
    descriptor: &sulsul_core::stt::EngineDescriptor,
    stt: &SttConfig,
    language: &str,
    clip: &AudioClip,
) -> Result<(sulsul_core::stt::Transcript, Duration), Failure> {
    let capabilities = &descriptor.capabilities;
    let language = Language::from_config(language);
    if !capabilities.languages.supports(&language) {
        return Err(Failure::Stt(SttError::UnsupportedLanguage(
            language.code().unwrap_or("auto").to_owned(),
        )));
    }

    // 엔진이 받는 샘플레이트로 맞춥니다 (같으면 그대로 복사).
    let audio = clip
        .resampled(capabilities.input_sample_rate)
        .map_err(Failure::Audio)?;

    let prompt = stt.initial_prompt.trim();
    let request = TranscribeRequest {
        language,
        prompt: (capabilities.supports_prompt && !prompt.is_empty()).then(|| prompt.to_owned()),
    };

    let started = Instant::now();
    let transcript = engine.transcribe(&audio, &request).map_err(Failure::Stt)?;
    Ok((transcript, started.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::AppPaths;
    use sulsul_core::stt::mock::{MOCK_ENGINE_ID, MockEngineFactory};
    use sulsul_core::toml::Value;

    fn registry() -> SttRegistry {
        let mut registry = SttRegistry::new();
        registry.register(Arc::new(MockEngineFactory::new()));
        registry.register(sulsul_stt_whisper::factory());
        registry
    }

    fn input(engine_id: &str, stt: SttConfig, cached: Option<CachedEngine>) -> JobInput {
        JobInput {
            registry: registry(),
            context: EngineContext {
                paths: AppPaths::under("unused"),
            },
            stt,
            engine_id: engine_id.to_owned(),
            language: "ko".to_owned(),
            // 엔진이 16kHz를 받으므로 48kHz 오디오는 변환되어야 합니다.
            audio: JobAudio::File(Arc::new(AudioClip::new(vec![0.1; 48_000], 48_000))),
            cached,
        }
    }

    fn mock_config(delay_ms: i64, text: &str) -> SttConfig {
        let mut stt = SttConfig::default();
        let table = stt.engines.entry(MOCK_ENGINE_ID.to_owned()).or_default();
        table.insert("delay_ms".into(), Value::Integer(delay_ms));
        table.insert("text".into(), Value::String(text.to_owned()));
        stt
    }

    #[test]
    fn mock_engine_transcribes_resampled_audio() {
        let output = run(input(MOCK_ENGINE_ID, mock_config(0, "안녕하세요"), None));
        let success = output.result.unwrap();
        assert_eq!(success.text, "안녕하세요");
        assert!(!success.reused);
        assert!(output.cached.is_some());
    }

    #[test]
    fn engine_is_reused_only_while_settings_match() {
        let first = run(input(MOCK_ENGINE_ID, mock_config(0, "하나"), None));
        let second = run(input(MOCK_ENGINE_ID, mock_config(0, "하나"), first.cached));
        let success = second.result.unwrap();
        assert!(success.reused);
        assert_eq!(success.prepare, Duration::ZERO);

        // 설정이 바뀌면 새로 만들어야 바뀐 값이 반영됩니다.
        let third = run(input(MOCK_ENGINE_ID, mock_config(0, "둘"), second.cached));
        let success = third.result.unwrap();
        assert!(!success.reused);
        assert_eq!(success.text, "둘");
    }

    #[test]
    fn whisper_reports_not_implemented() {
        let output = run(input(
            sulsul_stt_whisper::ENGINE_ID,
            SttConfig::default(),
            None,
        ));
        assert!(matches!(
            output.result,
            Err(Failure::Stt(SttError::NotImplemented(id))) if id == sulsul_stt_whisper::ENGINE_ID
        ));
        assert!(output.cached.is_none());
    }

    #[test]
    fn unknown_engine_is_reported() {
        let output = run(input("nope", SttConfig::default(), None));
        assert!(matches!(
            output.result,
            Err(Failure::Stt(SttError::UnknownEngine(id))) if id == "nope"
        ));
    }

    #[test]
    fn spawned_job_delivers_result() {
        let rx = spawn(
            input(MOCK_ENGINE_ID, mock_config(0, "스레드"), None),
            egui::Context::default(),
        );
        let output = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(output.result.unwrap().text, "스레드");
    }
}
