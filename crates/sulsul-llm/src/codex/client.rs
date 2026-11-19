//! Codex 백엔드 요청: 글 다듬기·연결 테스트(`POST /responses`, 스트림)와 모델 목록(`GET /models`).
//!
//! 요청은 Responses 형식입니다. 지시문(고른 프롬프트)은 `instructions`로, 받아쓴 글은 user 메시지로
//! 보냅니다. 이 백엔드는 스트림(`stream: true`)과 `store: false`만 받아, 서버 전송 이벤트(SSE)로 온
//! 글 조각을 모아 답을 만듭니다.
//!
//! access token이 곧 만료되면 먼저 새로 받고, 서버가 401로 거절하면 새로 받아 한 번 더 보냅니다
//! ([`super::auth`]). 고른 추론 단계를 400으로 거절하면 외부 제공자처럼 빼고 한 번 더 보냅니다.

use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use sulsul_core::config::ReasoningEffort;
use sulsul_core::{AppPaths, CancelToken};

use super::auth::{self, Tokens};
use super::http_agent;
use crate::client::{
    ChatReply, ConnectionReply, LlmError, TEST_PROMPT, WhenRejected, effort_value, error_message,
    hide_secret, is_rejection, network_error, run_with_deadline, truncate,
};

/// Codex 백엔드 주소 (Codex CLI가 ChatGPT 계정으로 로그인했을 때 쓰는 곳)
pub const BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// 모델 목록 요청의 제한 시간
pub const MODELS_TIMEOUT: Duration = Duration::from_secs(20);

/// 모델 목록을 받을 때 알리는 Codex 판. 서버는 이 판이 쓸 수 있는 모델만 돌려줍니다.
const MODELS_CLIENT_VERSION: &str = "0.170.0";

/// 요청할 ChatGPT 계정(작업 공간)을 알리는 머리글
const ACCOUNT_HEADER: &str = "chatgpt-account-id";

/// 연결 테스트에서 user 메시지로 보내는 글
const TEST_INPUT: &str = "ping";

/// Codex로 쓸 수 있는 모델 하나
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexModel {
    /// 요청에 넣는 모델 이름 (예: `gpt-6-luna`)
    pub slug: String,
    /// 서버가 알려 준 보여 줄 이름 (없으면 `slug`)
    pub name: String,
    /// 서버가 알려 준 설명 (영어, 없으면 빈 문자열)
    pub description: String,
}

/// 요청을 보낼 곳 (테스트에서 바꿈)
struct Backend {
    base_url: String,
    token_url: String,
}

impl Backend {
    fn standard() -> Self {
        Self {
            base_url: BASE_URL.to_owned(),
            token_url: auth::TOKEN_URL.to_owned(),
        }
    }
}

