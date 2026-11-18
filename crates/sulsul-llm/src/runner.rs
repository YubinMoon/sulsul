//! 로컬 모델 실행기(`sulsul-llm-runner`)와 주고받는 말, 그리고 실행기 프로세스를 다루는 손잡이.
//!
//! llama.cpp(로컬 LLM)와 whisper.cpp(음성 인식)는 각자 ggml을 정적으로 링크해서, 한 실행 파일에 함께
//! 넣으면 같은 이름의 기호가 겹쳐 링크가 실패합니다. 그래서 로컬 모델은 앱과 따로 빌드하는 실행기
//! 프로세스에서 돌리고, 앱은 표준 입출력으로 JSON 한 줄씩 주고받습니다.
//!
//! - 앱이 [`Request`] 한 줄을 쓰면 실행기는 [`Reply`] 한 줄로 답합니다. 한 번에 요청 하나만 보냅니다.
//! - 실행기는 모델을 올려 둔 채 다음 요청을 기다리고, 다른 모델을 요청하면 앞 모델을 내려놓고 새로 올립니다.
//! - 표준 입력이 닫히면(앱이 끝나거나 죽으면) 실행기도 끝납니다. 실행기의 로그는 표준 오류로 나오고,
//!   [`LocalRunner`]가 앱 로그로 옮깁니다.
//! - 앱이 기다리다 취소하거나 제한 시간이 지나면 실행기를 끝내고, 다음 요청 때 새로 띄웁니다.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sulsul_core::CancelToken;

/// 실행기 실행 파일 이름 (확장자 제외)
pub const RUNNER_NAME: &str = "sulsul-llm-runner";

/// 취소·제한 시간을 확인하는 간격
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// 실행기에 보내는 요청 한 줄
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// GGUF 모델 파일
    pub model: PathBuf,
    /// GPU에 올릴지. 쓸 수 있는 GPU가 없거나 GPU에 올리지 못하면 실행기가 CPU로 돌립니다.
    pub use_gpu: bool,
    /// 채팅 형식으로 이어 붙인 프롬프트([`crate::chat::ChatFormat::render`]). 없으면 모델만 올립니다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// 실행기의 답 한 줄
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Reply {
    /// 모델을 올렸습니다 (`prompt`가 없는 요청의 답).
    Ready {
        /// 모델을 올린 장치 (예: `"Vulkan0 (NVIDIA GeForce RTX 4070)"`, `"CPU"`)
        device: String,
    },
    /// 글을 만들었습니다.
    Done {
        /// 모델이 만든 글 그대로 (생각 블록 정리 전)
        text: String,
        device: String,
        prompt_tokens: u32,
        output_tokens: u32,
        /// 끝 토큰이 나오기 전에 최대 길이에 닿아 잘렸는지
        truncated: bool,
    },
    Failed {
        error: Failure,
    },
}

/// 실행기가 알려 온 실패
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum Failure {
    #[error("model file not found: {0}")]
    ModelMissing(String),
    #[error("failed to load the model: {0}")]
    Load(String),
    /// 프롬프트가 모델이 받을 수 있는 길이(토큰 수)를 넘습니다.
    #[error("prompt is too long ({0} tokens)")]
    PromptTooLong(u32),
    #[error("text generation failed: {0}")]
    Generate(String),
    #[error("bad request: {0}")]
    BadRequest(String),
}

