//! 브라우저로 ChatGPT 계정에 로그인합니다 (OAuth 인가 코드 + PKCE, Codex CLI와 같은 방법).
//!
//! 1. 로그인을 마친 브라우저가 돌아올 주소를 이 PC의 `127.0.0.1:1455`(쓰고 있으면 `1457`)에서 기다립니다.
//!    Codex 로그인 서버에 등록된 돌아올 주소가 이 두 포트뿐이라 다른 포트는 쓸 수 없습니다.
//! 2. 앱이 로그인 주소([`CodexLogin::auth_url`])를 브라우저로 엽니다.
//! 3. 돌아온 인가 코드를 토큰으로 바꿔 [`super::auth`]에 저장하고, 브라우저에 결과 페이지를 보여 줍니다.
//!
//! 브라우저에 보여 줄 문구는 앱이 번역 파일에서 골라 [`BrowserPages`]로 넘깁니다.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sulsul_core::{AppPaths, CancelToken};
use url::Url;

use super::auth::{self, CLIENT_ID, CodexAccount, ISSUER, Tokens};
use super::http_agent;
use crate::client::{LlmError, error_message, hide_secret, network_error};
use crate::job::Job;

/// 로그인을 마친 브라우저가 돌아올 포트 (앞의 것을 먼저 씀)
pub const CALLBACK_PORTS: [u16; 2] = [1455, 1457];

/// 브라우저가 로그인을 마칠 때까지 기다리는 시간
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// 브라우저가 돌아오는 경로
const CALLBACK_PATH: &str = "/auth/callback";

/// Codex CLI가 새 로그인을 시작하며 같은 포트에서 기다리던 로그인을 멈추게 할 때 부르는 경로
const CANCEL_PATH: &str = "/cancel";

/// 요청하는 권한. `offline_access`가 있어야 refresh token을 받습니다.
const SCOPE: &str = "openid profile email offline_access";

/// 로그인 페이지에 알리는 클라이언트 이름. Codex의 공개 클라이언트로 로그인하므로 Codex CLI의 값을 씁니다.
const ORIGINATOR: &str = "codex_cli_rs";

/// 인가 코드를 토큰으로 바꾸는 요청의 제한 시간
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);

/// 브라우저가 들어왔는지, 취소됐는지 확인하는 간격
const ACCEPT_INTERVAL: Duration = Duration::from_millis(50);

/// 들어온 연결에서 요청을 읽고 답을 쓰는 제한 시간. 브라우저가 미리 열어 두고 아무것도 보내지 않는
/// 연결이 있으면 그동안 다음 연결을 받지 못하므로 짧게 둡니다(이 PC 안의 요청은 바로 옵니다).
const STREAM_TIMEOUT: Duration = Duration::from_secs(2);

/// 브라우저 요청 머리를 이만큼까지만 읽습니다.
const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// 브라우저에 보여 줄 페이지의 문구
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserPage {
    pub title: String,
    pub body: String,
}

