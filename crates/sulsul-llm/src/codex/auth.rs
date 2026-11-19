//! Codex 로그인 정보(토큰)를 저장하고, 만료되기 전에 다시 받습니다.
//!
//! 로그인 정보는 `<데이터 폴더>/codex/auth.json`에 둡니다([`auth_file`]). access token은 열흘쯤 쓸 수 있고,
//! 만료되기 조금 전이나 서버가 401로 거절하면 refresh token으로 새로 받습니다. refresh token은 새로 받을
//! 때마다 바뀌므로 다시 받기는 한 번에 하나만 합니다. refresh token마저 만료·취소됐으면 로그인 정보를 지우고
//! [`LlmError::SignedOut`]으로 알립니다(다시 로그인해야 함).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD_INDIFFERENT;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sulsul_core::AppPaths;

use super::http_agent;
use crate::client::{LlmError, error_message, hide_secret, network_error};

/// ChatGPT 계정 로그인 서버 (OAuth 발급자)
pub const ISSUER: &str = "https://auth.openai.com";

/// Codex CLI가 쓰는 공개 OAuth 클라이언트 ID. 술술은 Codex와 같은 로그인을 씁니다.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// 토큰을 받고 다시 받는 주소
pub(crate) const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

const AUTH_DIR: &str = "codex";
const AUTH_FILE: &str = "auth.json";

/// access token이 만료되기 이만큼 전부터 새로 받습니다.
const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);

/// access token에서 만료 시각을 읽지 못하면 마지막으로 받은 뒤 이만큼 지났을 때 새로 받습니다 (Codex CLI와 같음).
const REFRESH_INTERVAL: Duration = Duration::from_secs(8 * 24 * 60 * 60);

/// 토큰 클레임에서 ChatGPT 계정 정보가 든 항목 이름
const AUTH_CLAIM: &str = "https://api.openai.com/auth";

/// access token 클레임에서 이메일이 든 항목 이름
const PROFILE_CLAIM: &str = "https://api.openai.com/profile";

/// 다시 받을 수 없는 refresh token이라고 알리는 오류 코드 (Codex CLI가 알아보는 것과 같음)
const DEAD_REFRESH_CODES: [&str; 4] = [
    "invalid_grant",
    "refresh_token_expired",
    "refresh_token_reused",
    "refresh_token_invalidated",
];

/// 토큰 다시 받기는 한 번에 하나만 합니다 (refresh token은 한 번 쓰면 바뀜).
static REFRESH_LOCK: Mutex<()> = Mutex::new(());

/// [`account`]가 파일이 바뀌었을 때만 다시 읽도록 마지막으로 읽은 내용을 둡니다.
static ACCOUNT_CACHE: Mutex<Option<CachedAccount>> = Mutex::new(None);

struct CachedAccount {
    path: PathBuf,
    stamp: Option<FileStamp>,
    account: Option<CodexAccount>,
}

/// 파일이 바뀌었는지 알아보는 값 (수정 시각, 크기)
type FileStamp = (SystemTime, u64);

/// 로그인한 ChatGPT 계정 (화면에 보여 줄 정보)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexAccount {
    /// 로그인한 이메일
    pub email: Option<String>,
    /// ChatGPT 요금제 (`plus`, `pro` 등 서버가 준 값 그대로)
    pub plan: Option<String>,
}

/// 저장한 토큰. 로그에 남지 않게 `Debug`는 값을 가립니다.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Tokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    /// 요청 머리글 `chatgpt-account-id`에 넣는 ChatGPT 계정(작업 공간) ID
    pub account_id: String,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tokens { .. }")
    }
}

/// `auth.json`의 내용
#[derive(Serialize, Deserialize)]
struct AuthFile {
    tokens: Tokens,
    /// 마지막으로 토큰을 받은 시각 (유닉스 초)
    #[serde(default)]
    last_refresh: u64,
}

/// 로그인 정보 파일 (`<데이터 폴더>/codex/auth.json`)
pub fn auth_file(paths: &AppPaths) -> PathBuf {
    paths.data_dir.join(AUTH_DIR).join(AUTH_FILE)
}

