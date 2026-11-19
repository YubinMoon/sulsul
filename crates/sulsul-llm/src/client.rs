//! 외부 제공자 요청 (OpenAI 호환 `POST /chat/completions`): 연결 테스트와 글 다듬기.
//!
//! 연결 테스트([`test_connection`])는 아주 짧은 요청 하나로 주소·API 키·모델 이름이 맞는지 확인합니다.
//! 비용이 거의 들지 않게 짧은 질문 하나만 보냅니다. 글 다듬기([`chat`])는 지시문을 system 메시지로,
//! 받아쓴 글을 user 메시지로 보냅니다. 두 요청 모두 제공자·모델마다 거부하는 매개변수(`max_tokens`,
//! `temperature` 등)는 넣지 않습니다.
//!
//! 예외는 프로필에서 고른 추론 단계([`ReasoningEffort`])입니다. 제공자 기본값이면 아무것도 넣지 않고,
//! 고른 단계는 제공자가 받는 방법([`ReasoningStyle`])으로 넣습니다. 받는 단계가 모델마다 달라, 400·422로
//! 거절되면 추론 단계를 빼고 한 번 더 보냅니다. 글 다듬기는 그 답을 쓰고, 연결 테스트는 그 답이 오면
//! 고른 단계가 원인이라고 알립니다([`LlmError::ReasoningRejected`]).

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use sulsul_core::config::{ApiProfile, ReasoningEffort};
use sulsul_core::{AppPaths, CancelToken};

use crate::job::Job;
use crate::presets::{
    MOCK_PROVIDER_ID, ReasoningStyle, chat_completions_url, find_preset, reasoning_support,
    requires_api_key,
};
use crate::{claude_cli, codex};

/// 외부 서버 연결 테스트의 기본 제한 시간 (생각하는 모델도 짧은 답은 이 안에 끝납니다)
pub const REMOTE_TIMEOUT: Duration = Duration::from_secs(60);

/// 이 PC 서버(Ollama·LM Studio 등)의 제한 시간. 첫 요청 때 모델을 메모리에 올리느라 오래 걸립니다.
pub const LOCAL_TIMEOUT: Duration = Duration::from_secs(120);

/// 연결 테스트로 보내는 질문
pub(crate) const TEST_PROMPT: &str = "Reply with the single word OK.";

/// 가짜 제공자가 답하기까지 기다리는 시간
const MOCK_DELAY: Duration = Duration::from_millis(800);

/// 가짜 제공자에 이 키를 넣으면 인증 실패를 흉내 냅니다.
const MOCK_FAIL_KEY: &str = "fail";

/// 취소·제한 시간을 확인하는 간격
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// 오류 본문을 메시지에 옮길 때의 최대 글자 수
const MAX_ERROR_CHARS: usize = 300;

/// 오류 설명에서 가릴 API 키의 최소 글자 수
const MIN_SECRET_CHARS: usize = 8;

/// 제공자 응답 (연결 테스트와 글 다듬기 공용)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionReply {
    /// 응답 본문 텍스트 (생각만 하고 끝난 모델 등은 비어 있을 수 있음)
    pub text: String,
    /// 요청을 보내고 응답을 받기까지 걸린 시간
    pub elapsed: Duration,
    /// 응답에 적힌 모델 이름
    pub model: Option<String>,
}

/// 글 다듬기 응답 ([`ConnectionReply`]와 같음)
pub type ChatReply = ConnectionReply;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("API key is required for this provider")]
    MissingApiKey,
    #[error("{}", http_error_text(*status, message))]
    Http {
        status: u16,
        /// 응답 본문에서 뽑은 오류 설명 (없으면 빈 문자열)
        message: String,
    },
    /// 고른 추론 단계를 넣은 요청은 거절되고, 빼고 보낸 요청은 통했습니다 (연결 테스트에서만 남).
    #[error("the model rejected the reasoning effort ({})", http_error_text(*status, message))]
    ReasoningRejected {
        status: u16,
        /// 거절한 응답 본문에서 뽑은 오류 설명 (없으면 빈 문자열)
        message: String,
    },
    /// 로그인 정보가 없거나 만료됐습니다. 다시 로그인해야 합니다
    /// (Codex: 다시 받을 수 없는 로그인, Claude Code CLI: CLI가 로그인하지 않음).
    #[error("not signed in; sign in again")]
    SignedOut,
    /// 제공자가 고른 모델을 쓸 수 없다고 답했습니다 (Codex: 이 계정에서 쓸 수 없는 모델).
    #[error("model is not available: {0}")]
    ModelNotFound(String),
    /// Claude Code CLI(`claude`)를 찾지 못했습니다.
    #[error("Claude Code CLI not found")]
    CliNotFound,
    /// Claude Code CLI를 띄우지 못했거나 결과 없이 오류로 끝났습니다.
    #[error("Claude Code CLI failed: {0}")]
    CliFailed(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    BadResponse(String),
    #[error("cancelled")]
    Cancelled,
}

fn http_error_text(status: u16, message: &str) -> String {
    if message.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {message}")
    }
}

/// 프로필에 맞는 연결 테스트 제한 시간. 이 PC에서 도는 서버는 모델을 올리는 시간까지 넉넉히 줍니다.
pub fn default_timeout(profile: &ApiProfile) -> Duration {
    let local_preset = find_preset(&profile.provider).is_some_and(|preset| preset.runs_locally);
    if local_preset || is_loopback_endpoint(&profile.endpoint) {
        LOCAL_TIMEOUT
    } else {
        REMOTE_TIMEOUT
    }
}

fn is_loopback_endpoint(endpoint: &str) -> bool {
    let rest = endpoint.trim();
    let rest = rest
        .strip_prefix("http://")
        .or_else(|| rest.strip_prefix("https://"))
        .unwrap_or(rest);
    let authority = rest.split('/').next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    host.eq_ignore_ascii_case("localhost") || host == "::1" || host.starts_with("127.")
}

