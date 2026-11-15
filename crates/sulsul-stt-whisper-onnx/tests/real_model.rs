//! 실제 모델 파일로 인식해 보는 테스트.
//!
//! 환경 변수 `SULSUL_TEST_WHISPER_ONNX_MODEL_DIR`(모델 파일이 있는 폴더)와 `SULSUL_TEST_WHISPER_WAV`(WAV 파일)가
//! 모두 있을 때만 돌고, 없으면 건너뜁니다. 결과를 보려면 `-- --nocapture`를 붙이세요.
//!
//! 고를 수 있는 값:
//! - `SULSUL_TEST_WHISPER_ONNX_MODEL`: 카탈로그 모델 ID (기본 `tiny-int8`). 폴더에서 이 모델의 파일 이름을 찾습니다.
//! - `SULSUL_TEST_WHISPER_ONNX_DEVICE`: `auto` / `gpu` / `cpu` (기본 `auto`)
//! - `SULSUL_TEST_WHISPER_LANGUAGE`: `auto` / `ko` / `en` … (기본 `auto`)
//!
//! 단계별 시간은 `RUST_LOG=sulsul_stt_whisper_onnx=debug`로 볼 수 있습니다.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use sulsul_core::audio::{AudioClip, read_wav};
use sulsul_core::stt::{
    ComputeDevice, EngineContext, Language, SttError, TranscribeRequest, WHISPER_SAMPLE_RATE,
};
use sulsul_core::{AppPaths, CancelToken, settings};
use sulsul_stt_whisper_onnx::{factory, find_model, keys};

fn env_text(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_text(name).map(PathBuf::from)
}

fn request(language: &Language, cancel: CancelToken) -> TranscribeRequest {
    TranscribeRequest {
        language: language.clone(),
        prompt: None,
        cancel,
    }
}

#[test]
fn transcribes_real_audio() {
    let (Some(dir), Some(wav)) = (
        env_path("SULSUL_TEST_WHISPER_ONNX_MODEL_DIR"),
        env_path("SULSUL_TEST_WHISPER_WAV"),
    ) else {
        eprintln!(
            "SULSUL_TEST_WHISPER_ONNX_MODEL_DIR / SULSUL_TEST_WHISPER_WAV 가 없어 건너뜁니다"
        );
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let model = env_text("SULSUL_TEST_WHISPER_ONNX_MODEL").unwrap_or_else(|| "tiny-int8".into());
    let device = env_text("SULSUL_TEST_WHISPER_ONNX_DEVICE").unwrap_or_else(|| "auto".into());
    let language = Language::from_config(
        &env_text("SULSUL_TEST_WHISPER_LANGUAGE").unwrap_or_else(|| "auto".into()),
    );
    let english_only = find_model(&model)
        .expect("카탈로그에 없는 모델 ID입니다")
        .is_english_only();

    let factory = factory();
    let mut table = sulsul_core::toml::Table::new();
    table.insert(keys::MODEL.into(), model.clone().into());
    table.insert(
        keys::MODEL_PATH.into(),
        dir.to_string_lossy().into_owned().into(),
    );
    table.insert(keys::DEVICE.into(), device.clone().into());
    let schema = &factory.descriptor().settings;
    assert!(
        settings::validate(schema, &table).is_empty(),
        "테스트 설정이 잘못되었습니다"
    );
    let settings = settings::resolve(schema, Some(&table));
    let ctx = EngineContext {
        paths: AppPaths::under(std::env::temp_dir().join("sulsul-whisper-onnx-test")),
    };

    let started = Instant::now();
    let mut engine = factory.create(&ctx, &settings).expect("create engine");
    let load_time = started.elapsed();
    let actual_device = engine.device();
    println!(
        "모델 {model}, 설정 장치 {device}, 실제 장치 {actual_device:?} (로딩 {load_time:.2?})"
    );
    match device.as_str() {
        "cpu" => assert_eq!(actual_device, ComputeDevice::Cpu),
        _ => assert_ne!(actual_device, ComputeDevice::Unknown),
    }

    let audio = read_wav(&wav)
        .expect("read wav")
        .resampled(WHISPER_SAMPLE_RATE)
        .expect("resample");

    // 첫 실행은 준비(DirectML 셰이더 컴파일 등)가 섞이므로 두 번 재서 둘 다 보여 줍니다.
    let mut elapsed = Vec::new();
    let mut transcript = None;
    for _ in 0..2 {
        let started = Instant::now();
        let result = engine
            .transcribe(&audio, &request(&language, CancelToken::new()))
            .expect("transcribe");
        elapsed.push(started.elapsed());
        transcript = Some(result);
    }
    let transcript = transcript.unwrap();
    println!(
        "인식 결과 (첫 실행 {:.2?}, 두 번째 {:.2?}, 오디오 {:.2?}, 언어 {:?}): {}",
        elapsed[0],
        elapsed[1],
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
    if english_only {
        assert_eq!(transcript.language.as_deref(), Some("en"));
    }

    // 아주 짧은 오디오(50ms)도 오류 없이 처리되고, 지정한 언어가 그대로 돌아옵니다.
    let short = AudioClip::new(vec![0.0; 800], WHISPER_SAMPLE_RATE);
    let short_result = engine
        .transcribe(
            &short,
            &request(&Language::Code("en".into()), CancelToken::new()),
        )
        .expect("short audio");
    println!("짧은 오디오 결과: {:?}", short_result.text);
    assert_eq!(short_result.language.as_deref(), Some("en"));

    // 미리 취소한 토큰이면 바로 멈춥니다.
    let cancel = CancelToken::new();
    cancel.cancel();
    let cancelled = engine.transcribe(&audio, &request(&language, cancel));
    assert!(matches!(cancelled, Err(SttError::Cancelled)));

    // 실행 중에 취소해도 끝까지 가지 않고 멈춥니다 (걸린 시간의 1/3 지점에서 취소).
    let warm = elapsed[1];
    if warm < Duration::from_millis(30) {
        println!("인식이 너무 빨라 실행 중 취소는 건너뜁니다");
        return;
    }
    let cancel = CancelToken::new();
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(warm / 3);
            cancel.cancel();
        })
    };
    let started = Instant::now();
    let cancelled = engine.transcribe(&audio, &request(&language, cancel));
    canceller.join().unwrap();
    println!("실행 중 취소: {:.2?} 만에 멈춤", started.elapsed());
    assert!(matches!(cancelled, Err(SttError::Cancelled)));
}
