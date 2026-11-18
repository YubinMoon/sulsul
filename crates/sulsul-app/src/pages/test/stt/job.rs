//! 받아쓰기 작업 스레드.
//!
//! 엔진 준비(모델 로딩)와 인식은 오래 걸릴 수 있어 화면 스레드 밖에서 돌립니다.
//! 실제 흐름은 [`Transcriber`](sulsul_core::stt::Transcriber)가 맡고, 여기서는 스레드를 띄워
//! 결과를 돌려주기만 합니다. 엔진은 단축키 받아쓰기와 함께 쓰는 [`SharedTranscriber`]에 보관되어
//! 다음 작업에 다시 쓰입니다.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use sulsul_core::CancelToken;
use sulsul_core::audio::{AudioClip, AudioError};
use sulsul_core::config::SttConfig;
use sulsul_core::stt::{ComputeDevice, SttError, TranscribeError, TranscribeJob};

use crate::transcriber::SharedTranscriber;

pub struct JobInput {
    /// 엔진 보관소. 엔진 ID와 설정이 같으면 보관한 엔진을 다시 씁니다.
    pub transcriber: SharedTranscriber,
    /// 설정 파일의 `[stt]` (엔진별 설정 포함)
    pub stt: SttConfig,
    pub engine_id: String,
    /// 받아쓸 오디오 (정리한 녹음 또는 WAV 파일)
    pub audio: Arc<AudioClip>,
    /// 작업을 중간에 멈추는 신호
    pub cancel: CancelToken,
}

pub struct JobOutput {
    pub result: Result<Success, Failure>,
}

#[derive(Debug)]
pub struct Success {
    pub text: String,
    /// 실제로 인식에 쓴 언어 (`auto` 또는 언어 코드). 영어 전용 모델이면 고른 언어와 다를 수 있습니다.
    pub language: String,
    /// 엔진이 판단한 언어 (알 수 있을 때)
    pub detected_language: Option<String>,
    /// 엔진이 계산에 쓴 장치 (결과에 표시)
    pub device: ComputeDevice,
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

impl From<TranscribeError> for Failure {
    fn from(error: TranscribeError) -> Self {
        match error {
            TranscribeError::Stt(error) => Self::Stt(error),
            TranscribeError::Audio(error) => Self::Audio(error),
        }
    }
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
        transcriber,
        stt,
        engine_id,
        audio,
        cancel,
    } = input;

    let job = TranscribeJob {
        stt: &stt,
        engine_id: &engine_id,
        // 단축키 받아쓰기처럼 엔진 설정의 언어를 씁니다.
        language: None,
        cancel,
    };
    // 단축키 받아쓰기가 엔진을 쓰고 있으면 끝날 때까지 기다립니다.
    let result = match transcriber.lock().transcribe(&audio, &job) {
        Ok(done) => {
            tracing::info!(
                engine = %engine_id,
                device = ?done.device,
                reused = done.reused,
                prepare_ms = done.prepare.as_millis(),
                recognize_ms = done.recognize.as_millis(),
                "transcription finished"
            );
            Ok(Success {
                text: done.transcript.text,
                language: done.language.as_config().to_owned(),
                detected_language: done.transcript.language,
                device: done.device,
                prepare: done.prepare,
                reused: done.reused,
                recognize: done.recognize,
            })
        }
        // 실패 원인은 결과를 받는 화면 쪽에서 기록합니다.
        Err(error) => Err(Failure::from(error)),
    };
    JobOutput { result }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::AppPaths;
    use sulsul_core::stt::SttRegistry;
    use sulsul_core::stt::mock::{MOCK_ENGINE_ID, MockEngineFactory};
    use sulsul_core::toml::Value;

    fn transcriber() -> SharedTranscriber {
        let mut registry = SttRegistry::new();
        registry.register(Arc::new(MockEngineFactory::new()));
        SharedTranscriber::new(registry, AppPaths::under("unused"))
    }

    fn input(engine_id: &str, text: &str, transcriber: SharedTranscriber) -> JobInput {
        let mut stt = SttConfig::default();
        let table = stt.engines.entry(MOCK_ENGINE_ID.to_owned()).or_default();
        table.insert("delay_ms".into(), Value::Integer(0));
        table.insert("text".into(), Value::String(text.to_owned()));
        JobInput {
            transcriber,
            stt,
            engine_id: engine_id.to_owned(),
            // 엔진이 16kHz를 받으므로 48kHz 오디오는 변환되어야 합니다.
            audio: Arc::new(AudioClip::new(vec![0.1; 48_000], 48_000)),
            cancel: CancelToken::new(),
        }
    }

    fn run_on_thread(input: JobInput) -> JobOutput {
        spawn(input, egui::Context::default())
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
    }

    #[test]
    fn spawned_job_delivers_result_and_reuses_engine() {
        let shared = transcriber();
        let first = run_on_thread(input(MOCK_ENGINE_ID, "스레드", shared.clone()));
        let success = first.result.unwrap();
        assert_eq!(success.text, "스레드");
        // 엔진 설정에 언어가 없으면 기본값(한국어)으로 인식합니다.
        assert_eq!(success.language, "ko");
        assert_eq!(success.detected_language.as_deref(), Some("ko"));
        assert_eq!(success.device, ComputeDevice::Unknown);
        assert!(!success.reused);

        // 같은 보관소를 쓰면 엔진을 다시 씁니다.
        let second = run_on_thread(input(MOCK_ENGINE_ID, "스레드", shared));
        assert!(second.result.unwrap().reused);
    }

    #[test]
    fn failure_is_delivered_and_engine_store_stays_usable() {
        let shared = transcriber();
        let output = run_on_thread(input("nope", "", shared.clone()));
        assert!(matches!(
            output.result,
            Err(Failure::Stt(SttError::UnknownEngine(id))) if id == "nope"
        ));
        // 실패한 뒤에도 같은 보관소로 다음 작업을 할 수 있습니다.
        let next = run_on_thread(input(MOCK_ENGINE_ID, "다음", shared));
        assert_eq!(next.result.unwrap().text, "다음");
    }

    #[test]
    fn cancelled_job_reports_cancelled() {
        let input = input(MOCK_ENGINE_ID, "", transcriber());
        input.cancel.cancel();
        let output = run_on_thread(input);
        assert!(matches!(
            output.result,
            Err(Failure::Stt(SttError::Cancelled))
        ));
    }
}