/// 키·주소·모델 이름이 맞는지 아주 짧은 요청 하나로 확인합니다. 블로킹 함수입니다.
///
/// `timeout`이 지나거나 `cancel`이 켜지면 요청이 끝나기를 기다리지 않고 바로 돌아옵니다.
pub fn test_connection(
    profile: &ApiProfile,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<ConnectionReply, LlmError> {
    let messages = json!([{ "role": "user", "content": TEST_PROMPT }]);
    send(
        profile,
        messages,
        || "OK".to_owned(),
        WhenRejected::Report,
        timeout,
        cancel,
    )
}

/// 받아쓴 글을 지시문에 따라 다듬어 달라고 요청합니다. 블로킹 함수입니다.
///
/// `instructions`는 system 메시지로, `text`는 user 메시지로 보냅니다. 응답 텍스트는 손대지 않고
/// 돌려주므로 생각 블록 등은 [`crate::chat::clean_output`]으로 정리해 씁니다.
/// 가짜 제공자는 받은 글 앞에 `[mock] `을 붙여 돌려줍니다.
pub fn chat(
    profile: &ApiProfile,
    instructions: &str,
    text: &str,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<ChatReply, LlmError> {
    let messages = json!([
        { "role": "system", "content": instructions.trim() },
        { "role": "user", "content": text.trim() },
    ]);
    let mock_text = format!("[mock] {}", text.trim());
    send(
        profile,
        messages,
        move || mock_text,
        WhenRejected::SendWithout,
        timeout,
        cancel,
    )
}

/// 고른 추론 단계를 제공자가 거절했을 때 할 일
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WhenRejected {
    /// 추론 단계를 빼고 다시 보내 그 답을 씁니다 (글 다듬기).
    SendWithout,
    /// 추론 단계를 빼고 다시 보내 보고, 통하면 [`LlmError::ReasoningRejected`]로 알립니다 (연결 테스트).
    Report,
}

/// 추론 단계를 넣은 요청을 거절할 때 제공자가 쓰는 상태 코드 (잘못된 값, 모르는 매개변수)
pub(crate) fn is_rejection(status: u16) -> bool {
    matches!(status, 400 | 422)
}

/// 고른 추론 단계를 요청 본문에 넣을 필드. 제공자 기본값이거나 바꿀 수 없는 제공자면 비어 있습니다.
fn reasoning_fields(profile: &ApiProfile) -> Map<String, Value> {
    let mut fields = Map::new();
    let effort = profile.reasoning_effort;
    let Some(level) = effort_value(effort) else {
        return fields;
    };
    match (reasoning_support(&profile.provider).style, effort) {
        (ReasoningStyle::Ignored, _) => {}
        (ReasoningStyle::ThinkingType, ReasoningEffort::None) => {
            fields.insert("thinking".into(), json!({ "type": "disabled" }));
        }
        (ReasoningStyle::ReasoningEnabled, ReasoningEffort::None) => {
            fields.insert("reasoning".into(), json!({ "enabled": false }));
        }
        _ => {
            fields.insert("reasoning_effort".into(), json!(level));
        }
    }
    fields
}

/// `reasoning_effort`에 넣는 단계 이름. 제공자 기본값이면 `None`입니다.
pub(crate) fn effort_value(effort: ReasoningEffort) -> Option<&'static str> {
    match effort {
        ReasoningEffort::Default => None,
        ReasoningEffort::None => Some("none"),
        ReasoningEffort::Low => Some("low"),
        ReasoningEffort::Medium => Some("medium"),
        ReasoningEffort::High => Some("high"),
    }
}

/// `messages`로 채팅 요청을 보냅니다. 가짜 제공자는 네트워크 없이 `mock_text`로 답합니다.
///
/// 고른 추론 단계를 거절당해 다시 보낼 때도 `timeout`은 처음 보낸 때부터 셉니다.
fn send(
    profile: &ApiProfile,
    messages: Value,
    mock_text: impl FnOnce() -> String,
    when_rejected: WhenRejected,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<ConnectionReply, LlmError> {
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    if profile.provider == MOCK_PROVIDER_ID {
        return mock_reply(profile, mock_text, cancel);
    }

    let url = chat_completions_url(&profile.endpoint).map_err(LlmError::InvalidEndpoint)?;
    let api_key = profile.api_key.trim().to_owned();
    if api_key.is_empty() && requires_api_key(&profile.provider) {
        return Err(LlmError::MissingApiKey);
    }
    let mut body = Map::new();
    body.insert("model".into(), json!(profile.model.trim()));
    body.insert("messages".into(), messages);
    let reasoning = reasoning_fields(profile);
    tracing::debug!(
        provider = %profile.provider,
        model = %profile.model.trim(),
        reasoning_effort = ?profile.reasoning_effort,
        %url,
        "sending LLM chat request"
    );

    let started = Instant::now();
    if reasoning.is_empty() {
        return post(
            &url,
            &api_key,
            Value::Object(body),
            started,
            timeout,
            cancel,
        );
    }
    let mut with_reasoning = body.clone();
    with_reasoning.extend(reasoning);
    let (status, message) = match post(
        &url,
        &api_key,
        Value::Object(with_reasoning),
        started,
        timeout,
        cancel,
    ) {
        Err(LlmError::Http { status, message }) if is_rejection(status) => (status, message),
        other => return other,
    };
    tracing::warn!(
        provider = %profile.provider,
        model = %profile.model.trim(),
        reasoning_effort = ?profile.reasoning_effort,
        "provider rejected the request with the reasoning effort (HTTP {status}: {message}); \
         sending again without it"
    );
    let reply = post(
        &url,
        &api_key,
        Value::Object(body),
        started,
        timeout,
        cancel,
    )?;
    match when_rejected {
        WhenRejected::SendWithout => Ok(reply),
        WhenRejected::Report => Err(LlmError::ReasoningRejected { status, message }),
    }
}

/// 요청 하나를 보내고 답을 기다립니다. `started`부터 `timeout`이 지나거나 `cancel`이 켜지면 바로 돌아옵니다.
fn post(
    url: &str,
    api_key: &str,
    body: Value,
    started: Instant,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<ConnectionReply, LlmError> {
    let (url, api_key, body) = (url.to_owned(), api_key.to_owned(), body.to_string());
    run_with_deadline(started, timeout, cancel, move |remaining| {
        send_chat_request(&url, &api_key, &body, remaining)
            .inspect_err(|e| tracing::debug!("LLM chat request failed: {e}"))
    })
    .map(|(text, model)| ConnectionReply {
        text,
        elapsed: started.elapsed(),
        model,
    })
}

/// `job`을 따로 돌리고 답을 기다립니다. `started`부터 `timeout`이 지나거나 `cancel`이 켜지면
/// `job`이 끝나기를 기다리지 않고 바로 돌아옵니다.
///
/// ureq 요청은 중간에 끊을 수 없으므로 따로 돌리고, 여기서는 취소·제한 시간만 지켜봅니다.
/// `job`은 남은 시간을 받아 요청의 제한 시간으로 씁니다.
pub(crate) fn run_with_deadline<T: Send + 'static>(
    started: Instant,
    timeout: Duration,
    cancel: &CancelToken,
    job: impl FnOnce(Duration) -> Result<T, LlmError> + Send + 'static,
) -> Result<T, LlmError> {
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    let remaining = timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(timeout_error(timeout));
    }
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("sulsul-llm-request".into())
        .spawn(move || {
            let _ = tx.send(job(remaining));
        })
        .map_err(|e| LlmError::Network(format!("failed to start request thread: {e}")))?;

    loop {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) if started.elapsed() >= timeout => {
                return Err(timeout_error(timeout));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(LlmError::Network(
                    "request thread stopped unexpectedly".into(),
                ));
            }
        }
    }
}