/// 로그인한 계정. 로그인하지 않았거나 로그인 정보를 읽을 수 없으면 `None`입니다.
///
/// 화면을 그릴 때마다 불러도 되도록, 파일이 바뀌었을 때만 다시 읽습니다.
pub fn account(paths: &AppPaths) -> Option<CodexAccount> {
    let path = auth_file(paths);
    let stamp = file_stamp(&path);
    let mut cache = ACCOUNT_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(cached) = cache.as_ref()
        && cached.path == path
        && cached.stamp == stamp
    {
        return cached.account.clone();
    }
    let account = if stamp.is_some() {
        read_valid(&path).map(|file| account_of(&file.tokens))
    } else {
        None
    };
    *cache = Some(CachedAccount {
        path,
        stamp,
        account: account.clone(),
    });
    account
}

/// 로그인 정보를 지웁니다 (로그아웃). 로그인하지 않았어도 오류가 아닙니다.
///
/// 이 PC에 둔 정보만 지웁니다. Codex CLI의 로그인에는 영향이 없습니다.
pub fn sign_out(paths: &AppPaths) -> io::Result<()> {
    let result = match fs::remove_file(auth_file(paths)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    };
    forget_cached_account();
    result
}

/// 새로 받은 토큰을 저장합니다. 다른 프로그램이 반쯤 쓴 파일을 읽지 않게 임시 파일에 쓴 뒤 바꿔 넣습니다.
pub(crate) fn store(paths: &AppPaths, tokens: &Tokens) -> io::Result<()> {
    let path = auth_file(paths);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = AuthFile {
        tokens: tokens.clone(),
        last_refresh: now_secs(),
    };
    let text = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, text)?;
    restrict_to_owner(&temp);
    let result = fs::rename(&temp, &path);
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    forget_cached_account();
    result
}

/// 요청에 쓸 토큰. access token이 곧 만료되면 먼저 새로 받습니다.
pub(crate) fn fresh_tokens(
    paths: &AppPaths,
    token_url: &str,
    timeout: Duration,
) -> Result<Tokens, LlmError> {
    let _guard = REFRESH_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let file = read_valid(&auth_file(paths)).ok_or(LlmError::SignedOut)?;
    if !needs_refresh(&file, now_secs()) {
        return Ok(file.tokens);
    }
    tracing::info!("Codex access token is about to expire; refreshing it");
    refresh(paths, token_url, file.tokens, timeout)
}

/// 서버가 `used` 토큰을 401로 거절했을 때 새 토큰을 받습니다.
///
/// 그사이 다른 요청이 이미 새로 받았으면(저장된 access token이 `used`와 다르면) 그것을 씁니다.
pub(crate) fn refresh_rejected(
    paths: &AppPaths,
    token_url: &str,
    used: &Tokens,
    timeout: Duration,
) -> Result<Tokens, LlmError> {
    let _guard = REFRESH_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let file = read_valid(&auth_file(paths)).ok_or(LlmError::SignedOut)?;
    if file.tokens.access_token != used.access_token {
        return Ok(file.tokens);
    }
    tracing::info!("Codex rejected the access token; refreshing it");
    refresh(paths, token_url, file.tokens, timeout)
}

/// 토큰 응답의 ID 토큰에서 ChatGPT 계정 ID를 꺼냅니다.
pub(crate) fn account_id_of(id_token: &str) -> Option<String> {
    let claims = jwt_claims(id_token)?;
    non_empty(claims.get(AUTH_CLAIM)?.get("chatgpt_account_id")?)
}

/// 토큰에 적힌 계정 정보 (이메일, 요금제)
pub(crate) fn account_of(tokens: &Tokens) -> CodexAccount {
    let id = jwt_claims(&tokens.id_token);
    let access = jwt_claims(&tokens.access_token);
    let email = id
        .as_ref()
        .and_then(|claims| claims.get("email"))
        .and_then(non_empty)
        .or_else(|| {
            access
                .as_ref()
                .and_then(|claims| claims.get(PROFILE_CLAIM)?.get("email"))
                .and_then(non_empty)
        });
    let plan = [&id, &access]
        .into_iter()
        .find_map(|claims| non_empty(claims.as_ref()?.get(AUTH_CLAIM)?.get("chatgpt_plan_type")?));
    CodexAccount { email, plan }
}

