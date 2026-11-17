//! 빌드한 실행기를 [`LocalRunner`]로 띄워 앱과 같은 방식으로 주고받는 테스트.
//!
//! 실제 모델로 글을 다듬는 테스트는 환경 변수 `SULSUL_TEST_LLM_MODEL`(GGUF 파일)이 있을 때만 돌고,
//! 없으면 건너뜁니다. 모델 계열은 `SULSUL_TEST_LLM_FAMILY`(기본 `qwen3_5`)로 고릅니다.
//! 결과를 보려면 `-- --nocapture`를 붙이세요.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sulsul_core::CancelToken;
use sulsul_llm::chat::{ChatFormat, clean_output};
use sulsul_llm::runner::{Failure, LocalError, LocalRunner};

const TIMEOUT: Duration = Duration::from_secs(240);

fn runner() -> LocalRunner {
    LocalRunner::spawn(Path::new(env!("CARGO_BIN_EXE_sulsul-llm-runner")))
        .expect("실행기를 띄우지 못했습니다")
}

#[test]
fn missing_model_is_reported_and_the_runner_keeps_serving() {
    let mut runner = runner();
    let missing = std::env::temp_dir().join("sulsul-llm-runner-test-missing.gguf");
    let cancel = CancelToken::new();

    for _ in 0..2 {
        let error = runner
            .generate(&missing, false, "안녕", TIMEOUT, &cancel)
            .expect_err("없는 모델로는 글을 만들 수 없어야 합니다");
        assert!(
            matches!(&error, LocalError::Failed(Failure::ModelMissing(path)) if path.contains("missing")),
            "{error:?}"
        );
        assert!(
            error.runner_is_healthy(),
            "모델이 없다는 답은 실행기가 멀쩡하다는 뜻입니다"
        );
    }
}

#[test]
fn cancelled_request_is_not_sent() {
    let mut runner = runner();
    let cancel = CancelToken::new();
    cancel.cancel();
    let missing = std::env::temp_dir().join("sulsul-llm-runner-test-cancel.gguf");
    let result = runner.load(&missing, false, TIMEOUT, &cancel);
    assert!(matches!(result, Err(LocalError::Cancelled)), "{result:?}");

    // 취소는 그 요청에만 해당하고, 실행기는 다음 요청을 받습니다.
    let result = runner.load(&missing, false, TIMEOUT, &CancelToken::new());
    assert!(
        matches!(result, Err(LocalError::Failed(Failure::ModelMissing(_)))),
        "{result:?}"
    );
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[test]
fn refines_text_with_a_real_model() {
    let Some(model) = env_path("SULSUL_TEST_LLM_MODEL") else {
        eprintln!("SULSUL_TEST_LLM_MODEL 이 없어 건너뜁니다");
        return;
    };
    let family = std::env::var("SULSUL_TEST_LLM_FAMILY").unwrap_or_else(|_| "qwen3_5".into());
    let format = ChatFormat::for_family(&family)
        .unwrap_or_else(|| panic!("모르는 모델 계열입니다: {family}"));
    let prompt = format.render(
        "음성 인식으로 받아쓴 글의 맞춤법, 띄어쓰기, 문장 부호만 바로잡으세요. 설명 없이 고친 글만 답하세요.",
        "오늘 회의는 세시에 시작 하고 끝나면 보고서를 정리 해서 보내 주세요",
    );

    let mut runner = runner();
    let cancel = CancelToken::new();
    for use_gpu in [true, false] {
        let started = Instant::now();
        let generated = runner
            .generate(&model, use_gpu, &prompt, TIMEOUT, &cancel)
            .unwrap_or_else(|error| panic!("글을 만들지 못했습니다 (GPU {use_gpu}): {error}"));
        let text = clean_output(&generated.text);
        eprintln!(
            "GPU {use_gpu} · {} · {:.1}초 · 출력 {}토큰\n{text}",
            generated.device,
            started.elapsed().as_secs_f32(),
            generated.output_tokens
        );
        assert!(!generated.truncated, "끝까지 만들지 못했습니다: {text}");
        assert!(
            text.contains("회의"),
            "받아쓴 글의 내용이 남아 있어야 합니다: {text}"
        );
        assert!(
            !text.contains("<think>") && !text.contains("</think>"),
            "{text}"
        );
        if !use_gpu {
            assert_eq!(generated.device, "CPU");
        }
    }
}
