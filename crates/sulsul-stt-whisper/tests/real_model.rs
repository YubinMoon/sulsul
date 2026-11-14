//! 실제 모델 파일로 인식해 보는 테스트.
//!
//! 환경 변수 `SULSUL_TEST_WHISPER_MODEL`(모델 파일)과 `SULSUL_TEST_WHISPER_WAV`(WAV 파일)가
//! 모두 있을 때만 돌고, 없으면 건너뜁니다. 결과를 보려면 `-- --nocapture`를 붙이세요.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use sulsul_core::audio::{AudioClip, read_wav};
use sulsul_core::stt::{
    ComputeDevice, EngineContext, Language, SttError, TranscribeRequest, WHISPER_SAMPLE_RATE,
};
use sulsul_core::{AppPaths, CancelToken, settings};
use sulsul_stt_whisper::{compiled_gpu_backend, factory, keys};

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[test]
fn transcribes_real_audio() {
    let (Some(model), Some(wav)) = (
        env_path("SULSUL_TEST_WHISPER_MODEL"),
        env_path("SULSUL_TEST_WHISPER_WAV"),
    ) else {
        eprintln!("SULSUL_TEST_WHISPER_MODEL / SULSUL_TEST_WHISPER_WAV 가 없어 건너뜁니다");
        return;
    };

    let factory = factory();
    let mut table = sulsul_core::toml::Table::new();
    table.insert(
        keys::MODEL_PATH.into(),
        model.to_string_lossy().into_owned().into(),
    );
    let settings = settings::resolve(&factory.descriptor().settings, Some(&table));
    let ctx = EngineContext {
        paths: AppPaths::under(std::env::temp_dir().join("sulsul-whisper-test")),
    };

    let started = Instant::now();
    let mut engine = factory.create(&ctx, &settings).expect("create engine");
    let load_time = started.elapsed();

    let device = engine.device();
    println!("장치: {device:?} (모델 로딩 {load_time:.2?})");
    match compiled_gpu_backend() {
        None => assert_eq!(device, ComputeDevice::Cpu),
        Some(_) => assert_ne!(device, ComputeDevice::Unknown),
    }

    let audio = read_wav(&wav)
        .expect("read wav")
        .resampled(WHISPER_SAMPLE_RATE)
        .expect("resample");

    let started = Instant::now();
    let transcript = engine
        .transcribe(
            &audio,
            &TranscribeRequest {
                language: Language::Auto,
                prompt: None,
                cancel: CancelToken::new(),
            },
        )
        .expect("transcribe");
    println!(
        "인식 결과 ({:.2?}, 오디오 {:.2?}, 언어 {:?}): {}",
        started.elapsed(),
        audio.duration(),
        transcript.language,
        transcript.text
    );
    for segment in &transcript.segments {
        println!(
            "  [{:?} ~ {:?}] {}",
            segment.start, segment.end, segment.text
        );
    }
    assert!(!transcript.text.is_empty());
    assert!(!transcript.segments.is_empty());
    assert!(transcript.language.is_some());

    // 아주 짧은 오디오(50ms)도 오류 없이 처리되고, 지정한 언어가 그대로 돌아옵니다.
    let short = AudioClip::new(vec![0.0; 800], WHISPER_SAMPLE_RATE);
    let short_result = engine
        .transcribe(
            &short,
            &TranscribeRequest {
                language: Language::Code("en".into()),
                prompt: Some("sulsul".into()),
                cancel: CancelToken::new(),
            },
        )
        .expect("short audio");
    println!("짧은 오디오 결과: {:?}", short_result.text);
    assert_eq!(short_result.language.as_deref(), Some("en"));

    // 미리 취소한 토큰이면 바로 멈춥니다.
    let cancel = CancelToken::new();
    cancel.cancel();
    let cancelled = engine.transcribe(
        &audio,
        &TranscribeRequest {
            language: Language::Auto,
            prompt: None,
            cancel,
        },
    );
    assert!(matches!(cancelled, Err(SttError::Cancelled)));

    // 실행 중에 취소해도 끝까지 가지 않고 멈춥니다.
    let cancel = CancelToken::new();
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            cancel.cancel();
        })
    };
    let started = Instant::now();
    let cancelled = engine.transcribe(
        &audio,
        &TranscribeRequest {
            language: Language::Auto,
            prompt: None,
            cancel,
        },
    );
    canceller.join().unwrap();
    println!("실행 중 취소: {:.2?} 만에 멈춤", started.elapsed());
    assert!(matches!(cancelled, Err(SttError::Cancelled)));
}