/// refresh token으로 새 토큰을 받아 저장합니다. 다시 받을 수 없는 로그인이면 지우고 `SignedOut`입니다.
fn refresh(
    paths: &AppPaths,
    token_url: &str,
    tokens: Tokens,
    timeout: Duration,
) -> Result<Tokens, LlmError> {
    if tokens.refresh_token.trim().is_empty() {
        return Err(expire_login(paths, "there is no refresh token"));
    }
    let reply = match request_refresh(token_url, &tokens.refresh_token, timeout) {
        Ok(reply) => reply,
        Err(RefreshFailure::Dead(reason)) => return Err(expire_login(paths, &reason)),
        Err(RefreshFailure::Other(error)) => return Err(error),
    };
    let account_id = reply
        .id_token
        .as_deref()
        .and_then(account_id_of)
        .unwrap_or(tokens.account_id);
    let refreshed = Tokens {
        id_token: reply.id_token.unwrap_or(tokens.id_token),
        access_token: reply.access_token,
        refresh_token: reply.refresh_token.unwrap_or(tokens.refresh_token),
        account_id,
    };
    // 저장하지 못해도 이번 요청에는 새 토큰을 씁니다. 옛 refresh token은 이미 쓸 수 없으므로
    // 다음에 앱을 켜면 다시 로그인해야 할 수 있습니다.
    if let Err(error) = store(paths, &refreshed) {
        tracing::error!("could not save the refreshed Codex login: {error}");
    }
    Ok(refreshed)
}

/// 다시 받을 수 없는 로그인을 지우고 `SignedOut`을 돌려줍니다.
fn expire_login(paths: &AppPaths, reason: &str) -> LlmError {
    tracing::warn!("Codex login can no longer be refreshed ({reason}); signing out");
    if let Err(error) = sign_out(paths) {
        tracing::warn!("could not remove the expired Codex login: {error}");
    }
    LlmError::SignedOut
}

/// 토큰을 다시 받은 응답
#[derive(Deserialize)]
struct RefreshReply {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

enum RefreshFailure {
    /// refresh token이 만료·취소돼 다시 로그인해야 합니다. 영어 이유를 담습니다.
    Dead(String),
    /// 네트워크·서버 문제 등. 나중에 다시 하면 될 수 있습니다.
    Other(LlmError),
}

fn request_refresh(
    token_url: &str,
    refresh_token: &str,
    timeout: Duration,
) -> Result<RefreshReply, RefreshFailure> {
    let body = json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    })
    .to_string();
    let mut response = http_agent(timeout)
        .post(token_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send(body.as_bytes())
        .map_err(|e| RefreshFailure::Other(network_error(e)))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| {
        RefreshFailure::Other(LlmError::Network(format!(
            "failed to read response body: {e}"
        )))
    })?;
    if (200..300).contains(&status) {
        // 응답에는 토큰이 들어 있어 오류(로그에 남음)에 본문을 싣지 않습니다.
        return serde_json::from_str(&text).map_err(|e| {
            RefreshFailure::Other(LlmError::BadResponse(format!(
                "invalid token response ({})",
                crate::json_error_summary(&e)
            )))
        });
    }
    let code = oauth_error_code(&text);
    let message = hide_secret(error_message(&text), refresh_token);
    let dead = status == 401
        || code
            .as_deref()
            .is_some_and(|code| DEAD_REFRESH_CODES.contains(&code));
    if dead {
        Err(RefreshFailure::Dead(format!("HTTP {status}: {message}")))
    } else {
        Err(RefreshFailure::Other(LlmError::Http { status, message }))
    }
}

/// OAuth 오류 응답의 오류 코드 (`{"error": "invalid_grant"}` 또는 `{"error": {"code": "..."}}`), 소문자
fn oauth_error_code(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body.trim()).ok()?;
    let code = match value.get("error") {
        Some(Value::String(code)) => Some(code.as_str()),
        Some(Value::Object(error)) => error.get("code").and_then(Value::as_str),
        _ => None,
    }
    .or_else(|| value.get("code").and_then(Value::as_str))?;
    Some(code.trim().to_ascii_lowercase())
}

/// 지금 토큰을 새로 받아야 하는지
fn needs_refresh(file: &AuthFile, now: u64) -> bool {
    match expires_at(&file.tokens.access_token) {
        Some(expires) => now.saturating_add(REFRESH_MARGIN.as_secs()) >= expires,
        None => now >= file.last_refresh.saturating_add(REFRESH_INTERVAL.as_secs()),
    }
}

/// access token의 만료 시각 (유닉스 초)
fn expires_at(token: &str) -> Option<u64> {
    jwt_claims(token)?.get("exp")?.as_u64()
}

