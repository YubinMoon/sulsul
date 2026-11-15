//! 인식 실행 흐름: 엔진 준비(재사용) → 언어 확인 → 리샘플 → 인식.
//!
//! 화면, 명령줄 도구, 단축키 흐름이 같은 [`Transcriber`]를 씁니다. 모델 로딩처럼 비싼
//! 준비를 반복하지 않도록 엔진 인스턴스를 보관하고, 엔진 ID와 설정이 그대로면 다시 씁니다.
//! 엔진 설정의 모델 메모리 유지 시간([`KEEP_LOADED_KEY`]) 동안 쓰지 않으면
//! [`Transcriber::unload_if_idle`]로 내려놓아 메모리를 돌려줍니다. 유지 시간은 기록해 두지 않고
//! 부를 때마다 지금 설정에서 읽으므로, 쉬는 동안 값을 바꿔도 바로 적용됩니다.

use std::time::{Duration, Instant};

use super::fields::{configured_language, effective_language, keep_loaded};
use super::{
    ComputeDevice, EngineContext, KEEP_LOADED_KEY, LANGUAGE_KEY, Language, LanguageSupport,
    SttEngine, SttError, SttRegistry, TranscribeRequest, Transcript, WHISPER_SAMPLE_RATE,
};
use crate::audio::{AudioClip, AudioError, test_audio_clip};
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
    /// 엔진을 만들 때 쓴 설정에서 언어와 메모리 유지 시간을 뺀 것([`reuse_key`]).
    /// 다음 작업의 설정과 같아야 다시 씁니다.
    settings: ResolvedSettings,
    engine: Box<dyn SttEngine>,
    /// 마지막으로 준비·인식이 끝난 시각
    last_used: Instant,
}

impl Loaded {
    /// 방금 썼다고 기록합니다.
    fn touch(&mut self) {
        self.last_used = Instant::now();
    }

    /// `engine_id` 엔진을 `key`([`reuse_key`]) 설정으로 만든 인스턴스인지
    fn matches(&self, engine_id: &str, key: &ResolvedSettings) -> bool {
        self.id == engine_id && &self.settings == key
    }

    /// 유지 시간이 `keep`일 때 내려놓을 시각. 유지 시간 항목이 없는 엔진(클라우드 등)은
    /// `keep`이 `None`이고 내려놓지 않습니다.
    fn deadline(&self, keep: Option<Duration>) -> Option<Instant> {
        keep.map(|keep| self.last_used + keep)
    }

    /// 유지 시간이 `keep`일 때 `now`에 이미 내려놓을 시각이 지났는지
    fn expired(&self, keep: Option<Duration>, now: Instant) -> bool {
        self.deadline(keep).is_some_and(|deadline| now >= deadline)
    }
}

/// 인식 한 번에 필요한 값
#[derive(Debug, Clone)]
pub struct TranscribeJob<'a> {
    /// 설정 파일의 `[stt]` (엔진별 설정 포함)
    pub stt: &'a SttConfig,
    pub engine_id: &'a str,
    /// 이 작업에만 쓸 언어(`ko`, `en`, `auto` 등). `None`이면 엔진 설정의 `language`를 씁니다.
    pub language: Option<&'a str>,
    pub cancel: CancelToken,
}

