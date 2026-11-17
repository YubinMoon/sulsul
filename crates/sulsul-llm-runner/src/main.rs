//! 글 다듬기 로컬 모델 실행기.
//!
//! 앱(`sulsul`)이 띄우고 표준 입출력으로 JSON 한 줄씩 주고받습니다. 주고받는 말과, llama.cpp를 앱이 아닌
//! 이 실행 파일에 넣는 까닭은 `sulsul_llm::runner`에 적었습니다. 표준 출력은 답에만 쓰고 로그는 표준 오류로
//! 냅니다. 표준 입력이 닫히면 끝납니다.
//!
//! 기능 플래그: 기본은 Vulkan, `--features cuda`는 CUDA, `--no-default-features`는 CPU 전용입니다.
//! 쓸 수 있는 GPU가 없거나 모델을 GPU에 올리지 못하면 CPU로 돌립니다.

mod engine;

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

use sulsul_llm::runner::{Failure, Reply, Request};

/// 모델이 만든 글과 토큰 수
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub text: String,
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    pub truncated: bool,
}

/// 모델을 올리고 글을 만드는 쪽 (테스트에서는 가짜로 바꿉니다)
pub trait Engine {
    /// `model`을 올리고 올린 장치 이름을 돌려줍니다. 이미 올려 두었으면 다시 올리지 않습니다.
    fn load(&mut self, model: &Path, use_gpu: bool) -> Result<String, Failure>;

    /// 올려 둔 모델로 `prompt` 뒤를 이어 글을 만듭니다.
    fn generate(&mut self, prompt: &str) -> Result<Generation, Failure>;
}

fn main() -> ExitCode {
    init_logging();
    let mut engine = match engine::LlamaEngine::new() {
        Ok(engine) => engine,
        Err(error) => {
            tracing::error!("could not initialize llama.cpp: {error}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!("local model runner ready");
    match serve(io::stdin().lock(), io::stdout().lock(), &mut engine) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("local model runner stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// 로그는 표준 오류로 냅니다(앱이 받아 앱 로그에 옮김). llama.cpp 로그는 경고 이상만 남깁니다.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,sulsul_llm_runner=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_ansi(false)
        .init();
    llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default());
}

/// 요청 한 줄마다 답 한 줄을 씁니다. 입력이 끝나면 돌아옵니다.
fn serve(input: impl BufRead, mut output: impl Write, engine: &mut impl Engine) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle(engine, &request),
            Err(error) => Reply::Failed {
                error: Failure::BadRequest(error.to_string()),
            },
        };
        let mut text = serde_json::to_string(&reply).map_err(io::Error::other)?;
        text.push('\n');
        output.write_all(text.as_bytes())?;
        output.flush()?;
    }
    Ok(())
}

fn handle(engine: &mut impl Engine, request: &Request) -> Reply {
    let device = match engine.load(&request.model, request.use_gpu) {
        Ok(device) => device,
        Err(error) => return Reply::Failed { error },
    };
    let Some(prompt) = &request.prompt else {
        return Reply::Ready { device };
    };
    match engine.generate(prompt) {
        Ok(generation) => Reply::Done {
            text: generation.text,
            device,
            prompt_tokens: generation.prompt_tokens,
            output_tokens: generation.output_tokens,
            truncated: generation.truncated,
        },
        Err(error) => Reply::Failed { error },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// 시험용 엔진: `bad.gguf`는 올리지 못하고, 받은 프롬프트를 거꾸로 돌려줍니다.
    #[derive(Default)]
    struct FakeEngine {
        loads: Vec<(PathBuf, bool)>,
    }

    impl Engine for FakeEngine {
        fn load(&mut self, model: &Path, use_gpu: bool) -> Result<String, Failure> {
            if model.ends_with("bad.gguf") {
                return Err(Failure::Load("broken file".into()));
            }
            self.loads.push((model.to_path_buf(), use_gpu));
            Ok(if use_gpu { "GPU" } else { "CPU" }.into())
        }

        fn generate(&mut self, prompt: &str) -> Result<Generation, Failure> {
            Ok(Generation {
                text: prompt.chars().rev().collect(),
                prompt_tokens: 3,
                output_tokens: 3,
                truncated: false,
            })
        }
    }

    fn run(input: &str) -> (Vec<Reply>, FakeEngine) {
        let mut engine = FakeEngine::default();
        let mut output = Vec::new();
        serve(input.as_bytes(), &mut output, &mut engine).unwrap();
        let replies = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        (replies, engine)
    }

    #[test]
    fn each_request_line_gets_one_reply_line() {
        let input = concat!(
            r#"{"model":"a.gguf","use_gpu":true}"#,
            "\n\n",
            r#"{"model":"a.gguf","use_gpu":false,"prompt":"가나다"}"#,
            "\n",
            r#"{"model":"bad.gguf","use_gpu":true,"prompt":"x"}"#,
            "\n",
            "not json\n",
        );
        let (replies, engine) = run(input);
        assert_eq!(replies.len(), 4, "빈 줄에는 답하지 않습니다: {replies:?}");
        assert_eq!(
            replies[0],
            Reply::Ready {
                device: "GPU".into()
            }
        );
        assert_eq!(
            replies[1],
            Reply::Done {
                text: "다나가".into(),
                device: "CPU".into(),
                prompt_tokens: 3,
                output_tokens: 3,
                truncated: false,
            }
        );
        assert_eq!(
            replies[2],
            Reply::Failed {
                error: Failure::Load("broken file".into())
            }
        );
        assert!(matches!(
            &replies[3],
            Reply::Failed {
                error: Failure::BadRequest(_)
            }
        ));
        assert_eq!(
            engine.loads,
            [
                (PathBuf::from("a.gguf"), true),
                (PathBuf::from("a.gguf"), false)
            ]
        );
    }
}