/// 로그인을 마쳤을 때와 실패했을 때 브라우저에 보여 줄 페이지
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserPages {
    pub success: BrowserPage,
    pub failure: BrowserPage,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoginError {
    /// 돌아올 주소의 포트를 모두 다른 프로그램이 쓰고 있습니다.
    #[error("the sign-in callback ports are in use ({0})")]
    PortsBusy(String),
    /// 로그인을 준비하지 못했습니다 (난수, 작업 스레드).
    #[error("could not start the sign-in: {0}")]
    Start(String),
    /// 로그인 페이지에서 거절했거나 오류가 났습니다.
    #[error("the sign-in was refused: {0}")]
    Refused(String),
    /// 인가 코드를 토큰으로 바꾸지 못했습니다.
    #[error("could not finish the sign-in: {0}")]
    Exchange(LlmError),
    /// 로그인한 계정에 ChatGPT 작업 공간이 없습니다.
    #[error("the account has no ChatGPT workspace")]
    NoAccount,
    /// 받은 로그인 정보를 저장하지 못했습니다.
    #[error("could not save the sign-in: {0}")]
    Save(String),
    /// 브라우저에서 로그인을 마치지 않았습니다.
    #[error("the sign-in was not finished in time")]
    TimedOut,
    /// 다른 프로그램(Codex CLI 등)이 같은 포트로 로그인을 시작해 멈췄습니다.
    #[error("another program started a sign-in on the same port")]
    Interrupted,
    #[error("cancelled")]
    Cancelled,
}

/// 진행 중인 브라우저 로그인. 버리면 기다리기를 멈춥니다.
pub struct CodexLogin {
    auth_url: String,
    job: Job<Result<CodexAccount, LoginError>>,
}

impl CodexLogin {
    /// 돌아올 주소를 열고 기다리기 시작합니다. 브라우저는 부르는 쪽이 [`Self::auth_url`]로 엽니다.
    ///
    /// 로그인을 마치면 토큰을 저장하고, `on_finish`가 작업 스레드에서 한 번 불립니다.
    pub fn start(
        paths: AppPaths,
        pages: BrowserPages,
        on_finish: impl Fn() + Send + 'static,
    ) -> Result<Self, LoginError> {
        Self::start_with(paths, pages, Settings::standard(), on_finish)
    }

    fn start_with(
        paths: AppPaths,
        pages: BrowserPages,
        settings: Settings,
        on_finish: impl Fn() + Send + 'static,
    ) -> Result<Self, LoginError> {
        let verifier = random_token(64)?;
        let challenge = URL_SAFE_NO_PAD.encode(&Sha256::digest(verifier.as_bytes())[..]);
        let state = random_token(32)?;
        let listener = bind(&settings.ports)?;
        let port = listener
            .local_addr()
            .map_err(|e| LoginError::Start(e.to_string()))?
            .port();
        let issuer = settings.issuer.trim_end_matches('/');
        let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let auth_url = authorize_url(issuer, &redirect_uri, &challenge, &state);
        tracing::info!(port, "waiting for the Codex sign-in in the browser");

        let session = Session {
            paths,
            pages,
            token_url: format!("{issuer}/oauth/token"),
            redirect_uri,
            verifier,
            state,
            timeout: settings.timeout,
        };
        let job = Job::start(
            "sulsul-codex-login",
            Err(LoginError::Cancelled),
            on_finish,
            move |cancel| session.wait(&listener, cancel),
        )
        .map_err(|e| LoginError::Start(e.to_string()))?;
        Ok(Self { auth_url, job })
    }

    /// 브라우저로 열 로그인 주소
    pub fn auth_url(&self) -> &str {
        &self.auth_url
    }

    /// 끝났으면 결과를 돌려줍니다 (부를 때마다 같은 결과).
    pub fn poll(&self) -> Option<Result<CodexAccount, LoginError>> {
        self.job.poll()
    }

    /// 기다리기를 멈춥니다.
    pub fn cancel(&self) {
        self.job.cancel();
    }

    /// 시작한 뒤 지난 시간
    pub fn elapsed(&self) -> Duration {
        self.job.elapsed()
    }
}

/// 로그인 서버 주소와 기다리는 방법 (테스트에서 바꿈)
struct Settings {
    issuer: String,
    ports: Vec<u16>,
    timeout: Duration,
}

impl Settings {
    fn standard() -> Self {
        Self {
            issuer: ISSUER.to_owned(),
            ports: CALLBACK_PORTS.to_vec(),
            timeout: LOGIN_TIMEOUT,
        }
    }
}

/// 브라우저가 돌아오기를 기다리는 로그인 한 번
struct Session {
    paths: AppPaths,
    pages: BrowserPages,
    token_url: String,
    redirect_uri: String,
    /// PKCE 검증 값. 로그에 남기지 않습니다.
    verifier: String,
    /// 이번 로그인을 알아보는 값. 다른 로그인의 응답을 받지 않으려고 씁니다.
    state: String,
    timeout: Duration,
}

impl Session {
    fn wait(
        &self,
        listener: &TcpListener,
        cancel: &CancelToken,
    ) -> Result<CodexAccount, LoginError> {
        listener
            .set_nonblocking(true)
            .map_err(|e| LoginError::Start(e.to_string()))?;
        let started = Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Err(LoginError::Cancelled);
            }
            if started.elapsed() >= self.timeout {
                return Err(LoginError::TimedOut);
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Some(result) = self.handle(stream) {
                        return result;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(ACCEPT_INTERVAL);
                }
                Err(error) => {
                    tracing::warn!("sign-in callback connection failed: {error}");
                    thread::sleep(ACCEPT_INTERVAL);
                }
            }
        }
    }

    /// 들어온 요청 하나를 처리합니다. 로그인이 끝났으면 그 결과를, 더 기다려야 하면 `None`을 돌려줍니다.
    fn handle(&self, mut stream: TcpStream) -> Option<Result<CodexAccount, LoginError>> {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(STREAM_TIMEOUT));
        let _ = stream.set_write_timeout(Some(STREAM_TIMEOUT));
        let target = read_request_target(&mut stream)?;
        let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
            respond(&mut stream, "400 Bad Request", "text/plain", "Bad request");
            return None;
        };
        match url.path() {
            CALLBACK_PATH => {}
            CANCEL_PATH => {
                tracing::info!("another program asked to stop the Codex sign-in");
                respond(&mut stream, "200 OK", "text/plain", "Sign-in cancelled");
                return Some(Err(LoginError::Interrupted));
            }
            // 브라우저가 함께 부르는 아이콘 등
            _ => {
                respond(&mut stream, "404 Not Found", "text/plain", "Not found");
                return None;
            }
        }

        let callback = Callback::from_url(&url);
        if callback.state.as_deref() != Some(self.state.as_str()) {
            // 예전 로그인 창에서 돌아왔을 수 있습니다. 이번 로그인은 계속 기다립니다.
            tracing::warn!("ignoring a sign-in callback for another sign-in attempt");
            self.page(&mut stream, "400 Bad Request", &self.pages.failure);
            return None;
        }
        if let Some(error) = callback.error {
            let reason = match callback.error_description {
                Some(description) => format!("{error}: {description}"),
                None => error,
            };
            self.page(&mut stream, "200 OK", &self.pages.failure);
            return Some(Err(LoginError::Refused(reason)));
        }
        let Some(code) = callback.code.filter(|code| !code.is_empty()) else {
            self.page(&mut stream, "200 OK", &self.pages.failure);
            return Some(Err(LoginError::Refused("no authorization code".into())));
        };

        let result = self.exchange(&code);
        let page = if result.is_ok() {
            &self.pages.success
        } else {
            &self.pages.failure
        };
        self.page(&mut stream, "200 OK", page);
        Some(result)
    }

    /// 인가 코드를 토큰으로 바꿔 저장합니다.
    fn exchange(&self, code: &str) -> Result<CodexAccount, LoginError> {
        let mut response = http_agent(EXCHANGE_TIMEOUT)
            .post(&self.token_url)
            .header("Accept", "application/json")
            .send_form([
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", code),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("code_verifier", self.verifier.as_str()),
            ])
            .map_err(|e| LoginError::Exchange(network_error(e)))?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().map_err(|e| {
            LoginError::Exchange(LlmError::Network(format!(
                "failed to read response body: {e}"
            )))
        })?;
        if !(200..300).contains(&status) {
            let message = hide_secret(hide_secret(error_message(&text), code), &self.verifier);
            return Err(LoginError::Exchange(LlmError::Http { status, message }));
        }
        // 응답에는 토큰이 들어 있어 오류(로그에 남음)에 본문을 싣지 않습니다.
        let reply: TokenReply = serde_json::from_str(&text).map_err(|e| {
            LoginError::Exchange(LlmError::BadResponse(format!(
                "invalid token response ({})",
                crate::json_error_summary(&e)
            )))
        })?;
        let account_id = auth::account_id_of(&reply.id_token).ok_or(LoginError::NoAccount)?;
        let tokens = Tokens {
            id_token: reply.id_token,
            access_token: reply.access_token,
            refresh_token: reply.refresh_token,
            account_id,
        };
        auth::store(&self.paths, &tokens).map_err(|e| LoginError::Save(e.to_string()))?;
        let account = auth::account_of(&tokens);
        tracing::info!(plan = ?account.plan, "signed in to Codex");
        Ok(account)
    }

    fn page(&self, stream: &mut TcpStream, status: &str, page: &BrowserPage) {
        respond(stream, status, "text/html; charset=utf-8", &page_html(page));
    }
}