pub(crate) fn timeout_error(timeout: Duration) -> LlmError {
    LlmError::Network(format!(
        "no response within {} seconds",
        timeout.as_secs().max(1)
    ))
}

/// 가짜 제공자: 네트워크 없이 잠깐 기다린 뒤 `text`로 답합니다.
fn mock_reply(
    profile: &ApiProfile,
    text: impl FnOnce() -> String,
    cancel: &CancelToken,
) -> Result<ConnectionReply, LlmError> {
    let started = Instant::now();
    while started.elapsed() < MOCK_DELAY {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        thread::sleep(POLL_INTERVAL);
    }
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    if profile.api_key.trim() == MOCK_FAIL_KEY {
        return Err(LlmError::Http {
            status: 401,
            message: "mock: invalid API key".into(),
        });
    }
    Ok(ConnectionReply {
        text: text(),
        elapsed: started.elapsed(),
        model: Some(profile.model.clone()),
    })
}

/// 요청을 보내고 (응답 텍스트, 응답에 적힌 모델 이름)을 돌려줍니다.
fn send_chat_request(
    url: &str,
    api_key: &str,
    body: &str,
    timeout: Duration,
) -> Result<(String, Option<String>), LlmError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .into();
    let mut request = agent
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json");
    if !api_key.is_empty() {
        request = request.header("Authorization", &format!("Bearer {api_key}"));
    }
    let mut response = request.send(body.as_bytes()).map_err(network_error)?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| LlmError::Network(format!("failed to read response body: {e}")))?;
    if !(200..300).contains(&status) {
        return Err(LlmError::Http {
            status,
            message: hide_secret(error_message(&text), api_key),
        });
    }
    parse_chat_response(&text)
}

/// 서버가 오류 설명에 API 키를 그대로 적어 보내도 로그에 남지 않게 가립니다.
pub(crate) fn hide_secret(message: String, secret: &str) -> String {
    // 아주 짧은 값은 키가 아닐 수 있고, 가리면 설명의 보통 글자까지 지워집니다.
    if secret.chars().count() < MIN_SECRET_CHARS || !message.contains(secret) {
        return message;
    }
    message.replace(secret, "***")
}

pub(crate) fn network_error(error: ureq::Error) -> LlmError {
    match error {
        ureq::Error::Timeout(_) => LlmError::Network("request timed out".into()),
        other => LlmError::Network(other.to_string()),
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Option<Vec<ChatChoice>>,
}

#[derive(Deserialize)]
struct ChatChoice {
    #[serde(default)]
    message: Option<ChatMessage>,
}

#[derive(Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: Value,
}

/// 성공 응답에서 텍스트와 모델 이름을 꺼냅니다.
fn parse_chat_response(raw: &str) -> Result<(String, Option<String>), LlmError> {
    // 성공 응답의 본문에는 다듬은 글이 들어 있을 수 있어, 오류(로그에 남음)에는 본문을 싣지 않습니다.
    let parsed: ChatResponse = serde_json::from_str(raw).map_err(|e| {
        LlmError::BadResponse(format!(
            "invalid JSON ({}; {} bytes)",
            crate::json_error_summary(&e),
            raw.len()
        ))
    })?;
    let Some(choices) = parsed.choices else {
        // 서버가 적은 오류 설명만 씁니다 (본문 앞부분으로 대신하지 않음).
        let detail = serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|value| json_error_message(&value))
            .map(|message| truncate(&message));
        return Err(LlmError::BadResponse(
            detail.unwrap_or_else(|| "response has no choices".into()),
        ));
    };
    let text = choices
        .first()
        .and_then(|choice| choice.message.as_ref())
        .map(|message| content_text(&message.content))
        .unwrap_or_default();
    let model = parsed
        .model
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty());
    Ok((text, model))
}