/// JWT의 클레임(가운데 부분)을 읽습니다. 서명은 확인하지 않습니다(서버가 확인함).
fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD_INDIFFERENT.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn non_empty(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// 로그인 정보를 읽습니다. 없거나, 망가졌거나, 꼭 필요한 값이 비어 있으면 `None`입니다.
fn read_valid(path: &Path) -> Option<AuthFile> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!("could not read the Codex login: {error}");
            return None;
        }
    };
    let file: AuthFile = match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(
                "the Codex login file is damaged ({})",
                crate::json_error_summary(&error)
            );
            return None;
        }
    };
    let tokens = &file.tokens;
    (!tokens.access_token.trim().is_empty() && !tokens.account_id.trim().is_empty()).then_some(file)
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn forget_cached_account() {
    *ACCOUNT_CACHE.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// 로그인 정보 파일을 나만 읽을 수 있게 합니다. Windows의 데이터 폴더는 처음부터 사용자 전용입니다.
#[cfg(unix)]
fn restrict_to_owner(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!("could not restrict the Codex login file: {error}");
    }
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) {}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_server::{body_of, json, serve};

    /// 클레임을 담은 가짜 JWT (서명은 확인하지 않으므로 아무 값)
    pub(crate) fn jwt(claims: Value) -> String {
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!(
            "{}.{}.sig",
            engine.encode(br#"{"alg":"none"}"#),
            engine.encode(claims.to_string())
        )
    }

    /// `expires` 초에 만료되는 가짜 토큰 묶음
    pub(crate) fn tokens(expires: u64) -> Tokens {
        Tokens {
            id_token: jwt(json!({
                "email": "me@example.com",
                AUTH_CLAIM: { "chatgpt_account_id": "acc-1", "chatgpt_plan_type": "plus" },
            })),
            access_token: jwt(json!({ "exp": expires })),
            refresh_token: "rt-old".into(),
            account_id: "acc-1".into(),
        }
    }

    pub(crate) fn hour_from_now() -> u64 {
        now_secs() + 60 * 60
    }

    fn fixture() -> (tempfile::TempDir, AppPaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        (dir, paths)
    }

    /// 요청을 받지 않는 주소 (토큰을 새로 받으려 하면 실패함)
    const NO_SERVER: &str = "http://127.0.0.1:1/oauth/token";

    #[test]
    fn stored_login_gives_the_account_and_sign_out_forgets_it() {
        let (_dir, paths) = fixture();
        assert_eq!(account(&paths), None);
        assert!(matches!(
            fresh_tokens(&paths, NO_SERVER, Duration::from_secs(5)),
            Err(LlmError::SignedOut)
        ));

        store(&paths, &tokens(hour_from_now())).unwrap();
        assert_eq!(
            account(&paths),
            Some(CodexAccount {
                email: Some("me@example.com".into()),
                plan: Some("plus".into()),
            })
        );
        assert!(
            auth_file(&paths).starts_with(&paths.data_dir),
            "로그인 정보는 설정 폴더가 아니라 데이터 폴더에 둡니다"
        );

        sign_out(&paths).unwrap();
        assert_eq!(account(&paths), None);
        sign_out(&paths).expect("로그인하지 않았을 때 로그아웃해도 오류가 아닙니다");
    }

    #[test]
    fn damaged_or_incomplete_login_counts_as_signed_out() {
        let (_dir, paths) = fixture();
        let path = auth_file(&paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(account(&paths), None);

        let mut missing_account = tokens(hour_from_now());
        missing_account.account_id.clear();
        store(&paths, &missing_account).unwrap();
        assert_eq!(account(&paths), None);
    }

    #[test]
    fn tokens_are_not_printed() {
        let shown = format!("{:?}", tokens(1));
        assert!(
            !shown.contains("rt-old") && !shown.contains("acc-1"),
            "{shown}"
        );
    }

    #[test]
    fn valid_token_is_used_without_refreshing() {
        let (_dir, paths) = fixture();
        let stored = tokens(hour_from_now());
        store(&paths, &stored).unwrap();
        let used = fresh_tokens(&paths, NO_SERVER, Duration::from_secs(5)).unwrap();
        assert_eq!(used, stored);
    }

    #[test]
    fn expiring_token_is_refreshed_and_saved() {
        let (_dir, paths) = fixture();
        store(&paths, &tokens(now_secs() + 60)).unwrap();
        let new_access = jwt(json!({ "exp": hour_from_now() }));
        let new_id = jwt(json!({
            "email": "me@example.com",
            AUTH_CLAIM: { "chatgpt_account_id": "acc-2", "chatgpt_plan_type": "pro" },
        }));
        let (base, server) = serve(vec![json(
            "200 OK",
            json!({
                "id_token": new_id,
                "access_token": new_access,
                "refresh_token": "rt-new",
            })
            .to_string(),
        )]);

        let used = fresh_tokens(
            &paths,
            &format!("{base}/oauth/token"),
            Duration::from_secs(10),
        )
        .unwrap();
        let raw = server.join().unwrap().remove(0);
        assert!(raw.starts_with("POST /oauth/token HTTP/1.1"), "{raw}");
        let body = body_of(&raw);
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["client_id"], CLIENT_ID);
        assert_eq!(body["refresh_token"], "rt-old");

        assert_eq!(used.access_token, new_access);
        assert_eq!(
            used.refresh_token, "rt-new",
            "바뀐 refresh token을 써야 합니다"
        );
        assert_eq!(used.account_id, "acc-2");
        // 새로 받은 토큰은 저장돼 다음 요청은 다시 받지 않습니다.
        let again = fresh_tokens(&paths, NO_SERVER, Duration::from_secs(5)).unwrap();
        assert_eq!(again, used);
        assert_eq!(account(&paths).unwrap().plan.as_deref(), Some("pro"));
    }

    #[test]
    fn dead_refresh_token_signs_out() {
        for (status, body) in [
            ("400 Bad Request", r#"{"error":"invalid_grant"}"#),
            (
                "401 Unauthorized",
                r#"{"error":{"code":"refresh_token_expired","message":"expired"}}"#,
            ),
            (
                "400 Bad Request",
                r#"{"error":{"code":"refresh_token_reused","message":"reused"}}"#,
            ),
        ] {
            let (_dir, paths) = fixture();
            store(&paths, &tokens(now_secs())).unwrap();
            let (base, server) = serve(vec![json(status, body)]);
            let result = fresh_tokens(
                &paths,
                &format!("{base}/oauth/token"),
                Duration::from_secs(10),
            );
            server.join().unwrap();
            assert!(
                matches!(result, Err(LlmError::SignedOut)),
                "{status} {body}: {result:?}"
            );
            assert_eq!(
                account(&paths),
                None,
                "{status} {body}: 다시 받을 수 없는 로그인은 지워야 합니다"
            );
        }
    }

    #[test]
    fn temporary_refresh_failure_keeps_the_login() {
        let (_dir, paths) = fixture();
        store(&paths, &tokens(now_secs())).unwrap();
        let (base, server) = serve(vec![json(
            "503 Service Unavailable",
            r#"{"error":{"message":"try later"}}"#,
        )]);
        let result = fresh_tokens(
            &paths,
            &format!("{base}/oauth/token"),
            Duration::from_secs(10),
        );
        server.join().unwrap();
        assert!(
            matches!(&result, Err(LlmError::Http { status: 503, message }) if message == "try later"),
            "{result:?}"
        );
        assert!(
            account(&paths).is_some(),
            "잠깐의 서버 문제로 로그인을 지우면 안 됩니다"
        );
    }

    #[test]
    fn rejected_token_uses_a_token_another_request_already_refreshed() {
        let (_dir, paths) = fixture();
        let used = tokens(hour_from_now());
        let mut newer = used.clone();
        newer.access_token = jwt(json!({ "exp": hour_from_now() + 1 }));
        store(&paths, &newer).unwrap();
        let got = refresh_rejected(&paths, NO_SERVER, &used, Duration::from_secs(5)).unwrap();
        assert_eq!(got, newer);
    }

    #[test]
    fn token_without_expiry_is_refreshed_after_eight_days() {
        let file = |last_refresh| AuthFile {
            tokens: Tokens {
                access_token: "opaque".into(),
                ..tokens(0)
            },
            last_refresh,
        };
        let now = 10 * REFRESH_INTERVAL.as_secs();
        assert!(!needs_refresh(&file(now - 60), now));
        assert!(needs_refresh(&file(now - REFRESH_INTERVAL.as_secs()), now));
    }

    #[test]
    fn account_id_comes_from_the_id_token() {
        assert_eq!(account_id_of(&tokens(0).id_token).as_deref(), Some("acc-1"));
        assert_eq!(account_id_of("not-a-jwt"), None);
        assert_eq!(account_id_of(&jwt(json!({ "email": "x" }))), None);
    }
}
