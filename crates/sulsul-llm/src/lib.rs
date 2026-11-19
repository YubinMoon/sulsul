//! 글 다듬기(LLM 후처리): 모델 카탈로그, 외부 제공자, 다듬기 실행.
//!
//! - [`catalog`]: 이 PC에 내려받아 쓰는 로컬 LLM(GGUF) 목록
//! - [`presets`]: OpenAI 호환 `/chat/completions`를 제공하는 외부 제공자 프리셋
//! - [`client`]: 외부 제공자 요청 (연결 테스트, 다듬기)
//! - [`codex`]: Codex 로그인(ChatGPT 계정)으로 다듬기. 브라우저 로그인, 로그인 정보 저장, 요청
//! - [`claude_cli`]: 이 PC에 설치된 Claude Code CLI(`claude -p`)로 다듬기
//! - [`job`]: 연결 테스트·로그인처럼 작업 스레드에서 도는 일의 손잡이
//! - [`entries`]: "사용 가능 모델" 목록과 고르기·지우기 규칙
//! - [`chat`]: 다듬기 요청의 모양(로컬 모델 채팅 형식)과 응답 정리
//! - [`runner`]: 로컬 모델 실행기(`sulsul-llm-runner`) 프로세스와 주고받는 말
//! - [`refine`]: 설정에서 고른 모델로 받아쓴 글을 다듬는 [`refine::Refiner`]
//!
//! 화면 문구는 갖지 않습니다. 이름·설명은 번역 키(`*_key`)로 내보내고, 로그와 오류 메시지는 영어입니다.
//! llama.cpp는 이 크레이트에 넣지 않습니다(음성 인식의 whisper.cpp와 ggml 기호가 겹침). 로컬 추론은
//! 따로 빌드하는 `sulsul-llm-runner` 실행 파일이 맡습니다.

pub mod catalog;
pub mod chat;
pub mod claude_cli;
pub mod client;
pub mod codex;
pub mod entries;
pub mod job;
pub mod presets;
pub mod refine;
pub mod runner;
#[cfg(test)]
mod test_server;

/// 이 PC에 내려받은 모델로 다듬는 후처리기 ID (`[postprocess] processor`)
pub const LOCAL_PROCESSOR_ID: &str = "llm_local";

/// 외부 제공자로 다듬는 후처리기 ID (`[postprocess] processor`)
pub const CLOUD_PROCESSOR_ID: &str = "llm_cloud";

/// Codex 로그인(ChatGPT 계정)으로 다듬는 후처리기 ID (`[postprocess] processor`)
pub const CODEX_PROCESSOR_ID: &str = "llm_codex";

/// 이 PC의 Claude Code CLI로 다듬는 후처리기 ID (`[postprocess] processor`)
pub const CLAUDE_CLI_PROCESSOR_ID: &str = "llm_claude_cli";

/// `[postprocess.processors.llm_local]`에서 고른 로컬 모델(변형 ID)을 담는 키
pub const LOCAL_MODEL_KEY: &str = "model";

/// JSON 오류를 받은 값 없이 종류와 위치만으로 줄입니다.
///
/// serde_json의 형식 오류(`invalid type: string "..."`)는 받은 값을 그대로 담습니다. 다듬기 요청·응답에는
/// 받아쓴 글과 다듬은 글이 들어 있어, 그 오류를 그대로 로그에 남기면 글 본문도 함께 남습니다.
pub fn json_error_summary(error: &serde_json::Error) -> String {
    format!(
        "{:?} error at line {} column {}",
        error.classify(),
        error.line(),
        error.column()
    )
}