/// 받아쓴 글을 지시문에 따라 다듬어 달라고 요청합니다. 블로킹 함수입니다.
///
/// `instructions`는 `instructions`로, `text`는 user 메시지로 보냅니다. 응답은 앞뒤 공백만 지워 돌려주므로
/// 생각 블록 등은 [`crate::chat::clean_output`]으로 정리해 씁니다.
pub fn chat(
    paths: &AppPaths,
    model: &str,
    effort: ReasoningEffort,
    instructions: &str,
    text: &str,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<ChatReply, LlmError> {
    let body = request_body(model, instructions.trim(), text.trim());
    let job = Request {
        paths: paths.clone(),
        backend: Backend::standard(),
        body,
        effort,
        when_rejected: WhenRejected::SendWithout,
    };
    job.send(timeout, cancel)
}

/// 로그인과 모델 이름이 맞는지 아주 짧은 요청 하나로 확인합니다. 블로킹 함수입니다.
pub fn test_connection(
    paths: &AppPaths,
    model: &str,
    effort: ReasoningEffort,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<ConnectionReply, LlmError> {
    let job = Request {
        paths: paths.clone(),
        backend: Backend::standard(),
        body: request_body(model, TEST_PROMPT, TEST_INPUT),
        effort,
        when_rejected: WhenRejected::Report,
    };
    job.send(timeout, cancel)
}

/// 로그인한 계정으로 쓸 수 있는 모델 목록(서버가 정한 순서)을 받습니다. 블로킹 함수입니다.
pub fn list_models(
    paths: &AppPaths,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<Vec<CodexModel>, LlmError> {
    fetch_models(paths.clone(), Backend::standard(), timeout, cancel)
}

/// 다듬기·연결 테스트 요청 하나
struct Request {
    paths: AppPaths,
    backend: Backend,
    /// 추론 단계를 뺀 요청 본문
    body: Map<String, Value>,
    effort: ReasoningEffort,
    when_rejected: WhenRejected,
}

impl Request {
    /// `timeout`이 지나거나 `cancel`이 켜지면 요청이 끝나기를 기다리지 않고 바로 돌아옵니다.
    /// 토큰을 새로 받고 다시 보내는 데 드는 시간도 `timeout` 안에 셉니다.
    fn send(self, timeout: Duration, cancel: &CancelToken) -> Result<ConnectionReply, LlmError> {
        let started = Instant::now();
        let model = self
            .body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        tracing::debug!(
            %model,
            reasoning_effort = ?self.effort,
            "sending Codex request"
        );
        run_with_deadline(started, timeout, cancel, move |remaining| {
            self.run(Instant::now() + remaining)
                .inspect_err(|e| tracing::debug!("Codex request failed: {e}"))
        })
        .map(|(text, model)| ConnectionReply {
            text,
            elapsed: started.elapsed(),
            model,
        })
    }

    fn run(&self, deadline: Instant) -> Result<(String, Option<String>), LlmError> {
        let post = |body: &Map<String, Value>| {
            authorized(&self.paths, &self.backend, deadline, |tokens, timeout| {
                post_responses(&self.backend.base_url, tokens, body, timeout)
            })
        };
        let Some(level) = effort_value(self.effort) else {
            return post(&self.body);
        };
        let mut with_reasoning = self.body.clone();
        with_reasoning.insert("reasoning".into(), json!({ "effort": level }));
        let (status, message) = match post(&with_reasoning) {
            Err(LlmError::Http { status, message }) if is_rejection(status) => (status, message),
            other => return other,
        };
        tracing::warn!(
            reasoning_effort = ?self.effort,
            "Codex rejected the request with the reasoning effort (HTTP {status}: {message}); \
             sending again without it"
        );
        let reply = post(&self.body)?;
        match self.when_rejected {
            WhenRejected::SendWithout => Ok(reply),
            WhenRejected::Report => Err(LlmError::ReasoningRejected { status, message }),
        }
    }
}

/// 다듬기 요청 본문 (추론 단계는 따로 넣음). 이 백엔드는 스트림과 `store: false`만 받습니다.
fn request_body(model: &str, instructions: &str, text: &str) -> Map<String, Value> {
    let body = json!({
        "model": model.trim(),
        "instructions": instructions,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": text }],
        }],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "store": false,
        "stream": true,
        "include": [],
    });
    match body {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// 토큰을 붙여 `request`를 보냅니다. access token을 401로 거절당하면 새로 받아 한 번 더 보냅니다.
fn authorized<T>(
    paths: &AppPaths,
    backend: &Backend,
    deadline: Instant,
    request: impl Fn(&Tokens, Duration) -> Result<T, LlmError>,
) -> Result<T, LlmError> {
    let tokens = auth::fresh_tokens(paths, &backend.token_url, left(deadline)?)?;
    match request(&tokens, left(deadline)?) {
        Err(LlmError::Http { status: 401, .. }) => {
            let tokens =
                auth::refresh_rejected(paths, &backend.token_url, &tokens, left(deadline)?)?;
            request(&tokens, left(deadline)?)
        }
        other => other,
    }
}

/// `deadline`까지 남은 시간. 다 지났으면 시간 초과입니다.
fn left(deadline: Instant) -> Result<Duration, LlmError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(LlmError::Network("request timed out".into()))
    } else {
        Ok(remaining)
    }
}

fn post_responses(
    base_url: &str,
    tokens: &Tokens,
    body: &Map<String, Value>,
    timeout: Duration,
) -> Result<(String, Option<String>), LlmError> {
    let url = format!("{}/responses", base_url.trim_end_matches('/'));
    let body = Value::Object(body.clone()).to_string();
    let mut response = http_agent(timeout)
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .header("Authorization", &format!("Bearer {}", tokens.access_token))
        .header(ACCOUNT_HEADER, &tokens.account_id)
        .send(body.as_bytes())
        .map_err(network_error)?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let text = response.body_mut().read_to_string().unwrap_or_default();
        return Err(http_failure(status, &text, &tokens.access_token));
    }
    read_stream(BufReader::new(response.body_mut().as_reader()))
}