/// 인가 코드를 토큰으로 바꾼 응답
#[derive(Deserialize)]
struct TokenReply {
    id_token: String,
    access_token: String,
    #[serde(default)]
    refresh_token: String,
}

/// 돌아온 주소의 값. 인가 코드는 비밀이라 `Debug`를 두지 않습니다.
struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

impl Callback {
    fn from_url(url: &Url) -> Self {
        let mut callback = Self {
            code: None,
            state: None,
            error: None,
            error_description: None,
        };
        for (name, value) in url.query_pairs() {
            let value = Some(value.into_owned());
            match name.as_ref() {
                "code" => callback.code = value,
                "state" => callback.state = value,
                "error" => callback.error = value,
                "error_description" => callback.error_description = value,
                _ => {}
            }
        }
        callback
    }
}

/// 앞의 포트부터 열어 봅니다. 모두 쓰고 있으면 `PortsBusy`입니다.
fn bind(ports: &[u16]) -> Result<TcpListener, LoginError> {
    let mut failures = Vec::new();
    for &port in ports {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => return Ok(listener),
            Err(error) => {
                tracing::warn!(port, "sign-in callback port is not available: {error}");
                failures.push(port.to_string());
            }
        }
    }
    Err(LoginError::PortsBusy(failures.join(", ")))
}