/// `content`는 문자열, `null`, 또는 `[{ "type": "text", "text": ... }]` 조각 목록일 수 있습니다.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.trim().to_owned(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match part {
                Value::String(text) => Some(text.as_str()),
                Value::Object(map) => map.get("text").and_then(Value::as_str),
                _ => None,
            })
            .collect::<String>()
            .trim()
            .to_owned(),
        _ => String::new(),
    }
}

/// 오류 응답 본문에서 사람이 읽을 설명을 뽑습니다. 제공자마다 모양이 달라 여러 형태를 봅니다.
///
/// - `{"error": {"message": "..."}}` (OpenAI 등 대부분)
/// - `[{"error": {"message": "..."}}]` (Gemini)
/// - `{"error": "..."}` (xAI), `{"message": "..."}` (Cerebras), `{"detail": "..."}` (Mistral)
///
/// JSON이 아니면 본문 앞부분을, 본문이 비어 있으면 빈 문자열을 돌려줍니다.
pub(crate) fn error_message(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => json_error_message(&value).map_or_else(|| truncate(trimmed), |m| truncate(&m)),
        Err(_) => truncate(trimmed),
    }
}

fn json_error_message(value: &Value) -> Option<String> {
    let non_empty = |text: &str| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    };
    match value {
        Value::Array(items) => items.iter().find_map(json_error_message),
        Value::Object(map) => {
            if let Some(error) = map.get("error") {
                match error {
                    Value::String(text) => return non_empty(text),
                    Value::Object(inner) => {
                        if let Some(text) = inner.get("message").and_then(Value::as_str) {
                            return non_empty(text);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(text) = map.get("message").and_then(Value::as_str) {
                return non_empty(text);
            }
            match map.get("detail") {
                Some(Value::String(text)) => non_empty(text),
                // FastAPI 모양: {"detail": [{"msg": "..."}]}
                Some(Value::Array(items)) => items
                    .iter()
                    .find_map(|item| item.get("msg").and_then(Value::as_str).and_then(non_empty)),
                _ => None,
            }
        }
        _ => None,
    }
}

pub(crate) fn truncate(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.chars().count() <= MAX_ERROR_CHARS {
        return trimmed.to_owned();
    }
    let prefix: String = trimmed.chars().take(MAX_ERROR_CHARS).collect();
    format!("{prefix}...")
}

/// 작업 스레드에서 연결 테스트를 돌리는 손잡이. 화면 스레드는 [`poll`](Self::poll)만 부릅니다.
///
/// 외부 제공자([`test_connection`]), Codex 로그인([`codex::client::test_connection`]),
/// Claude Code CLI([`claude_cli::test_connection`])를 테스트합니다.
/// 버리면 테스트를 취소합니다.
pub struct ConnectionTest {
    /// 테스트하는 외부 제공자 (Codex면 `None`)
    profile: Option<ApiProfile>,
    job: Job<Result<ConnectionReply, LlmError>>,
}

impl ConnectionTest {
    /// 외부 제공자 연결 테스트를 작업 스레드에서 시작합니다. 제한 시간은 [`default_timeout`]을 따릅니다.
    ///
    /// `on_finish`는 끝났을 때 작업 스레드에서 한 번 불립니다 (앱은 egui repaint 요청을 넘김).
    pub fn start(
        profile: ApiProfile,
        on_finish: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let timeout = default_timeout(&profile);
        let worker_profile = profile.clone();
        let job = Job::start(
            "sulsul-llm-connection-test",
            Err(LlmError::Cancelled),
            on_finish,
            move |cancel| test_connection(&worker_profile, timeout, cancel),
        )?;
        Ok(Self {
            profile: Some(profile),
            job,
        })
    }

    /// Codex 로그인으로 `model`에 짧은 요청을 보내는 테스트를 작업 스레드에서 시작합니다.
    pub fn start_codex(
        paths: AppPaths,
        model: String,
        effort: ReasoningEffort,
        on_finish: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let job = Job::start(
            "sulsul-llm-connection-test",
            Err(LlmError::Cancelled),
            on_finish,
            move |cancel| {
                codex::client::test_connection(&paths, &model, effort, REMOTE_TIMEOUT, cancel)
            },
        )?;
        Ok(Self { profile: None, job })
    }

    /// 이 PC의 Claude Code CLI로 `model`에 짧은 요청을 보내는 테스트를 작업 스레드에서 시작합니다.
    pub fn start_claude_cli(
        paths: AppPaths,
        cli: std::path::PathBuf,
        model: String,
        effort: ReasoningEffort,
        on_finish: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let job = Job::start(
            "sulsul-llm-connection-test",
            Err(LlmError::Cancelled),
            on_finish,
            move |cancel| {
                let target = claude_cli::CliTarget {
                    cli: &cli,
                    model: &model,
                    effort,
                };
                claude_cli::test_connection(&paths, target, claude_cli::REQUEST_TIMEOUT, cancel)
            },
        )?;
        Ok(Self { profile: None, job })
    }

    /// 테스트하는 외부 제공자 (시작할 때의 값). Codex·Claude Code CLI 테스트면 `None`입니다.
    pub fn profile(&self) -> Option<&ApiProfile> {
        self.profile.as_ref()
    }

    /// 시작한 뒤 지난 시간. 끝났으면 끝날 때까지 걸린 시간입니다.
    pub fn elapsed(&self) -> Duration {
        self.job.elapsed()
    }

    /// 끝났으면 결과를 돌려줍니다 (부를 때마다 같은 결과).
    /// [`cancel`](Self::cancel) 뒤에는 요청이 아직 안 끝났어도 바로 `Some(Err(Cancelled))`입니다.
    pub fn poll(&self) -> Option<Result<ConnectionReply, LlmError>> {
        self.job.poll()
    }

    /// 테스트를 멈춥니다. 이미 끝났으면 결과를 그대로 둡니다.
    pub fn cancel(&self) {
        self.job.cancel();
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::presets::profile_from_preset;

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let mut expected_total: Option<usize> = None;
        loop {
            let n = stream.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if expected_total.is_none()
                && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
            {
                let headers = String::from_utf8_lossy(&buf[..pos]);
                let content_len = headers
                    .lines()
                    .find_map(|line| {
                        let (k, v) = line.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                expected_total = Some(pos + 4 + content_len);
            }
            if let Some(total) = expected_total
                && buf.len() >= total
            {
                break;
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn respond(stream: &mut TcpStream, status: &str, body: &str) {
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes()).unwrap();
    }

    /// 요청 하나를 받아 `status`·`body`로 답하는 가짜 서버. 받은 요청 원문을 돌려줍니다.
    fn serve_once(
        status: &'static str,
        body: &'static str,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let raw = read_http_request(&mut stream);
            respond(&mut stream, status, body);
            raw
        });
        (format!("http://{addr}/v1"), server)
    }

    fn profile(provider: &str, endpoint: &str, key: &str, model: &str) -> ApiProfile {
        ApiProfile {
            id: "p".into(),
            name: "테스트".into(),
            provider: provider.into(),
            endpoint: endpoint.into(),
            api_key: key.into(),
            model: model.into(),
            ..ApiProfile::default()
        }
    }

    fn quick() -> Duration {
        Duration::from_secs(10)
    }

    #[test]
    fn sends_minimal_chat_request_and_parses_reply() {
        let (endpoint, server) = serve_once(
            "200 OK",
            r#"{"id":"c1","model":"gpt-6-luna-2026","choices":[{"index":0,"message":{"role":"assistant","content":" OK "}}]}"#,
        );
        let reply = test_connection(
            &profile("openai", &endpoint, " sk-test-123 ", "gpt-6-luna"),
            quick(),
            &CancelToken::new(),
        )
        .unwrap();
        let raw = server.join().unwrap();

        assert!(
            raw.starts_with("POST /v1/chat/completions HTTP/1.1"),
            "요청 주소가 /v1/chat/completions여야 합니다:\n{raw}"
        );
        let lower = raw.to_ascii_lowercase();
        assert!(lower.contains("authorization: bearer sk-test-123\r\n"));
        assert!(lower.contains("content-type: application/json"));
        let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let mut fields: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
        fields.sort();
        assert_eq!(
            fields,
            ["messages", "model"],
            "모델이 거부할 수 있는 매개변수(max_tokens 등)를 넣으면 안 됩니다"
        );
        assert_eq!(body["model"], "gpt-6-luna");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);

        assert_eq!(reply.text, "OK");
        assert_eq!(reply.model.as_deref(), Some("gpt-6-luna-2026"));
    }

    #[test]
    fn chat_sends_instructions_as_system_and_text_as_user() {
        let (endpoint, server) = serve_once(
            "200 OK",
            r#"{"model":"gpt-6-luna","choices":[{"message":{"content":"안녕하세요."}}]}"#,
        );
        let reply = chat(
            &profile("openai", &endpoint, "sk-test", "gpt-6-luna"),
            "  맞춤법을 고쳐 주세요.\n",
            " 안녕 하세요 ",
            quick(),
            &CancelToken::new(),
        )
        .unwrap();
        let raw = server.join().unwrap();
        let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let mut fields: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
        fields.sort();
        assert_eq!(
            fields,
            ["messages", "model"],
            "모델이 거부할 수 있는 매개변수를 넣으면 안 됩니다"
        );
        assert_eq!(
            body["messages"],
            serde_json::json!([
                { "role": "system", "content": "맞춤법을 고쳐 주세요." },
                { "role": "user", "content": "안녕 하세요" },
            ])
        );
        assert_eq!(reply.text, "안녕하세요.");
    }

    /// 요청을 차례로 받아 `responses`의 (상태, 본문)으로 하나씩 답하는 가짜 서버. 받은 요청 원문들을 돌려줍니다.
    fn serve_sequence(
        responses: &'static [(&'static str, &'static str)],
    ) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            responses
                .iter()
                .map(|(status, body)| {
                    let (mut stream, _) = listener.accept().unwrap();
                    let raw = read_http_request(&mut stream);
                    respond(&mut stream, status, body);
                    raw
                })
                .collect()
        });
        (format!("http://{addr}/v1"), server)
    }

    fn body_of(raw: &str) -> Value {
        serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }

    fn with_effort(provider: &str, effort: ReasoningEffort) -> ApiProfile {
        ApiProfile {
            reasoning_effort: effort,
            ..profile(provider, "http://127.0.0.1:1/v1", "sk-test", "m")
        }
    }

    const OK_REPLY: &str = r#"{"model":"m","choices":[{"message":{"content":"고친 글"}}]}"#;
    const REJECTED: &str = r#"{"error":{"message":"Unsupported value: 'reasoning_effort'"}}"#;

    #[test]
    fn reasoning_effort_goes_in_the_way_each_provider_takes_it() {
        use ReasoningEffort::{Default, High, Low, Medium, None};

        let fields = |provider: &str, effort| {
            Value::Object(reasoning_fields(&with_effort(provider, effort)))
        };
        for provider in ["openai", "deepseek", "together", "anthropic", "unknown"] {
            assert_eq!(
                fields(provider, Default),
                json!({}),
                "{provider}: 제공자 기본값이면 아무것도 넣지 않아야 합니다"
            );
        }
        assert_eq!(
            fields("openai", None),
            json!({ "reasoning_effort": "none" })
        );
        assert_eq!(fields("openai", Low), json!({ "reasoning_effort": "low" }));
        assert_eq!(
            fields("groq", Medium),
            json!({ "reasoning_effort": "medium" })
        );
        assert_eq!(
            fields("openai", High),
            json!({ "reasoning_effort": "high" })
        );
        assert_eq!(
            fields("unknown", Low),
            json!({ "reasoning_effort": "low" }),
            "모르는 공급자(직접 입력한 호환 서버)는 OpenAI 방식으로 보냅니다"
        );

        // DeepSeek은 `thinking`으로, Together는 `reasoning.enabled`로 끄고, 단계는 `reasoning_effort`로 고릅니다.
        assert_eq!(
            fields("deepseek", None),
            json!({ "thinking": { "type": "disabled" } })
        );
        assert_eq!(
            fields("deepseek", High),
            json!({ "reasoning_effort": "high" })
        );
        assert_eq!(
            fields("together", None),
            json!({ "reasoning": { "enabled": false } })
        );
        assert_eq!(
            fields("together", Low),
            json!({ "reasoning_effort": "low" })
        );

        // Anthropic 호환 주소는 추론 단계를 무시하므로 파일에 값이 있어도 넣지 않습니다.
        for effort in ReasoningEffort::ALL {
            assert_eq!(fields("anthropic", effort), json!({}), "{effort:?}");
        }
    }

    #[test]
    fn chosen_reasoning_effort_is_sent_with_the_chat() {
        let (endpoint, server) = serve_once("200 OK", OK_REPLY);
        let profile = ApiProfile {
            reasoning_effort: ReasoningEffort::Low,
            ..profile("openai", &endpoint, "sk-test", "gpt-6-luna")
        };
        let reply = chat(&profile, "지시", "글", quick(), &CancelToken::new()).unwrap();
        let body = body_of(&server.join().unwrap());
        let mut fields: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
        fields.sort();
        assert_eq!(fields, ["messages", "model", "reasoning_effort"]);
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(reply.text, "고친 글");
    }

    #[test]
    fn chat_sends_again_without_a_rejected_reasoning_effort() {
        let (endpoint, server) =
            serve_sequence(&[("400 Bad Request", REJECTED), ("200 OK", OK_REPLY)]);
        let profile = ApiProfile {
            reasoning_effort: ReasoningEffort::None,
            ..profile("openai", &endpoint, "sk-test", "gpt-6.1-sol")
        };
        let reply = chat(&profile, "지시", "글", quick(), &CancelToken::new()).unwrap();
        assert_eq!(
            reply.text, "고친 글",
            "추론 단계를 거절당해도 빼고 다시 보내 다듬은 글을 받아야 합니다"
        );

        let requests = server.join().unwrap();
        let (first, second) = (body_of(&requests[0]), body_of(&requests[1]));
        assert_eq!(first["reasoning_effort"], "none");
        assert!(
            second.get("reasoning_effort").is_none(),
            "다시 보낼 때는 추론 단계를 빼야 합니다: {second}"
        );
        assert_eq!(first["messages"], second["messages"]);
    }

    #[test]
    fn connection_test_reports_a_rejected_reasoning_effort() {
        let (endpoint, server) =
            serve_sequence(&[("422 Unprocessable Entity", REJECTED), ("200 OK", OK_REPLY)]);
        let profile = ApiProfile {
            reasoning_effort: ReasoningEffort::High,
            ..profile("mistral", &endpoint, "sk-test", "m")
        };
        let result = test_connection(&profile, quick(), &CancelToken::new());
        server.join().unwrap();
        match result {
            Err(LlmError::ReasoningRejected { status, message }) => {
                assert_eq!(status, 422);
                assert!(message.contains("reasoning_effort"), "{message}");
            }
            other => panic!("고른 추론 단계가 원인이라고 알려야 합니다: {other:?}"),
        }
    }

    #[test]
    fn other_failures_are_reported_as_they_are() {
        // 빼고 보내도 거절되면 추론 단계 탓이 아니므로 두 번째 오류를 알립니다.
        let (endpoint, server) = serve_sequence(&[
            ("400 Bad Request", REJECTED),
            ("400 Bad Request", r#"{"error":{"message":"bad model"}}"#),
        ]);
        let still_rejected = ApiProfile {
            reasoning_effort: ReasoningEffort::Low,
            ..profile("openai", &endpoint, "sk-test", "m")
        };
        let result = test_connection(&still_rejected, quick(), &CancelToken::new());
        assert_eq!(server.join().unwrap().len(), 2);
        assert_eq!(
            result,
            Err(LlmError::Http {
                status: 400,
                message: "bad model".into()
            })
        );

        // 거절이 아닌 오류(인증 등)는 다시 보내지 않습니다. 가짜 서버는 요청 하나만 받습니다.
        let (endpoint, server) =
            serve_once("401 Unauthorized", r#"{"error":{"message":"invalid key"}}"#);
        let wrong_key = ApiProfile {
            reasoning_effort: ReasoningEffort::Low,
            ..profile("openai", &endpoint, "sk-test", "m")
        };
        let result = chat(&wrong_key, "지시", "글", quick(), &CancelToken::new());
        server.join().unwrap();
        assert!(
            matches!(result, Err(LlmError::Http { status: 401, .. })),
            "{result:?}"
        );
    }

    #[test]
    fn mock_provider_echoes_the_text_for_chat() {
        let mock = profile_from_preset(MOCK_PROVIDER_ID, "m".into(), "가짜".into());
        let reply = chat(&mock, "지시", " 받아쓴 글 ", quick(), &CancelToken::new()).unwrap();
        assert_eq!(reply.text, "[mock] 받아쓴 글");
    }

    #[test]
    fn keyless_local_server_gets_no_authorization_header() {
        let (endpoint, server) = serve_once(
            "200 OK",
            r#"{"model":"gemma4:e4b","choices":[{"message":{"content":[{"type":"text","text":"O"},{"type":"text","text":"K"}]}}]}"#,
        );
        let endpoint = endpoint.trim_end_matches("/v1").to_owned();
        let reply = test_connection(
            &profile("ollama", &endpoint, "", "gemma4:e4b"),
            quick(),
            &CancelToken::new(),
        )
        .unwrap();
        let raw = server.join().unwrap();
        assert!(raw.starts_with("POST /v1/chat/completions "), "{raw}");
        assert!(!raw.to_ascii_lowercase().contains("authorization:"));
        assert_eq!(reply.text, "OK", "조각 목록 content도 이어 붙여야 합니다");
    }

    #[test]
    fn empty_content_and_missing_model_are_allowed() {
        let (endpoint, server) = serve_once(
            "200 OK",
            r#"{"choices":[{"message":{"content":null,"reasoning_content":"..."}}]}"#,
        );
        let reply = test_connection(
            &profile("custom", &endpoint, "", "m"),
            quick(),
            &CancelToken::new(),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(reply.text, "");
        assert_eq!(reply.model, None);
    }

    #[test]
    fn http_error_carries_message_from_body() {
        let (endpoint, server) = serve_once(
            "401 Unauthorized",
            r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#,
        );
        let err = test_connection(
            &profile("openai", &endpoint, "bad", "gpt-6-luna"),
            quick(),
            &CancelToken::new(),
        )
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(
            err,
            LlmError::Http {
                status: 401,
                message: "Incorrect API key provided".into()
            }
        );
        assert_eq!(err.to_string(), "HTTP 401: Incorrect API key provided");
    }

    #[test]
    fn non_json_success_is_bad_response() {
        let (endpoint, server) = serve_once("200 OK", "<html>proxy</html>");
        let err = test_connection(
            &profile("custom", &endpoint, "", "m"),
            quick(),
            &CancelToken::new(),
        )
        .unwrap_err();
        server.join().unwrap();
        assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");
    }

    #[test]
    fn bad_success_responses_do_not_copy_the_body_into_errors() {
        // 오류는 로그에 남으므로, 다듬은 글이 든 성공 응답 본문을 옮겨 적으면 안 됩니다.
        let secret = "비밀 받아쓰기";
        for raw in [
            r#"{"model": 5, "choices":[{"message":{"content":"비밀 받아쓰기"}}]}"#,
            r#"{"choices": "비밀 받아쓰기"}"#,
            r#"{"message":{"role":"assistant","content":"비밀 받아쓰기"}}"#,
            "비밀 받아쓰기",
        ] {
            let err = parse_chat_response(raw).unwrap_err();
            assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");
            assert!(!err.to_string().contains(secret), "{raw} → {err}");
        }
        assert_eq!(
            parse_chat_response(r#"{"error":{"message":"overloaded"}}"#).unwrap_err(),
            LlmError::BadResponse("overloaded".into()),
            "서버가 적은 오류 설명은 그대로 씁니다"
        );
    }

    #[test]
    fn error_messages_hide_the_api_key() {
        let key = "sk-test-0123456789";
        assert_eq!(
            hide_secret(format!("Incorrect API key provided: {key}."), key),
            "Incorrect API key provided: ***."
        );
        assert_eq!(
            hide_secret("bad key".into(), "bad"),
            "bad key",
            "짧은 값은 가리지 않습니다"
        );
        assert_eq!(hide_secret("Invalid API Key".into(), ""), "Invalid API Key");
    }

    #[test]
    fn error_message_understands_provider_shapes() {
        let cases = [
            (r#"{"error":{"message":"bad key","code":401}}"#, "bad key"),
            (
                r#"[{"error":{"code":400,"message":"Missing or invalid Authorization header.","status":"INVALID_ARGUMENT"}}]"#,
                "Missing or invalid Authorization header.",
            ),
            (
                r#"{"code":"unauthenticated:no-credentials","error":"No credentials presented."}"#,
                "No credentials presented.",
            ),
            (
                r#"{"message":"Wrong API Key","type":"invalid_request_error"}"#,
                "Wrong API Key",
            ),
            (r#"{"detail":"Invalid API Key"}"#, "Invalid API Key"),
            (
                r#"{"detail":[{"loc":["body"],"msg":"field required"}]}"#,
                "field required",
            ),
            ("upstream connect error", "upstream connect error"),
            ("   ", ""),
        ];
        for (body, expected) in cases {
            assert_eq!(error_message(body), expected, "{body}");
        }
        let long = "x".repeat(MAX_ERROR_CHARS + 50);
        assert!(
            error_message(&long).ends_with("..."),
            "긴 본문은 잘라야 합니다"
        );
        assert_eq!(
            LlmError::Http {
                status: 502,
                message: String::new()
            }
            .to_string(),
            "HTTP 502"
        );
    }

    #[test]
    fn missing_key_and_bad_endpoint_fail_before_sending() {
        let cancel = CancelToken::new();
        let openai = profile_from_preset("openai", "o".into(), "OpenAI".into());
        assert_eq!(
            test_connection(&openai, quick(), &cancel),
            Err(LlmError::MissingApiKey)
        );
        let bad = profile("custom", "ftp://nope", "", "m");
        assert!(matches!(
            test_connection(&bad, quick(), &cancel),
            Err(LlmError::InvalidEndpoint(_))
        ));
    }

    #[test]
    fn cancel_returns_promptly_while_server_is_silent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // 연결은 받지만 답하지 않는 서버
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_secs(3));
            drop(stream);
        });
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            canceller.cancel();
        });
        let started = Instant::now();
        let result = test_connection(
            &profile("custom", &format!("http://{addr}/v1"), "", "m"),
            Duration::from_secs(30),
            &cancel,
        );
        assert_eq!(result, Err(LlmError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "취소하면 응답을 기다리지 않고 바로 돌아와야 합니다"
        );
        server.join().unwrap();
    }

    #[test]
    fn timeout_is_respected_while_server_is_silent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_secs(2));
            drop(stream);
        });
        let started = Instant::now();
        let result = test_connection(
            &profile("custom", &format!("http://{addr}/v1"), "", "m"),
            Duration::from_millis(300),
            &CancelToken::new(),
        );
        assert!(matches!(result, Err(LlmError::Network(_))), "{result:?}");
        assert!(started.elapsed() < Duration::from_millis(1500));
        server.join().unwrap();
    }

    #[test]
    fn default_timeout_is_longer_for_local_servers() {
        let ollama = profile_from_preset("ollama", "o".into(), "O".into());
        assert_eq!(default_timeout(&ollama), LOCAL_TIMEOUT);
        let mut custom = profile_from_preset("custom", "c".into(), "C".into());
        assert_eq!(
            default_timeout(&custom),
            LOCAL_TIMEOUT,
            "127.0.0.1은 이 PC입니다"
        );
        custom.endpoint = "http://[::1]:8080/v1".into();
        assert_eq!(default_timeout(&custom), LOCAL_TIMEOUT);
        custom.endpoint = "https://llm.example.com/v1".into();
        assert_eq!(default_timeout(&custom), REMOTE_TIMEOUT);
        let openai = profile_from_preset("openai", "o".into(), "O".into());
        assert_eq!(default_timeout(&openai), REMOTE_TIMEOUT);
    }

    #[test]
    fn mock_provider_answers_without_network() {
        let mock = profile_from_preset(MOCK_PROVIDER_ID, "m".into(), "가짜".into());
        let started = Instant::now();
        let reply = test_connection(&mock, quick(), &CancelToken::new()).unwrap();
        assert!(
            started.elapsed() >= MOCK_DELAY,
            "가짜 제공자도 잠깐 기다려야 합니다"
        );
        assert_eq!(reply.text, "OK");
        assert_eq!(reply.model.as_deref(), Some("mock-small"));

        let mut failing = mock.clone();
        failing.api_key = MOCK_FAIL_KEY.into();
        assert_eq!(
            test_connection(&failing, quick(), &CancelToken::new()),
            Err(LlmError::Http {
                status: 401,
                message: "mock: invalid API key".into()
            })
        );
    }

    fn wait_for(test: &ConnectionTest) -> Result<ConnectionReply, LlmError> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = test.poll() {
                return result;
            }
            assert!(Instant::now() < deadline, "연결 테스트가 끝나지 않았습니다");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn connection_test_handle_reports_result_once_finished() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let mock = profile_from_preset(MOCK_PROVIDER_ID, "m".into(), "가짜".into());
        let test = ConnectionTest::start(mock.clone(), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        assert_eq!(test.profile(), Some(&mock));
        assert!(test.poll().is_none(), "가짜 제공자는 바로 끝나지 않습니다");

        let reply = wait_for(&test).unwrap();
        assert_eq!(reply.text, "OK");
        assert_eq!(
            test.poll(),
            Some(Ok(reply)),
            "다시 불러도 같은 결과여야 합니다"
        );
        let frozen = test.elapsed();
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            test.elapsed(),
            frozen,
            "끝난 뒤에는 걸린 시간이 멈춰야 합니다"
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        while calls.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "on_finish는 한 번 불려야 합니다"
        );
        // 끝난 뒤의 취소는 결과를 바꾸지 않습니다.
        test.cancel();
        assert!(matches!(test.poll(), Some(Ok(_))));
    }

    #[test]
    fn connection_test_handle_cancels_immediately() {
        let mock = profile_from_preset(MOCK_PROVIDER_ID, "m".into(), "가짜".into());
        let test = ConnectionTest::start(mock, || {}).unwrap();
        test.cancel();
        assert_eq!(
            test.poll(),
            Some(Err(LlmError::Cancelled)),
            "cancel 뒤에는 바로 Cancelled여야 합니다"
        );

        let mut failing = profile_from_preset(MOCK_PROVIDER_ID, "m".into(), "가짜".into());
        failing.api_key = MOCK_FAIL_KEY.into();
        let test = ConnectionTest::start(failing, || {}).unwrap();
        assert!(matches!(
            wait_for(&test),
            Err(LlmError::Http { status: 401, .. })
        ));
    }

    /// 실제 제공자에 요청합니다. `SULSUL_TEST_LLM_PROVIDER`, `SULSUL_TEST_LLM_API_KEY`(필요하면
    /// `SULSUL_TEST_LLM_MODEL`, `SULSUL_TEST_LLM_ENDPOINT`)를 지정하고 `--ignored`로 돌립니다.
    #[test]
    #[ignore = "네트워크와 실제 API 키가 필요합니다"]
    fn real_provider_connection() {
        let provider =
            std::env::var("SULSUL_TEST_LLM_PROVIDER").unwrap_or_else(|_| "openai".into());
        let mut profile = profile_from_preset(&provider, "real".into(), "실제".into());
        profile.api_key = std::env::var("SULSUL_TEST_LLM_API_KEY").unwrap_or_default();
        if let Ok(model) = std::env::var("SULSUL_TEST_LLM_MODEL") {
            profile.model = model;
        }
        if let Ok(endpoint) = std::env::var("SULSUL_TEST_LLM_ENDPOINT") {
            profile.endpoint = endpoint;
        }
        let reply = test_connection(&profile, default_timeout(&profile), &CancelToken::new())
            .unwrap_or_else(|e| panic!("{provider} 연결 테스트 실패: {e}"));
        println!(
            "{provider}: {:?} ({:?}) in {:?}",
            reply.text, reply.model, reply.elapsed
        );
    }
}