/// 실패한 응답을 오류로 바꿉니다. 계정에서 쓸 수 없는 모델은 따로 알립니다.
fn http_failure(status: u16, body: &str, secret: &str) -> LlmError {
    let message = hide_secret(error_message(body), secret);
    if error_code(body).as_deref() == Some("model_not_found") {
        return LlmError::ModelNotFound(message);
    }
    LlmError::Http { status, message }
}

/// 오류 본문의 `error.code` (소문자)
fn error_code(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body.trim()).ok()?;
    let code = value.get("error")?.get("code")?.as_str()?;
    Some(code.trim().to_ascii_lowercase())
}

/// 서버 전송 이벤트(SSE)를 읽어 (답, 응답한 모델 이름)을 만듭니다.
///
/// 글 조각(`response.output_text.delta`)을 모으고, 끝(`response.completed`)에 온 전체 답이 있으면 그것을
/// 씁니다. 다듬은 글이 로그에 남지 않게, 읽지 못한 이벤트의 내용은 오류에 싣지 않습니다.
fn read_stream(reader: impl BufRead) -> Result<(String, Option<String>), LlmError> {
    let mut text = String::new();
    let mut data = String::new();
    for line in reader.lines() {
        let line = line
            .map_err(|e| LlmError::Network(format!("failed to read the response stream: {e}")))?;
        if line.is_empty() {
            if let Some(done) = take_event(&mut data, &mut text)? {
                return Ok(done);
            }
            continue;
        }
        // `event:`·`id:` 줄과 `:`로 시작하는 주석은 쓰지 않습니다(이벤트 종류는 `data`의 `type`에 있음).
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if let Some(done) = take_event(&mut data, &mut text)? {
        return Ok(done);
    }
    let text = text.trim();
    if text.is_empty() {
        return Err(LlmError::BadResponse(
            "the response stream ended before the reply was finished".into(),
        ));
    }
    tracing::warn!("Codex response stream ended without a completion event");
    Ok((text.to_owned(), None))
}

/// 모아 둔 이벤트 하나를 처리하고 비웁니다. 답이 끝났으면 `Some`입니다.
fn take_event(
    data: &mut String,
    text: &mut String,
) -> Result<Option<(String, Option<String>)>, LlmError> {
    let raw = std::mem::take(data);
    let raw = raw.trim();
    if raw.is_empty() || raw == "[DONE]" {
        return Ok(None);
    }
    let Ok(event) = serde_json::from_str::<Value>(raw) else {
        tracing::debug!(
            "skipping an unreadable Codex stream event ({} bytes)",
            raw.len()
        );
        return Ok(None);
    };
    let kind = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "response.output_text.delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                text.push_str(delta);
            }
            Ok(None)
        }
        "response.completed" | "response.incomplete" | "response.done" => {
            let response = event.get("response");
            if kind == "response.incomplete" {
                let reason = response
                    .and_then(|r| r.pointer("/incomplete_details/reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                tracing::warn!(reason, "Codex reply was cut short");
            }
            let model = response
                .and_then(|r| r.get("model"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_owned);
            let full = response.map(output_text).unwrap_or_default();
            let answer = if full.trim().is_empty() {
                std::mem::take(text)
            } else {
                full
            };
            Ok(Some((answer.trim().to_owned(), model)))
        }
        "response.failed" | "error" => Err(stream_failure(&event)),
        _ => Ok(None),
    }
}

/// 끝난 응답(`response`)의 답 글: `output`의 메시지 안 `output_text`를 모은 것
fn output_text(response: &Value) -> String {
    let Some(items) = response.get("output").and_then(Value::as_array) else {
        return String::new();
    };
    items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect()
}

/// 스트림 안에서 알린 실패를 오류로 바꿉니다. 사용 한도는 스트림 안에서 알리기도 해서 429로 봅니다.
fn stream_failure(event: &Value) -> LlmError {
    let error = event
        .pointer("/response/error")
        .filter(|error| !error.is_null())
        .or_else(|| event.get("error"))
        .unwrap_or(event);
    let field = |name: &str| {
        error
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_owned()
    };
    let code = field("code").to_ascii_lowercase();
    let kind = field("type").to_ascii_lowercase();
    let message = truncate(&field("message"));
    let is = |needle: &str| code.contains(needle) || kind.contains(needle);
    if is("rate_limit") || is("usage_limit") || is("quota") {
        return LlmError::Http {
            status: 429,
            message,
        };
    }
    if code == "model_not_found" {
        return LlmError::ModelNotFound(message);
    }
    let detail = if message.is_empty() { code } else { message };
    LlmError::BadResponse(if detail.is_empty() {
        "the model stopped with an error".into()
    } else {
        detail
    })
}

fn fetch_models(
    paths: AppPaths,
    backend: Backend,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<Vec<CodexModel>, LlmError> {
    let started = Instant::now();
    run_with_deadline(started, timeout, cancel, move |remaining| {
        let deadline = Instant::now() + remaining;
        authorized(&paths, &backend, deadline, |tokens, timeout| {
            get_models(&backend.base_url, tokens, timeout)
        })
        .inspect_err(|e| tracing::warn!("could not list Codex models: {e}"))
    })
}

fn get_models(
    base_url: &str,
    tokens: &Tokens,
    timeout: Duration,
) -> Result<Vec<CodexModel>, LlmError> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut response = http_agent(timeout)
        .get(&url)
        .query("client_version", MODELS_CLIENT_VERSION)
        .header("Accept", "application/json")
        .header("Authorization", &format!("Bearer {}", tokens.access_token))
        .header(ACCOUNT_HEADER, &tokens.account_id)
        .call()
        .map_err(network_error)?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| LlmError::Network(format!("failed to read response body: {e}")))?;
    if !(200..300).contains(&status) {
        return Err(http_failure(status, &text, &tokens.access_token));
    }
    parse_models(&text)
}