/// 로그인 페이지 주소 (Codex CLI와 같은 값)
fn authorize_url(issuer: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", redirect_uri),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
            ("scope", SCOPE),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", ORIGINATOR),
        ])
        .finish();
    format!("{issuer}/oauth/authorize?{query}")
}

/// URL에 넣을 수 있는 무작위 값 (`bytes`바이트를 base64url로)
fn random_token(bytes: usize) -> Result<String, LoginError> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| LoginError::Start(format!("no random source: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(&buf))
}

/// 요청 첫 줄의 대상(`/auth/callback?...`)을 읽습니다. GET 요청이 아니거나 읽지 못하면 `None`입니다.
fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < MAX_REQUEST_BYTES {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let mut parts = head.lines().next()?.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    (method.eq_ignore_ascii_case("GET") && target.starts_with('/')).then(|| target.to_owned())
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let written = stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body.as_bytes()))
        .and_then(|()| stream.flush());
    if let Err(error) = written {
        tracing::debug!("could not answer the browser: {error}");
    }
}

/// 브라우저에 보여 줄 페이지의 모양. 앱 화면처럼 어두운 바탕에 제목과 설명 한 줄입니다.
const PAGE_STYLE: &str = "body{margin:0;min-height:100vh;display:flex;align-items:center;\
justify-content:center;background:#17181b;color:#ececf0;\
font-family:Pretendard,'Malgun Gothic',system-ui,sans-serif}\
main{max-width:30rem;padding:2rem;text-align:center}\
h1{font-size:1.5rem;margin:0 0 .75rem}p{margin:0;color:#a3a3ad;line-height:1.6}";