/// 인식 결과와 걸린 시간
#[derive(Debug, Clone, PartialEq)]
pub struct Transcription {
    pub transcript: Transcript,
    /// 실제로 인식에 쓴 언어. 자동 감지면 [`Language::Auto`]이고, 감지한 언어는
    /// [`Transcript::language`]에 있습니다. 모델이 언어 하나만 지원하면 고른 언어와 다를 수 있습니다.
    pub language: Language,
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

/// [`test_engine`]의 End-to-End 점검 결과
#[derive(Debug, Clone, PartialEq)]
pub struct EngineTestResult {
    pub device: ComputeDevice,
    pub prepare: Duration,
    pub recognize: Duration,
    pub transcript: Transcript,
}

/// 지정한 엔진을 새로 생성하고 짧은 테스트 오디오를 실제로 넣어 동작과 지연 시간을 확인합니다.
///
/// 언어는 엔진 설정의 `language`를 쓰되, 지금 모델이 지원하지 않으면 자동 감지나 첫 언어로 바꿔
/// 점검이 언어 때문에 실패하지 않게 합니다.
pub fn test_engine(
    registry: &SttRegistry,
    ctx: &EngineContext,
    stt: &SttConfig,
    engine_id: &str,
    cancel: CancelToken,
) -> Result<EngineTestResult, TranscribeError> {
    if cancel.is_cancelled() {
        return Err(SttError::Cancelled.into());
    }
    let factory = registry
        .get(engine_id)
        .ok_or_else(|| SttError::UnknownEngine(engine_id.to_owned()))?;
    let capabilities = factory.descriptor().capabilities.clone();
    let settings = registry.resolve_settings(stt, engine_id)?;
    let support = factory.languages(&settings);

    let prepare_started = Instant::now();
    let mut engine = registry.create(ctx, stt, engine_id)?;
    let prepare = prepare_started.elapsed();
    if cancel.is_cancelled() {
        return Err(SttError::Cancelled.into());
    }

    let language = pick_test_language(&support, configured_language(&settings, stt));
    let sample_rate = capabilities.input_sample_rate.max(1);
    let clip = if sample_rate == WHISPER_SAMPLE_RATE {
        test_audio_clip(WHISPER_SAMPLE_RATE)
    } else {
        test_audio_clip(WHISPER_SAMPLE_RATE).resampled(sample_rate)?
    };
    let request = TranscribeRequest {
        language,
        prompt: None,
        cancel,
    };

    let recognize_started = Instant::now();
    let transcript = engine.transcribe(&clip, &request)?;
    Ok(EngineTestResult {
        device: engine.device(),
        prepare,
        recognize: recognize_started.elapsed(),
        transcript,
    })
}

/// 고를 수 있는 언어([`LanguageSupport::choices`]) 가운데 점검에 쓸 언어:
/// 원하는 언어가 있으면 그것, 아니면 자동 감지, 그것도 안 되면 첫 선택지입니다.
fn pick_test_language(support: &LanguageSupport, preferred: Language) -> Language {
    if support.offers(&preferred) {
        return preferred;
    }
    // 자동 감지를 지원하면 선택지 맨 앞이 `auto`입니다.
    support
        .choices()
        .first()
        .map_or(Language::Auto, |choice| Language::from_config(choice))
}

/// 엔진을 다시 쓸지 비교할 설정. 언어와 메모리 유지 시간은 모델과 상관없어서 뺍니다.
fn reuse_key(settings: &ResolvedSettings) -> ResolvedSettings {
    let mut key = settings.clone();
    key.remove(LANGUAGE_KEY);
    key.remove(KEEP_LOADED_KEY);
    key
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
    ///
    /// 준비도 "쓴 것"으로 셉니다. 유지 시간이 0분이면 준비한 엔진도 바로 내려놓을 대상입니다.
    pub fn prepare(&mut self, stt: &SttConfig, engine_id: &str) -> Result<Prepared, SttError> {
        let settings = self.resolve_settings(stt, engine_id)?;
        let (loaded, elapsed, reused) = ensure(
            &self.registry,
            &self.context,
            &mut self.loaded,
            stt,
            engine_id,
            &settings,
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
    /// 고른 언어를 지금 모델이 지원하지 않으면 모델을 읽기 전에 [`SttError::UnsupportedLanguage`]를
    /// 돌려줍니다. 단, 모델이 언어 하나만 지원하면(예: 영어 전용) 그 언어로 인식합니다.
    pub fn transcribe(
        &mut self,
        clip: &AudioClip,
        job: &TranscribeJob<'_>,
    ) -> Result<Transcription, TranscribeError> {
        // 이미 취소된 작업 때문에 모델을 읽지 않습니다.
        if job.cancel.is_cancelled() {
            return Err(SttError::Cancelled.into());
        }

        let settings = self.resolve_settings(job.stt, job.engine_id)?;
        let factory = self
            .registry
            .get(job.engine_id)
            .ok_or_else(|| SttError::UnknownEngine(job.engine_id.to_owned()))?;
        let capabilities = factory.descriptor().capabilities.clone();
        let language = effective_language(factory.as_ref(), &settings, job.stt, job.language)?;

        let (loaded, prepare, reused) = ensure(
            &self.registry,
            &self.context,
            &mut self.loaded,
            job.stt,
            job.engine_id,
            &settings,
        )?;

        // 엔진이 받는 샘플레이트로 맞춥니다 (같으면 그대로 복사).
        let audio = clip.resampled(capabilities.input_sample_rate)?;

        let prompt = job.stt.initial_prompt.trim();
        let request = TranscribeRequest {
            language: language.clone(),
            prompt: (capabilities.supports_prompt && !prompt.is_empty()).then(|| prompt.to_owned()),
            cancel: job.cancel.clone(),
        };

        let started = Instant::now();
        let transcript = loaded.engine.transcribe(&audio, &request);
        let recognize = started.elapsed();
        // 실패했어도 엔진은 방금 썼으므로 유지 시간을 다시 셉니다.
        loaded.touch();
        Ok(Transcription {
            transcript: transcript?,
            language,
            device: loaded.engine.device(),
            prepare,
            reused,
            recognize,
        })
    }

    /// 보관한 엔진을 내려놓을 시각 (마지막으로 쓴 시각 + 지금 설정의 모델 메모리 유지 시간).
    ///
    /// 유지 시간은 `stt`(지금 설정)에서 보관한 엔진의 `keep_loaded_minutes`를 다시 읽습니다.
    /// 엔진 표나 그 값이 없으면 스키마 기본값(5분)이고, 잘못된 값도 기본값으로 바뀝니다.
    /// 보관한 엔진이 없거나, 유지 시간 항목이 없는 엔진(클라우드 등)이면 `None`입니다.
    pub fn idle_deadline(&self, stt: &SttConfig) -> Option<Instant> {
        let loaded = self.loaded.as_ref()?;
        loaded.deadline(self.current_keep_loaded(stt, &loaded.id))
    }

    /// `now`가 [`Transcriber::idle_deadline`]을 지났으면 보관한 엔진을 내려놓고 `true`를 돌려줍니다.
    /// 유지 시간을 0분으로 바꾸면 다음에 부를 때 바로 내려놓습니다.
    ///
    /// 화면 스레드에서 불러도 됩니다. 엔진 해제는 짧은 스레드에서 하므로 기다리지 않습니다.
    pub fn unload_if_idle(&mut self, stt: &SttConfig, now: Instant) -> bool {
        if self
            .idle_deadline(stt)
            .is_none_or(|deadline| now < deadline)
        {
            return false;
        }
        if let Some(loaded) = self.loaded.take() {
            release(loaded, "idle");
        }
        true
    }

    /// 준비해 둔 엔진을 내려놓아 메모리를 돌려줍니다.
    pub fn unload(&mut self) {
        if let Some(loaded) = self.loaded.take() {
            release(loaded, "requested");
        }
    }

    /// 지금 설정에서 `engine_id` 엔진의 모델 메모리 유지 시간. 항목이 없는 엔진이면 `None`입니다.
    fn current_keep_loaded(&self, stt: &SttConfig, engine_id: &str) -> Option<Duration> {
        let settings = self.registry.resolve_settings(stt, engine_id).ok()?;
        keep_loaded(&settings)
    }

    /// 엔진 설정을 읽습니다. 읽지 못하면(모르는 엔진 등) 보관한 엔진도 비웁니다.
    fn resolve_settings(
        &mut self,
        stt: &SttConfig,
        engine_id: &str,
    ) -> Result<ResolvedSettings, SttError> {
        let resolved = self.registry.resolve_settings(stt, engine_id);
        if resolved.is_err() {
            self.loaded = None;
        }
        resolved
    }
}

/// 엔진을 내려놓습니다. GPU 문맥 해제는 오래 걸릴 수 있어 화면 스레드를 붙잡지 않도록
/// 짧은 스레드에서 버립니다.
fn release(loaded: Loaded, reason: &str) {
    let Loaded { id, engine, .. } = loaded;
    tracing::info!("unloading STT engine '{id}' ({reason})");
    let spawned = std::thread::Builder::new()
        .name("sulsul-unload".into())
        .spawn(move || drop(engine));
    if let Err(error) = spawned {
        // 스레드를 띄우지 못하면 넘긴 엔진은 그 자리에서 버려집니다.
        tracing::warn!("could not start the unload thread ({error}); engine dropped in place");
    }
}

/// `slot`에 `engine_id` 엔진이 `settings`로 준비돼 있게 합니다.
/// 돌려주는 값은 (엔진, 만드는 데 걸린 시간, 다시 썼는지)입니다.
///
/// 보관한 엔진이 지금 설정의 유지 시간 기준으로 이미 내려놓을 시각을 지났으면, 아직
/// [`Transcriber::unload_if_idle`]이 불리지 않았더라도 다시 쓰지 않고 새로 읽습니다.
fn ensure<'a>(
    registry: &SttRegistry,
    context: &EngineContext,
    slot: &'a mut Option<Loaded>,
    stt: &SttConfig,
    engine_id: &str,
    settings: &ResolvedSettings,
) -> Result<(&'a mut Loaded, Duration, bool), SttError> {
    let key = reuse_key(settings);
    let keep = keep_loaded(settings);
    let now = Instant::now();

    // 엔진 ID와 설정이 그대로이고 유지 시간 안이면 이전 인스턴스를 다시 씁니다.
    let (mut loaded, elapsed, reused) = match slot.take() {
        Some(loaded) if loaded.matches(engine_id, &key) && !loaded.expired(keep, now) => {
            (loaded, Duration::ZERO, true)
        }
        previous => {
            if let Some(previous) = previous {
                if previous.matches(engine_id, &key) {
                    tracing::info!("unloading STT engine '{}' (idle)", previous.id);
                }
                // 새 모델을 읽기 전에 이전 엔진의 메모리를 먼저 돌려줍니다.
                drop(previous);
            }
            let started = Instant::now();
            let engine = registry.create(context, stt, engine_id)?;
            let loaded = Loaded {
                id: engine_id.to_owned(),
                settings: key,
                engine,
                last_used: Instant::now(),
            };
            (loaded, started.elapsed(), false)
        }
    };
    loaded.touch();
    Ok((slot.insert(loaded), elapsed, reused))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::paths::AppPaths;
    use crate::stt::mock::{MOCK_ENGINE_ID, MockEngineFactory};
    use crate::stt::{
        EngineCapabilities, EngineDescriptor, EngineKind, SttEngineFactory, language_field,
    };

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

    fn set(stt: &mut SttConfig, key: &str, value: toml::Value) {
        stt.engines
            .entry(MOCK_ENGINE_ID.to_owned())
            .or_default()
            .insert(key.into(), value);
    }

    /// 엔진이 16kHz를 받으므로 48kHz 오디오는 변환되어야 합니다.
    fn clip() -> AudioClip {
        AudioClip::new(vec![0.1; 48_000], 48_000)
    }

    fn job<'a>(stt: &'a SttConfig, engine_id: &'a str) -> TranscribeJob<'a> {
        TranscribeJob {
            stt,
            engine_id,
            language: None,
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
        // 엔진 설정에 언어가 없으면 기본값(한국어)을 씁니다.
        assert_eq!(result.language, Language::Code("ko".into()));
        assert_eq!(result.transcript.language.as_deref(), Some("ko"));
        assert_eq!(result.device, ComputeDevice::Unknown);
        assert!(!result.reused);
    }

    #[test]
    fn engine_language_is_used_unless_the_job_overrides_it() {
        let mut transcriber = transcriber();
        let mut stt = mock_config(0, "");
        set(&mut stt, LANGUAGE_KEY, "ja".into());

        let from_settings = transcriber
            .transcribe(&clip(), &job(&stt, MOCK_ENGINE_ID))
            .unwrap();
        assert_eq!(from_settings.language, Language::Code("ja".into()));

        let overridden = transcriber
            .transcribe(
                &clip(),
                &TranscribeJob {
                    language: Some("auto"),
                    ..job(&stt, MOCK_ENGINE_ID)
                },
            )
            .unwrap();
        assert_eq!(overridden.language, Language::Auto);
        assert_eq!(overridden.transcript.language, None);
    }

    #[test]
    fn legacy_global_language_is_used_when_engine_has_none() {
        let mut stt = mock_config(0, "");
        stt.language = "en".into();
        let result = transcriber()
            .transcribe(&clip(), &job(&stt, MOCK_ENGINE_ID))
            .unwrap();
        assert_eq!(result.language, Language::Code("en".into()));
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
    fn language_or_keep_loaded_changes_do_not_reload_the_model() {
        let mut transcriber = transcriber();
        let mut stt = mock_config(0, "");
        transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap();

        set(&mut stt, LANGUAGE_KEY, "en".into());
        set(&mut stt, KEEP_LOADED_KEY, toml::Value::Integer(30));
        let result = transcriber
            .transcribe(&clip(), &job(&stt, MOCK_ENGINE_ID))
            .unwrap();
        assert!(
            result.reused,
            "언어·유지 시간만 바꿨는데 모델을 다시 읽었습니다"
        );
        assert_eq!(result.language, Language::Code("en".into()));
        // 바뀐 유지 시간은 바로 적용됩니다.
        let left = transcriber
            .idle_deadline(&stt)
            .unwrap()
            .saturating_duration_since(Instant::now());
        assert!(left > Duration::from_secs(29 * 60), "{left:?}");
    }

    /// 유지 시간만 `minutes`로 바꾼 가짜 엔진 설정
    fn keep_for(minutes: i64) -> SttConfig {
        let mut stt = mock_config(0, "");
        set(&mut stt, KEEP_LOADED_KEY, toml::Value::Integer(minutes));
        stt
    }

    #[test]
    fn idle_engine_is_unloaded_after_keep_loaded_minutes() {
        let mut transcriber = transcriber();
        let stt = keep_for(1);
        assert_eq!(transcriber.idle_deadline(&stt), None);
        assert!(!transcriber.unload_if_idle(&stt, Instant::now()));

        let before = Instant::now();
        transcriber
            .transcribe(&clip(), &job(&stt, MOCK_ENGINE_ID))
            .unwrap();
        let deadline = transcriber.idle_deadline(&stt).unwrap();
        assert!(deadline >= before + Duration::from_secs(60));
        assert!(deadline <= Instant::now() + Duration::from_secs(60));

        // 시각이 되기 전에는 그대로 둡니다.
        assert!(!transcriber.unload_if_idle(&stt, deadline - Duration::from_millis(1)));
        assert!(transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap().reused);

        // 준비도 "쓴 것"이므로 내려놓을 시각이 미뤄집니다.
        let deadline = transcriber.idle_deadline(&stt).unwrap();
        assert!(transcriber.unload_if_idle(&stt, deadline));
        assert_eq!(transcriber.idle_deadline(&stt), None);
        let next = transcriber
            .transcribe(&clip(), &job(&stt, MOCK_ENGINE_ID))
            .unwrap();
        assert!(!next.reused);
    }

    #[test]
    fn engine_table_without_value_uses_default_keep_loaded() {
        let mut transcriber = transcriber();
        let stt = mock_config(0, "");
        transcriber.prepare(&stt, MOCK_ENGINE_ID).unwrap();
        let used = Instant::now();
        // 엔진 표에 값이 없거나 엔진 표 자체가 없어도 기본값(5분)으로 내려놓습니다.
        for config in [&stt, &SttConfig::default()] {
            let deadline = transcriber.idle_deadline(config).unwrap();
            assert!(deadline <= used + Duration::from_secs(5 * 60));
            assert!(deadline > used + Duration::from_secs(4 * 60));
        }
    }

    #[test]
    fn zero_minutes_unloads_right_after_transcription() {
        let mut transcriber = transcriber();
        let stt = keep_for(0);
        transcriber
            .transcribe(&clip(), &job(&stt, MOCK_ENGINE_ID))
            .unwrap();
        assert!(transcriber.idle_deadline(&stt).unwrap() <= Instant::now());
        assert!(transcriber.unload_if_idle(&stt, Instant::now()));
        assert!(!transcriber.unload_if_idle(&stt, Instant::now()));
    }

    /// 0분이면 내려놓기 확인(`unload_if_idle`)이 사이에 없어도 매번 새로 읽습니다.
    #[test]
    fn zero_minutes_reloads_on_every_transcription() {
        let mut transcriber = transcriber();
        let stt = keep_for(0);
        for attempt in 1..=2 {
            let result = transcriber
                .transcribe(&clip(), &job(&stt, MOCK_ENGINE_ID))
                .unwrap();
            assert!(
                !result.reused,
                "{attempt}번째 받아쓰기가 모델을 다시 썼습니다"
            );
        }
    }

    /// 쉬는 동안 유지 시간을 0분으로 줄이면 예전 시각을 기다리지 않고 다음 확인에서 내려놓습니다.
    #[test]
    fn shortening_to_zero_unloads_on_the_next_check() {
        let mut transcriber = transcriber();
        let one_minute = keep_for(1);
        transcriber
            .transcribe(&clip(), &job(&one_minute, MOCK_ENGINE_ID))
            .unwrap();
        assert!(!transcriber.unload_if_idle(&one_minute, Instant::now()));

        let zero = keep_for(0);
        assert!(transcriber.idle_deadline(&zero).unwrap() <= Instant::now());
        assert!(transcriber.unload_if_idle(&zero, Instant::now()));
        assert_eq!(transcriber.idle_deadline(&zero), None);
    }

    /// 내려놓을 시각은 마지막 사용 시각 + 지금 설정의 유지 시간이라, 값을 줄이면 당겨지고
    /// 늘리면 그만큼 미뤄집니다.
    #[test]
    fn changed_keep_loaded_moves_the_deadline() {
        let mut transcriber = transcriber();
        let five = keep_for(5);
        let one = keep_for(1);
        transcriber
            .transcribe(&clip(), &job(&five, MOCK_ENGINE_ID))
            .unwrap();

        let after_one = transcriber.idle_deadline(&one).unwrap();
        let after_five = transcriber.idle_deadline(&five).unwrap();
        assert_eq!(after_five - after_one, Duration::from_secs(4 * 60));

        // 5분 설정 그대로면 1분 뒤에도 남겨 둡니다.
        assert!(!transcriber.unload_if_idle(&five, after_one));
        // 1분으로 줄이면 1분이 되기 직전까지는 남기고, 1분이 되면 내려놓습니다.
        assert!(!transcriber.unload_if_idle(&one, after_one - Duration::from_millis(1)));
        assert!(transcriber.unload_if_idle(&one, after_one));
    }

    /// 지금 설정 기준으로 이미 시간이 지난 엔진은 `unload_if_idle`이 아직 돌지 않았어도
    /// 받아쓰기 직전 준비에서 다시 쓰지 않고 새로 읽습니다.
    #[test]
    fn expired_engine_is_reloaded_even_before_the_idle_check() {
        let mut transcriber = transcriber();
        let five = keep_for(5);
        transcriber
            .transcribe(&clip(), &job(&five, MOCK_ENGINE_ID))
            .unwrap();
        assert!(
            transcriber
                .transcribe(&clip(), &job(&five, MOCK_ENGINE_ID))
                .unwrap()
                .reused
        );

        let zero = keep_for(0);
        let result = transcriber
            .transcribe(&clip(), &job(&zero, MOCK_ENGINE_ID))
            .unwrap();
        assert!(!result.reused, "유지 시간이 지난 엔진을 다시 썼습니다");
    }

    /// 유지 시간 항목이 없는 엔진(클라우드처럼)은 내려놓지 않습니다.
    #[test]
    fn engine_without_keep_loaded_field_stays_loaded() {
        struct Plain(EngineDescriptor);
        impl SttEngineFactory for Plain {
            fn descriptor(&self) -> &EngineDescriptor {
                &self.0
            }
            fn create(
                &self,
                ctx: &EngineContext,
                settings: &ResolvedSettings,
            ) -> Result<Box<dyn SttEngine>, SttError> {
                MockEngineFactory::new().create(ctx, settings)
            }
        }
        let mut descriptor = MockEngineFactory::new().descriptor().clone();
        descriptor.id = "plain".into();
        descriptor.settings = vec![language_field(&LanguageSupport::Any, "ko")];
        descriptor.capabilities = EngineCapabilities {
            input_sample_rate: WHISPER_SAMPLE_RATE,
            languages: LanguageSupport::Any,
            supports_prompt: false,
            streaming: false,
        };
        descriptor.kind = EngineKind::Cloud;

        let mut registry = SttRegistry::new();
        registry.register(Arc::new(Plain(descriptor)));
        let mut transcriber = Transcriber::new(
            registry,
            EngineContext {
                paths: AppPaths::under("unused"),
            },
        );
        let stt = SttConfig::default();
        transcriber.prepare(&stt, "plain").unwrap();
        assert_eq!(transcriber.idle_deadline(&stt), None);
        assert!(!transcriber.unload_if_idle(&stt, Instant::now() + Duration::from_secs(86_400)));
    }

    /// 언어 하나만 지원하는 모델은 다른 언어를 골라 두어도 그 언어로 인식하고,
    /// 여러 언어를 지원하는 모델에서 지원하지 않는 언어를 고르면 오류입니다.
    #[test]
    fn unsupported_language_falls_back_only_for_single_language_models() {
        struct Limited(EngineDescriptor, LanguageSupport);
        impl SttEngineFactory for Limited {
            fn descriptor(&self) -> &EngineDescriptor {
                &self.0
            }
            fn create(
                &self,
                ctx: &EngineContext,
                settings: &ResolvedSettings,
            ) -> Result<Box<dyn SttEngine>, SttError> {
                MockEngineFactory::new().create(ctx, settings)
            }
            fn languages(&self, _settings: &ResolvedSettings) -> LanguageSupport {
                self.1.clone()
            }
        }
        let transcriber_with = |support: LanguageSupport| {
            let mut registry = SttRegistry::new();
            let mut descriptor = MockEngineFactory::new().descriptor().clone();
            descriptor.id = "limited".into();
            registry.register(Arc::new(Limited(descriptor, support)));
            Transcriber::new(
                registry,
                EngineContext {
                    paths: AppPaths::under("unused"),
                },
            )
        };
        let mut stt = SttConfig::default();
        stt.engines.entry("limited".into()).or_default().extend([
            ("delay_ms".to_owned(), toml::Value::Integer(0)),
            (LANGUAGE_KEY.to_owned(), "ko".into()),
        ]);

        let mut english = transcriber_with(LanguageSupport::Only(vec!["en".into()]));
        let result = english.transcribe(&clip(), &job(&stt, "limited")).unwrap();
        assert_eq!(result.language, Language::Code("en".into()));
        assert_eq!(result.transcript.language.as_deref(), Some("en"));

        let mut two = transcriber_with(LanguageSupport::Only(vec!["en".into(), "ja".into()]));
        let error = two.transcribe(&clip(), &job(&stt, "limited")).unwrap_err();
        assert!(matches!(
            error,
            TranscribeError::Stt(SttError::UnsupportedLanguage(code)) if code == "ko"
        ));
        // 언어를 확인하는 단계에서 멈췄으므로 모델을 읽지 않았습니다.
        assert_eq!(two.idle_deadline(&stt), None);
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
        assert_eq!(transcriber.idle_deadline(&stt), None);
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

    #[test]
    fn test_engine_runs_end_to_end() {
        let mut registry = SttRegistry::new();
        registry.register(Arc::new(MockEngineFactory::new()));
        let ctx = EngineContext {
            paths: AppPaths::under("unused"),
        };
        let mut stt = mock_config(0, "");
        set(&mut stt, LANGUAGE_KEY, "ja".into());
        let report =
            test_engine(&registry, &ctx, &stt, MOCK_ENGINE_ID, CancelToken::new()).unwrap();
        // 가짜 엔진은 인식 언어를 결과에 적으므로, 엔진 설정의 언어를 썼는지 볼 수 있습니다.
        assert_eq!(report.transcript.language.as_deref(), Some("ja"));
        assert_eq!(report.device, ComputeDevice::Unknown);
    }

    #[test]
    fn test_language_falls_back_when_unsupported() {
        let english = LanguageSupport::Only(vec!["en".into()]);
        assert_eq!(
            pick_test_language(&english, Language::Code("ko".into())),
            Language::Code("en".into())
        );
        let auto_and = LanguageSupport::AutoAnd(vec!["ko".into()]);
        assert_eq!(
            pick_test_language(&auto_and, Language::Code("fr".into())),
            Language::Auto
        );
        // 모델이 알아도 고를 수 있는 언어가 아니면 자동 감지로 점검합니다.
        let with_cantonese = LanguageSupport::AutoAnd(vec!["ko".into(), "yue".into()]);
        assert_eq!(
            pick_test_language(&with_cantonese, Language::Code("yue".into())),
            Language::Auto
        );
        assert_eq!(
            pick_test_language(&LanguageSupport::Any, Language::Code("ja".into())),
            Language::Code("ja".into())
        );
        let no_auto = LanguageSupport::Only(vec!["ja".into(), "en".into()]);
        assert_eq!(
            pick_test_language(&no_auto, Language::Code("fr".into())),
            Language::Code("en".into())
        );
    }
}