#[derive(Deserialize)]
struct ModelsReply {
    #[serde(default)]
    models: Vec<ModelInfo>,
}

#[derive(Deserialize)]
struct ModelInfo {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// `list`면 목록에 보이는 모델, `hide`면 숨긴 모델
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    priority: Option<i64>,
}

/// 모델 목록 응답에서 목록에 보일 모델을 서버가 정한 순서(`priority`)대로 꺼냅니다.
fn parse_models(raw: &str) -> Result<Vec<CodexModel>, LlmError> {
    let reply: ModelsReply = serde_json::from_str(raw).map_err(|e| {
        LlmError::BadResponse(format!(
            "invalid model list ({})",
            crate::json_error_summary(&e)
        ))
    })?;
    let mut listed: Vec<ModelInfo> = reply
        .models
        .into_iter()
        .filter(|model| !model.slug.trim().is_empty())
        .filter(|model| {
            model
                .visibility
                .as_deref()
                .is_none_or(|visibility| visibility.eq_ignore_ascii_case("list"))
        })
        .collect();
    listed.sort_by_key(|model| model.priority.unwrap_or(i64::MAX));
    let mut models: Vec<CodexModel> = Vec::new();
    for model in listed {
        let slug = model.slug.trim().to_owned();
        if models.iter().any(|known| known.slug == slug) {
            continue;
        }
        let name = model
            .display_name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| slug.clone());
        models.push(CodexModel {
            slug,
            name,
            description: model.description.unwrap_or_default().trim().to_owned(),
        });
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::codex::auth::tests::{hour_from_now, jwt, tokens};
    use crate::test_server::{self, body_of, header_of, serve, sse};

    /// 글 조각 두 개와 끝 이벤트로 된 정상 스트림
    fn ok_stream(answer_parts: &[&str], model: &str) -> String {
        let mut stream = String::from(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{}}\n\n",
        );
        for part in answer_parts {
            stream.push_str(&format!(
                "event: response.output_text.delta\ndata: {}\n\n",
                json!({ "type": "response.output_text.delta", "delta": part })
            ));
        }
        let full: String = answer_parts.concat();
        stream.push_str(&format!(
            "event: response.completed\ndata: {}\n\n",
            json!({
                "type": "response.completed",
                "response": {
                    "model": model,
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": full }],
                    }],
                },
            })
        ));
        stream
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        paths: AppPaths,
        /// 저장해 둔 토큰
        tokens: Tokens,
    }

    impl Fixture {
        /// 곧 만료되지 않는 로그인을 저장해 둔 데이터 폴더
        fn signed_in() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let paths = AppPaths::under(dir.path());
            let tokens = tokens(hour_from_now());
            auth::store(&paths, &tokens).unwrap();
            Self {
                _dir: dir,
                paths,
                tokens,
            }
        }

        fn request(
            &self,
            base: &str,
            effort: ReasoningEffort,
            when_rejected: WhenRejected,
        ) -> Request {
            Request {
                paths: self.paths.clone(),
                backend: Backend {
                    base_url: format!("{base}/backend-api/codex"),
                    token_url: format!("{base}/oauth/token"),
                },
                body: request_body("gpt-6-luna", "맞춤법을 고쳐 주세요.", "안녕 하세요"),
                effort,
                when_rejected,
            }
        }
    }

    fn quick() -> Duration {
        Duration::from_secs(10)
    }

    #[test]
    fn sends_a_responses_request_and_reads_the_stream() {
        let fixture = Fixture::signed_in();
        let (base, server) = serve(vec![sse(ok_stream(&["안녕", "하세요."], "gpt-6-luna"))]);
        let reply = fixture
            .request(&base, ReasoningEffort::Default, WhenRejected::SendWithout)
            .send(quick(), &CancelToken::new())
            .unwrap();
        let raw = server.join().unwrap().remove(0);

        assert!(
            raw.starts_with("POST /backend-api/codex/responses HTTP/1.1"),
            "{raw}"
        );
        assert_eq!(
            header_of(&raw, "authorization"),
            Some(format!("Bearer {}", fixture.tokens.access_token).as_str())
        );
        assert_eq!(header_of(&raw, "chatgpt-account-id"), Some("acc-1"));
        assert_eq!(header_of(&raw, "accept"), Some("text/event-stream"));
        let body = body_of(&raw);
        assert_eq!(body["model"], "gpt-6-luna");
        assert_eq!(body["instructions"], "맞춤법을 고쳐 주세요.");
        assert_eq!(
            body["input"],
            json!([{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "안녕 하세요" }],
            }])
        );
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert!(
            body.get("reasoning").is_none(),
            "모델 기본값이면 추론 단계를 넣지 않습니다"
        );

        assert_eq!(reply.text, "안녕하세요.");
        assert_eq!(reply.model.as_deref(), Some("gpt-6-luna"));
    }

    #[test]
    fn chosen_reasoning_effort_is_sent_and_dropped_when_rejected() {
        let rejected = r#"{"error":{"type":"invalid_request_error","code":"unsupported_value","message":"Unsupported value: 'reasoning.effort'"}}"#;

        let fixture = Fixture::signed_in();
        let (base, server) = serve(vec![sse(ok_stream(&["OK"], "m"))]);
        fixture
            .request(&base, ReasoningEffort::None, WhenRejected::SendWithout)
            .send(quick(), &CancelToken::new())
            .unwrap();
        let raw = server.join().unwrap().remove(0);
        assert_eq!(body_of(&raw)["reasoning"], json!({ "effort": "none" }));

        // 다듬기: 추론 단계를 빼고 다시 보낸 답을 씁니다.
        let (base, server) = serve(vec![
            test_server::json("400 Bad Request", rejected),
            sse(ok_stream(&["고친 글"], "m")),
        ]);
        let reply = fixture
            .request(&base, ReasoningEffort::Low, WhenRejected::SendWithout)
            .send(quick(), &CancelToken::new())
            .unwrap();
        let requests = server.join().unwrap();
        assert_eq!(
            body_of(&requests[0])["reasoning"],
            json!({ "effort": "low" })
        );
        assert!(body_of(&requests[1]).get("reasoning").is_none());
        assert_eq!(reply.text, "고친 글");

        // 연결 테스트: 고른 단계가 원인이라고 알립니다.
        let (base, server) = serve(vec![
            test_server::json("400 Bad Request", rejected),
            sse(ok_stream(&["OK"], "m")),
        ]);
        let result = fixture
            .request(&base, ReasoningEffort::High, WhenRejected::Report)
            .send(quick(), &CancelToken::new());
        server.join().unwrap();
        assert!(
            matches!(result, Err(LlmError::ReasoningRejected { status: 400, .. })),
            "{result:?}"
        );
    }

    #[test]
    fn unknown_model_is_reported_without_retrying() {
        let fixture = Fixture::signed_in();
        let (base, server) = serve(vec![test_server::json(
            "400 Bad Request",
            r#"{"detail":"The 'x' model is not supported when using Codex with a ChatGPT account.","error":{"type":"invalid_request_error","code":"model_not_found","message":"The 'x' model is not supported when using Codex with a ChatGPT account."}}"#,
        )]);
        let result = fixture
            .request(&base, ReasoningEffort::Low, WhenRejected::SendWithout)
            .send(quick(), &CancelToken::new());
        assert_eq!(
            server.join().unwrap().len(),
            1,
            "모델이 없으면 다시 보내지 않습니다"
        );
        assert!(
            matches!(&result, Err(LlmError::ModelNotFound(message)) if message.contains("not supported")),
            "{result:?}"
        );
    }

    #[test]
    fn rejected_token_is_refreshed_and_the_request_sent_again() {
        let fixture = Fixture::signed_in();
        let new_access = jwt(json!({ "exp": hour_from_now() + 60 }));
        let (base, server) = serve(vec![
            test_server::json(
                "401 Unauthorized",
                r#"{"detail":"Could not parse your authentication token."}"#,
            ),
            test_server::json(
                "200 OK",
                json!({ "access_token": new_access, "refresh_token": "rt-new" }).to_string(),
            ),
            sse(ok_stream(&["OK"], "m")),
        ]);
        let reply = fixture
            .request(&base, ReasoningEffort::Default, WhenRejected::SendWithout)
            .send(quick(), &CancelToken::new())
            .unwrap();
        let requests = server.join().unwrap();
        assert!(
            requests[1].starts_with("POST /oauth/token"),
            "{}",
            requests[1]
        );
        assert_eq!(
            header_of(&requests[2], "authorization"),
            Some(format!("Bearer {new_access}").as_str()),
            "새로 받은 토큰으로 다시 보내야 합니다"
        );
        assert_eq!(reply.text, "OK");
    }

    #[test]
    fn signed_out_fails_before_sending() {
        let dir = tempfile::tempdir().unwrap();
        let result = chat(
            &AppPaths::under(dir.path()),
            "gpt-6-luna",
            ReasoningEffort::Default,
            "지시",
            "글",
            quick(),
            &CancelToken::new(),
        );
        assert_eq!(result, Err(LlmError::SignedOut));
    }

    #[test]
    fn cancel_returns_promptly() {
        let fixture = Fixture::signed_in();
        // 연결은 받지만 답하지 않는 서버
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        let started = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            canceller.cancel();
        });
        let result = fixture
            .request(&base, ReasoningEffort::Default, WhenRejected::SendWithout)
            .send(Duration::from_secs(30), &cancel);
        assert_eq!(result, Err(LlmError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(listener);
    }

    #[test]
    fn stream_failures_are_reported() {
        let failed = |error: Value| {
            let event = json!({ "type": "response.failed", "response": { "error": error } });
            read_stream(Cursor::new(format!("data: {event}\n\n")))
        };
        assert!(matches!(
            failed(json!({ "code": "usage_limit_reached", "message": "limit" })),
            Err(LlmError::Http { status: 429, .. })
        ));
        assert!(matches!(
            failed(json!({ "code": "server_error", "message": "boom" })),
            Err(LlmError::BadResponse(message)) if message == "boom"
        ));
        let error = json!({ "type": "error", "error": { "type": "rate_limit_exceeded", "message": "slow" } });
        assert!(matches!(
            read_stream(Cursor::new(format!("data: {error}\n\n"))),
            Err(LlmError::Http { status: 429, .. })
        ));
    }

    #[test]
    fn stream_without_completion_uses_the_collected_text() {
        let delta = json!({ "type": "response.output_text.delta", "delta": " 반쯤 " });
        let raw = format!(": keep-alive\n\nevent: x\ndata: {delta}\n\ndata: not json\n\n");
        assert_eq!(
            read_stream(Cursor::new(raw)).unwrap(),
            ("반쯤".to_owned(), None)
        );
        assert!(matches!(
            read_stream(Cursor::new("data: [DONE]\n\n")),
            Err(LlmError::BadResponse(_))
        ));
    }

    #[test]
    fn model_list_keeps_listed_models_in_server_order() {
        let fixture = Fixture::signed_in();
        let (base, server) = serve(vec![test_server::json(
            "200 OK",
            json!({ "models": [
                { "slug": "gpt-5.6-terra", "display_name": "GPT 5.6 Terra", "visibility": "list", "priority": 3 },
                { "slug": "gpt-reserve", "visibility": "hide", "priority": 1 },
                { "slug": "gpt-6-luna", "display_name": "GPT 6.0 Luna", "description": " Fast ", "visibility": "list", "priority": 2 },
                { "slug": "plain", "priority": 9 },
                { "slug": "gpt-6-luna", "visibility": "list", "priority": 10 },
            ]})
            .to_string(),
        )]);
        let models = fetch_models(
            fixture.paths.clone(),
            Backend {
                base_url: format!("{base}/backend-api/codex"),
                token_url: format!("{base}/oauth/token"),
            },
            quick(),
            &CancelToken::new(),
        )
        .unwrap();
        let raw = server.join().unwrap().remove(0);
        assert!(
            raw.starts_with(&format!(
                "GET /backend-api/codex/models?client_version={MODELS_CLIENT_VERSION} HTTP/1.1"
            )),
            "{raw}"
        );
        assert_eq!(header_of(&raw, "chatgpt-account-id"), Some("acc-1"));
        let slugs: Vec<_> = models.iter().map(|m| m.slug.as_str()).collect();
        assert_eq!(slugs, ["gpt-6-luna", "gpt-5.6-terra", "plain"]);
        assert_eq!(models[0].name, "GPT 6.0 Luna");
        assert_eq!(models[0].description, "Fast");
        assert_eq!(
            models[2].name, "plain",
            "보여 줄 이름이 없으면 모델 이름을 씁니다"
        );
    }
}