fn page_html(page: &BrowserPage) -> String {
    let title = escape_html(&page.title);
    let body = escape_html(&page.body);
    format!(
        "<!doctype html><html lang=\"ko\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><style>{PAGE_STYLE}</style></head>\
         <body><main><h1>{title}</h1><p>{body}</p></main></body></html>"
    )
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use serde_json::json;

    use super::*;
    use crate::codex::auth::tests::jwt;
    use crate::test_server::{self, serve};

    fn pages() -> BrowserPages {
        BrowserPages {
            success: BrowserPage {
                title: "로그인했습니다".into(),
                body: "이 창을 닫으세요.".into(),
            },
            failure: BrowserPage {
                title: "로그인하지 못했습니다".into(),
                body: "다시 하세요.".into(),
            },
        }
    }

    fn settings(issuer: &str) -> Settings {
        Settings {
            issuer: issuer.to_owned(),
            ports: vec![0],
            timeout: Duration::from_secs(30),
        }
    }

    fn param(url: &str, name: &str) -> String {
        Url::parse(url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_else(|| panic!("{name}가 없습니다: {url}"))
    }

    /// 브라우저처럼 돌아올 주소에 GET 요청을 보내고 응답 원문을 돌려줍니다.
    fn browser_get(login: &CodexLogin, target: &str) -> String {
        let redirect = Url::parse(&param(login.auth_url(), "redirect_uri")).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", redirect.port().unwrap())).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn wait_result(login: &CodexLogin) -> Result<CodexAccount, LoginError> {
        let started = Instant::now();
        loop {
            if let Some(result) = login.poll() {
                return result;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "로그인이 끝나지 않습니다"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn signs_in_through_the_browser_and_saves_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        let id_token = jwt(json!({
            "email": "me@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-1",
                "chatgpt_plan_type": "plus",
            },
        }));
        let (issuer, token_server) = serve(vec![test_server::json(
            "200 OK",
            json!({
                "id_token": id_token,
                "access_token": jwt(json!({ "exp": 4_000_000_000_u64 })),
                "refresh_token": "rt-1",
            })
            .to_string(),
        )]);
        let (finished_tx, finished_rx) = mpsc::channel();
        let login = CodexLogin::start_with(paths.clone(), pages(), settings(&issuer), move || {
            let _ = finished_tx.send(());
        })
        .unwrap();

        // 로그인 주소는 Codex CLI와 같은 값을 담아야 합니다.
        let url = login.auth_url().to_owned();
        assert!(
            url.starts_with(&format!("{issuer}/oauth/authorize?")),
            "{url}"
        );
        assert_eq!(param(&url, "client_id"), CLIENT_ID);
        assert_eq!(param(&url, "response_type"), "code");
        assert_eq!(param(&url, "code_challenge_method"), "S256");
        assert!(param(&url, "scope").contains("offline_access"));
        let redirect = param(&url, "redirect_uri");
        assert!(redirect.starts_with("http://127.0.0.1:"), "{redirect}");
        assert!(redirect.ends_with(CALLBACK_PATH), "{redirect}");
        let state = param(&url, "state");
        let challenge = param(&url, "code_challenge");

        // 아이콘 요청과 다른 로그인의 응답은 넘기고 계속 기다립니다.
        assert!(browser_get(&login, "/favicon.ico").starts_with("HTTP/1.1 404"));
        let stale = browser_get(&login, "/auth/callback?code=old&state=other");
        assert!(stale.starts_with("HTTP/1.1 400"), "{stale}");
        assert!(
            login.poll().is_none(),
            "다른 로그인의 응답으로 끝나면 안 됩니다"
        );

        let page = browser_get(&login, &format!("/auth/callback?code=c%2B1&state={state}"));
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("로그인했습니다"), "{page}");
        let account = wait_result(&login).unwrap();
        assert_eq!(account.email.as_deref(), Some("me@example.com"));
        assert_eq!(account.plan.as_deref(), Some("plus"));
        finished_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("끝나면 on_finish가 불려야 합니다");
        assert_eq!(
            auth::account(&paths),
            Some(account),
            "로그인 정보가 저장돼야 합니다"
        );

        // 토큰 요청은 인가 코드와 PKCE 검증 값을 폼으로 보냅니다.
        let raw = token_server.join().unwrap().remove(0);
        assert!(raw.starts_with("POST /oauth/token HTTP/1.1"), "{raw}");
        let form: Vec<(String, String)> =
            url::form_urlencoded::parse(raw.split("\r\n\r\n").nth(1).unwrap().as_bytes())
                .into_owned()
                .collect();
        let field = |name: &str| {
            form.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(field("grant_type"), "authorization_code");
        assert_eq!(field("client_id"), CLIENT_ID);
        assert_eq!(field("code"), "c+1");
        assert_eq!(field("redirect_uri"), redirect);
        let verifier = field("code_verifier");
        assert_eq!(
            URL_SAFE_NO_PAD.encode(&Sha256::digest(verifier.as_bytes())[..]),
            challenge,
            "검증 값의 SHA-256이 로그인 주소의 challenge와 같아야 합니다"
        );
    }

    #[test]
    fn refused_sign_in_ends_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let login = CodexLogin::start_with(
            AppPaths::under(dir.path()),
            pages(),
            settings("http://127.0.0.1:1"),
            || {},
        )
        .unwrap();
        let state = param(login.auth_url(), "state");
        let page = browser_get(
            &login,
            &format!("/auth/callback?error=access_denied&error_description=no+way&state={state}"),
        );
        assert!(page.contains("로그인하지 못했습니다"), "{page}");
        assert_eq!(
            wait_result(&login),
            Err(LoginError::Refused("access_denied: no way".into()))
        );
    }

    #[test]
    fn failed_exchange_shows_the_failure_page() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        let (issuer, server) = serve(vec![test_server::json(
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"bad code"}"#,
        )]);
        let login =
            CodexLogin::start_with(paths.clone(), pages(), settings(&issuer), || {}).unwrap();
        let state = param(login.auth_url(), "state");
        let page = browser_get(&login, &format!("/auth/callback?code=c&state={state}"));
        server.join().unwrap();
        assert!(page.contains("로그인하지 못했습니다"), "{page}");
        assert!(matches!(
            wait_result(&login),
            Err(LoginError::Exchange(LlmError::Http { status: 400, .. }))
        ),);
        assert_eq!(auth::account(&paths), None);
    }

    #[test]
    fn account_without_a_chatgpt_workspace_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        let (issuer, server) = serve(vec![test_server::json(
            "200 OK",
            json!({
                "id_token": jwt(json!({ "email": "me@example.com" })),
                "access_token": "a",
                "refresh_token": "r",
            })
            .to_string(),
        )]);
        let login =
            CodexLogin::start_with(paths.clone(), pages(), settings(&issuer), || {}).unwrap();
        let state = param(login.auth_url(), "state");
        browser_get(&login, &format!("/auth/callback?code=c&state={state}"));
        server.join().unwrap();
        assert_eq!(wait_result(&login), Err(LoginError::NoAccount));
        assert_eq!(auth::account(&paths), None);
    }

    #[test]
    fn cancel_and_codex_cancel_request_stop_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        let login = CodexLogin::start_with(
            paths.clone(),
            pages(),
            settings("http://127.0.0.1:1"),
            || {},
        )
        .unwrap();
        login.cancel();
        assert_eq!(login.poll(), Some(Err(LoginError::Cancelled)));

        let login =
            CodexLogin::start_with(paths, pages(), settings("http://127.0.0.1:1"), || {}).unwrap();
        assert!(browser_get(&login, "/cancel").starts_with("HTTP/1.1 200"));
        assert_eq!(wait_result(&login), Err(LoginError::Interrupted));
    }

    #[test]
    fn gives_up_after_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let login = CodexLogin::start_with(
            AppPaths::under(dir.path()),
            pages(),
            Settings {
                timeout: Duration::from_millis(100),
                ..settings("http://127.0.0.1:1")
            },
            || {},
        )
        .unwrap();
        assert_eq!(wait_result(&login), Err(LoginError::TimedOut));
    }

    #[test]
    fn busy_ports_are_reported() {
        let taken = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let result = CodexLogin::start_with(
            AppPaths::under(dir.path()),
            pages(),
            Settings {
                ports: vec![port],
                ..settings("http://127.0.0.1:1")
            },
            || {},
        );
        assert_eq!(result.err(), Some(LoginError::PortsBusy(port.to_string())));
    }

    #[test]
    fn browser_page_escapes_its_text() {
        let html = page_html(&BrowserPage {
            title: "<b>제목</b>".into(),
            body: "a & \"b\"".into(),
        });
        assert!(html.contains("&lt;b&gt;제목&lt;/b&gt;"), "{html}");
        assert!(html.contains("a &amp; &quot;b&quot;"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
    }
}