/// 실행기로 다듬은 결과
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    pub text: String,
    pub device: String,
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocalError {
    #[error("local model runner not found: {}", .0.display())]
    RunnerMissing(PathBuf),
    #[error("could not start the local model runner: {0}")]
    Spawn(String),
    /// 실행기가 답하지 않고 끝났거나 이상한 답을 보냈습니다.
    #[error("local model runner stopped unexpectedly: {0}")]
    Crashed(String),
    #[error("local model runner did not answer within {} seconds", .0.as_secs().max(1))]
    Timeout(Duration),
    #[error(transparent)]
    Failed(#[from] Failure),
    #[error("cancelled")]
    Cancelled,
}

impl LocalError {
    /// 실행기가 멀쩡히 답한 실패인지. 아니면 실행기를 버리고 다음에 새로 띄웁니다.
    pub fn runner_is_healthy(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

/// 이 실행 파일 옆의 실행기 경로 (`sulsul-llm-runner.exe` 등)
pub fn default_runner_path() -> PathBuf {
    let file_name = format!("{RUNNER_NAME}{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&file_name)))
        .unwrap_or_else(|| PathBuf::from(file_name))
}

/// 띄워 둔 실행기 프로세스. 버리면 프로세스를 끝냅니다.
pub struct LocalRunner {
    child: Child,
    stdin: ChildStdin,
    /// 실행기 표준 출력의 줄들 (읽는 스레드가 넣음). 끊기면 실행기가 끝난 것입니다.
    replies: Receiver<String>,
    /// 지금 올려 둔 모델과 GPU 사용 여부
    loaded: Option<(PathBuf, bool)>,
}

impl LocalRunner {
    /// 실행기를 띄웁니다. 모델은 아직 올리지 않습니다.
    pub fn spawn(exe: &Path) -> Result<Self, LocalError> {
        if !exe.is_file() {
            return Err(LocalError::RunnerMissing(exe.to_path_buf()));
        }
        let mut command = Command::new(exe);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            /// 콘솔 창을 띄우지 않습니다.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .map_err(|e| LocalError::Spawn(e.to_string()))?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LocalError::Spawn("runner pipes are missing".into()));
        };

        let (tx, replies) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("sulsul-llm-runner-out".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
            .and_then(|_| {
                thread::Builder::new()
                    .name("sulsul-llm-runner-log".into())
                    .spawn(move || {
                        for line in BufReader::new(stderr).lines() {
                            let Ok(line) = line else { break };
                            tracing::debug!(target: "sulsul_llm_runner", "{line}");
                        }
                    })
            });
        if let Err(error) = spawned {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LocalError::Spawn(format!(
                "failed to start runner reader threads: {error}"
            )));
        }
        tracing::info!(pid = child.id(), "local model runner started");
        Ok(Self {
            child,
            stdin,
            replies,
            loaded: None,
        })
    }

    /// `model`을 올려 둡니다. 이미 올려 두었으면 바로 돌아옵니다. 올린 장치를 돌려줍니다(이미 올렸으면 `None`).
    pub fn load(
        &mut self,
        model: &Path,
        use_gpu: bool,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Option<String>, LocalError> {
        if self.is_loaded(model, use_gpu) {
            return Ok(None);
        }
        let request = Request {
            model: model.to_path_buf(),
            use_gpu,
            prompt: None,
        };
        match self.exchange(&request, timeout, cancel)? {
            Reply::Ready { device } => {
                self.loaded = Some((model.to_path_buf(), use_gpu));
                Ok(Some(device))
            }
            other => Err(unexpected(&other)),
        }
    }

    /// `prompt` 뒤를 이어 글을 만듭니다. 모델이 올라가 있지 않으면 먼저 올립니다(그 시간도 `timeout`에 듭니다).
    pub fn generate(
        &mut self,
        model: &Path,
        use_gpu: bool,
        prompt: &str,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Generated, LocalError> {
        let request = Request {
            model: model.to_path_buf(),
            use_gpu,
            prompt: Some(prompt.to_owned()),
        };
        match self.exchange(&request, timeout, cancel)? {
            Reply::Done {
                text,
                device,
                prompt_tokens,
                output_tokens,
                truncated,
            } => {
                self.loaded = Some((model.to_path_buf(), use_gpu));
                Ok(Generated {
                    text,
                    device,
                    prompt_tokens,
                    output_tokens,
                    truncated,
                })
            }
            other => Err(unexpected(&other)),
        }
    }

    fn is_loaded(&self, model: &Path, use_gpu: bool) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|(path, gpu)| path == model && *gpu == use_gpu)
    }

    /// 요청 한 줄을 보내고 답 한 줄을 기다립니다. 실행기가 알려 온 실패는 [`LocalError::Failed`]입니다.
    fn exchange(
        &mut self,
        request: &Request,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Reply, LocalError> {
        if cancel.is_cancelled() {
            return Err(LocalError::Cancelled);
        }
        // 실패한 요청 뒤에는 실행기가 무엇을 올려 두었는지 알 수 없으므로 잊습니다.
        self.loaded = None;
        let mut line =
            serde_json::to_string(request).map_err(|e| LocalError::Crashed(e.to_string()))?;
        line.push('\n');
        if let Err(error) = self
            .stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.flush())
        {
            return Err(LocalError::Crashed(format!(
                "could not send the request: {error}"
            )));
        }

        let started = Instant::now();
        let raw = loop {
            if cancel.is_cancelled() {
                return Err(LocalError::Cancelled);
            }
            match self.replies.recv_timeout(POLL_INTERVAL) {
                Ok(raw) => break raw,
                Err(RecvTimeoutError::Timeout) if started.elapsed() >= timeout => {
                    return Err(LocalError::Timeout(timeout));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let status = self
                        .child
                        .try_wait()
                        .ok()
                        .flatten()
                        .map_or_else(|| "output closed".to_owned(), |s| s.to_string());
                    return Err(LocalError::Crashed(status));
                }
            }
        };
        // 답에는 모델이 만든 글이 들어 있으므로 오류(로그에 남음)에는 답을 싣지 않습니다.
        let reply: Reply = serde_json::from_str(&raw).map_err(|e| {
            LocalError::Crashed(format!(
                "invalid reply ({}; {} bytes)",
                crate::json_error_summary(&e),
                raw.len()
            ))
        })?;
        match reply {
            Reply::Failed { error } => Err(LocalError::Failed(error)),
            other => Ok(other),
        }
    }
}

/// 요청과 맞지 않는 답. 답의 종류만 적습니다 (`Done`에는 모델이 만든 글이 있음).
fn unexpected(reply: &Reply) -> LocalError {
    let kind = match reply {
        Reply::Ready { .. } => "ready",
        Reply::Done { .. } => "done",
        Reply::Failed { .. } => "failed",
    };
    LocalError::Crashed(format!("unexpected '{kind}' reply"))
}

impl Drop for LocalRunner {
    fn drop(&mut self) {
        // 표준 입력을 닫으면 쉬고 있던 실행기는 스스로 끝나지만, 글을 만드는 중일 수도 있으니 바로 끝냅니다.
        let _ = self.child.kill();
        let _ = self.child.wait();
        tracing::info!("local model runner stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_lines_round_trip() {
        let request = Request {
            model: PathBuf::from("C:/models/a.gguf"),
            use_gpu: true,
            prompt: None,
        };
        let line = serde_json::to_string(&request).unwrap();
        assert!(!line.contains('\n'), "요청은 한 줄이어야 합니다");
        assert!(!line.contains("prompt"), "프롬프트가 없으면 쓰지 않습니다");
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), request);

        let replies = [
            Reply::Ready {
                device: "CPU".into(),
            },
            Reply::Done {
                text: "줄\n바꿈".into(),
                device: "CPU".into(),
                prompt_tokens: 10,
                output_tokens: 3,
                truncated: false,
            },
            Reply::Failed {
                error: Failure::PromptTooLong(9000),
            },
        ];
        for reply in replies {
            let line = serde_json::to_string(&reply).unwrap();
            assert!(!line.contains('\n'), "답은 한 줄이어야 합니다: {line}");
            assert_eq!(serde_json::from_str::<Reply>(&line).unwrap(), reply);
        }
        assert_eq!(
            serde_json::to_string(&Reply::Failed {
                error: Failure::Load("bad".into())
            })
            .unwrap(),
            r#"{"status":"failed","error":{"kind":"load","message":"bad"}}"#
        );
    }

    #[test]
    fn missing_runner_is_reported_before_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("no-runner.exe");
        assert!(matches!(
            LocalRunner::spawn(&exe),
            Err(LocalError::RunnerMissing(path)) if path == exe
        ));
    }

    #[test]
    fn runner_sits_next_to_the_current_executable() {
        let path = default_runner_path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(RUNNER_NAME), "{name}");
        assert_eq!(
            path.parent(),
            std::env::current_exe().unwrap().parent(),
            "앱 실행 파일과 같은 폴더여야 합니다"
        );
    }
}
