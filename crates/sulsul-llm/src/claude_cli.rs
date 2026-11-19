//! Claude Code CLI로 다듬기: 이 PC에 설치된 `claude`를 실행해, 그 CLI가 로그인한 Claude 계정으로 다듬습니다.
//!
//! 술술이 Claude 계정에 직접 로그인하지 않고, 사용자가 설치하고 로그인해 둔 Claude Code CLI를 빌려 씁니다.
//! 다듬을 때마다 `claude -p --output-format json`을 한 번 띄워, 받아쓴 글은 표준 입력으로, 지시문(고른
//! 프롬프트)은 파일(`--system-prompt-file`)로 넘기고 JSON 결과의 `result`를 받습니다. CLI를 띄우는 데
//! 몇 초가 더 걸립니다.
//!
//! 다듬기에 끼어들지 않게 도구(`--tools ""`), 세션 저장, CLAUDE.md·훅·MCP 서버·플러그인 같은 사용자 설정
//! (`--safe-mode`, `--strict-mcp-config`)을 끄고 실행합니다. 로그인과 모델 고르기는 그대로 됩니다.
//!
//! 설정은 `[postprocess.processors.llm_claude_cli]`에 둡니다: 목록에 추가했는지(`registered`), 모델,
//! 추론 단계, CLI 위치(`path`, 비우면 자동으로 찾음).

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use sulsul_core::config::{PostProcessConfig, ReasoningEffort};
use sulsul_core::toml::Value as TomlValue;
use sulsul_core::{AppPaths, CancelToken};

use crate::CLAUDE_CLI_PROCESSOR_ID;
use crate::client::{
    ChatReply, ConnectionReply, LlmError, POLL_INTERVAL, TEST_PROMPT, timeout_error, truncate,
};

/// 목록에 추가했는지를 담는 키
pub const REGISTERED_KEY: &str = "registered";
/// 고른 모델을 담는 키
pub const MODEL_KEY: &str = "model";
/// 고른 추론 단계를 담는 키
pub const REASONING_EFFORT_KEY: &str = "reasoning_effort";
/// CLI 위치를 담는 키 (비우면 자동으로 찾음)
pub const PATH_KEY: &str = "path";

/// 모델을 고르지 않았을 때 쓰는 모델 (CLI의 별명. 그 계열의 최신 모델을 가리킴)
pub const DEFAULT_MODEL: &str = "sonnet";

/// 화면에서 바로 고를 수 있게 보여 줄 모델 별명
pub const MODEL_ALIASES: [&str; 4] = ["sonnet", "opus", "haiku", "fable"];

/// 다듬기·연결 테스트 한 번의 제한 시간. CLI를 띄우는 시간까지 넉넉히 줍니다.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

/// 설치·로그인 상태 확인의 제한 시간
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(30);

/// CLI를 찾은 결과를 다시 쓰는 시간. 목록을 그릴 때마다 PATH를 뒤지지 않으려는 것입니다.
const FIND_CACHE_TTL: Duration = Duration::from_secs(5);

/// 연결 테스트에서 user 메시지로 보내는 글
const TEST_INPUT: &str = "ping";

/// 술술을 Claude Code 안에서 띄웠을 때 물려받는, 바깥 세션을 알리는 환경 변수. CLI에 넘기지 않습니다.
const NESTED_SESSION_ENV: [&str; 2] = ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"];

/// 다듬기 실행 인자 가운데 늘 같은 것
const FIXED_ARGS: [&str; 9] = [
    "-p",
    "--output-format",
    "json",
    "--no-session-persistence",
    "--safe-mode",
    "--strict-mcp-config",
    "--disable-slash-commands",
    "--tools",
    "",
];

// ── 설정 ────────────────────────────────────────────────────────────────

/// 사용 가능 모델 목록에 추가했는지
pub fn is_registered(config: &PostProcessConfig) -> bool {
    config
        .processors
        .get(CLAUDE_CLI_PROCESSOR_ID)
        .and_then(|table| table.get(REGISTERED_KEY))
        .and_then(TomlValue::as_bool)
        .unwrap_or(false)
}

/// 목록에 추가하거나 뺍니다. 빼도 키를 지우지 않고 `false`로 적습니다(지우면 파일의 옛 값이 되살아남).
pub fn set_registered(config: &mut PostProcessConfig, registered: bool) {
    table(config).insert(REGISTERED_KEY.to_owned(), TomlValue::Boolean(registered));
}

/// 설정 파일에 적힌 모델 이름 그대로 (없으면 빈 문자열). 화면의 입력 칸에 씁니다.
pub fn model_setting(config: &PostProcessConfig) -> &str {
    string_setting(config, MODEL_KEY).unwrap_or_default()
}

/// 고른 모델. 비어 있으면 [`DEFAULT_MODEL`]입니다.
pub fn chosen_model(config: &PostProcessConfig) -> &str {
    string_setting(config, MODEL_KEY)
        .filter(|model| !model.is_empty())
        .unwrap_or(DEFAULT_MODEL)
}

pub fn set_model(config: &mut PostProcessConfig, model: &str) {
    write_string(config, MODEL_KEY, model.trim());
}

/// 고른 추론 단계. 없거나 모르는 값이면 기본값입니다.
pub fn chosen_effort(config: &PostProcessConfig) -> ReasoningEffort {
    string_setting(config, REASONING_EFFORT_KEY)
        .and_then(|name| {
            ReasoningEffort::ALL
                .into_iter()
                .find(|effort| effort_setting_name(*effort).eq_ignore_ascii_case(name))
        })
        .unwrap_or_default()
}

/// 고른 추론 단계를 적습니다. 기본값도 지우지 않고 `"default"`로 적습니다.
pub fn set_effort(config: &mut PostProcessConfig, effort: ReasoningEffort) {
    write_string(config, REASONING_EFFORT_KEY, effort_setting_name(effort));
}

/// CLI가 받는 추론 단계인지. Claude Code CLI는 생각을 끄는 단계가 없습니다.
pub fn accepts_effort(effort: ReasoningEffort) -> bool {
    effort != ReasoningEffort::None
}

/// 설정에 적은 CLI 위치 (비어 있으면 자동으로 찾음)
pub fn path_setting(config: &PostProcessConfig) -> &str {
    string_setting(config, PATH_KEY).unwrap_or_default()
}

pub fn set_path(config: &mut PostProcessConfig, path: &str) {
    write_string(config, PATH_KEY, path.trim());
}

fn table(config: &mut PostProcessConfig) -> &mut sulsul_core::toml::Table {
    config
        .processors
        .entry(CLAUDE_CLI_PROCESSOR_ID.to_owned())
        .or_default()
}

fn string_setting<'a>(config: &'a PostProcessConfig, key: &str) -> Option<&'a str> {
    config
        .processors
        .get(CLAUDE_CLI_PROCESSOR_ID)
        .and_then(|table| table.get(key))
        .and_then(TomlValue::as_str)
        .map(str::trim)
}

fn write_string(config: &mut PostProcessConfig, key: &str, value: &str) {
    table(config).insert(key.to_owned(), TomlValue::String(value.to_owned()));
}

/// 설정 파일에 적는 추론 단계 이름 (`ApiProfile::reasoning_effort`와 같은 이름)
fn effort_setting_name(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Default => "default",
        ReasoningEffort::None => "none",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
    }
}

/// CLI의 `--effort` 값. 기본값과 CLI가 받지 않는 단계는 넘기지 않습니다.
fn effort_arg(effort: ReasoningEffort) -> Option<&'static str> {
    match effort {
        ReasoningEffort::Default | ReasoningEffort::None => None,
        ReasoningEffort::Low => Some("low"),
        ReasoningEffort::Medium => Some("medium"),
        ReasoningEffort::High => Some("high"),
    }
}

// ── CLI 찾기 ────────────────────────────────────────────────────────────

/// 마지막으로 찾은 결과 (CLI 위치 설정 값, 찾은 시각, 찾은 파일)
static FIND_CACHE: Mutex<Option<(String, Instant, Option<PathBuf>)>> = Mutex::new(None);

/// 쓸 CLI 파일. 설정에 위치를 적었으면 그 파일을, 아니면 PATH와 흔한 설치 위치에서 찾습니다.
/// 결과를 잠깐 기억해 두므로 그릴 때마다 불러도 됩니다.
pub fn find_cli(config: &PostProcessConfig) -> Option<PathBuf> {
    let custom = path_setting(config);
    let mut cache = FIND_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((key, at, found)) = cache.as_ref()
        && key == custom
        && at.elapsed() < FIND_CACHE_TTL
    {
        return found.clone();
    }
    let found = locate(custom);
    *cache = Some((custom.to_owned(), Instant::now(), found.clone()));
    found
}

/// 기억해 둔 결과 없이 다시 찾습니다 (화면에서 "다시 확인"을 눌렀을 때).
pub fn find_cli_now(config: &PostProcessConfig) -> Option<PathBuf> {
    *FIND_CACHE.lock().unwrap_or_else(PoisonError::into_inner) = None;
    find_cli(config)
}

fn locate(custom: &str) -> Option<PathBuf> {
    if !custom.is_empty() {
        let path = PathBuf::from(custom);
        return path.is_file().then_some(path);
    }
    let names = cli_file_names();
    let on_path = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    on_path
        .iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .chain(common_install_paths())
        .find(|candidate| candidate.is_file())
}

/// PATH에서 찾을 파일 이름. Windows는 설치 프로그램(`claude.exe`)과 npm(`claude.cmd`)으로 설치한 것입니다.
fn cli_file_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    }
}

/// PATH에 없을 때 볼 흔한 설치 위치 (술술을 PATH가 짧은 곳에서 띄운 경우)
fn common_install_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    if let Some(home) = home {
        let bin = PathBuf::from(home).join(".local").join("bin");
        let name = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        paths.push(bin.join(name));
    }
    if cfg!(windows)
        && let Some(appdata) = std::env::var_os("APPDATA")
    {
        paths.push(PathBuf::from(appdata).join("npm").join("claude.cmd"));
    }
    paths
}

// ── 상태 확인 ───────────────────────────────────────────────────────────

/// CLI의 설치·로그인 상태
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliStatus {
    /// 확인한 CLI 파일
    pub path: PathBuf,
    /// CLI 판 (예: `2.1.328`). 알아내지 못했으면 `None`
    pub version: Option<String>,
    pub login: CliLogin,
}

/// CLI의 로그인 상태 (`claude auth status --json`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliLogin {
    LoggedIn {
        /// 로그인 방식 (예: `claude.ai`, API 키)
        method: Option<String>,
        /// 요금제 (예: `max`, `pro`)
        plan: Option<String>,
        email: Option<String>,
    },
    LoggedOut,
    /// 확인하지 못했습니다 (오래된 CLI 등). 다듬을 때 알게 됩니다.
    Unknown,
}

/// CLI의 판과 로그인 상태를 확인합니다. 블로킹 함수입니다.
pub fn check(
    paths: &AppPaths,
    cli: &Path,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<CliStatus, LlmError> {
    let deadline = Instant::now() + timeout;
    let work_dir = work_dir(paths)?;
    let version = run_cli(
        cli,
        &["--version".into()],
        &work_dir,
        None,
        left(deadline, timeout)?,
        cancel,
    )?;
    let version = version
        .success
        .then(|| parse_version(&version.stdout))
        .flatten();
    let args: Vec<OsString> = ["auth", "status", "--json"].map(OsString::from).into();
    let auth = run_cli(
        cli,
        &args,
        &work_dir,
        None,
        left(deadline, timeout)?,
        cancel,
    )?;
    // 로그인하지 않았으면 0이 아닌 값으로 끝나지만 JSON은 똑같이 나옵니다.
    let login = parse_login(&auth.stdout);
    Ok(CliStatus {
        path: cli.to_path_buf(),
        version,
        login,
    })
}

/// `claude --version`의 출력(예: `2.1.328 (Claude Code)`)에서 판만 꺼냅니다.
fn parse_version(stdout: &str) -> Option<String> {
    let first = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let version = first.split_whitespace().next()?;
    Some(version.to_owned())
}

fn parse_login(stdout: &str) -> CliLogin {
    let Some(status) = last_json_object(stdout) else {
        return CliLogin::Unknown;
    };
    let text = |key: &str| {
        status
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    match status.get("loggedIn").and_then(Value::as_bool) {
        Some(true) => CliLogin::LoggedIn {
            method: text("authMethod"),
            plan: text("subscriptionType"),
            email: text("email"),
        },
        Some(false) => CliLogin::LoggedOut,
        None => CliLogin::Unknown,
    }
}

// ── 다듬기 ──────────────────────────────────────────────────────────────

/// 요청을 보낼 CLI와 모델
#[derive(Debug, Clone, Copy)]
pub struct CliTarget<'a> {
    /// 실행할 CLI 파일 ([`find_cli`])
    pub cli: &'a Path,
    /// 모델 이름이나 별명 (비우면 CLI 기본 모델)
    pub model: &'a str,
    pub effort: ReasoningEffort,
}

/// 받아쓴 글을 지시문에 따라 다듬어 달라고 CLI에 요청합니다. 블로킹 함수입니다.
///
/// `instructions`는 시스템 프롬프트로, `text`는 user 메시지로 넘깁니다.
pub fn chat(
    paths: &AppPaths,
    target: CliTarget<'_>,
    instructions: &str,
    text: &str,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<ChatReply, LlmError> {
    request(
        paths,
        target,
        instructions.trim(),
        text.trim(),
        timeout,
        cancel,
    )
}

/// CLI와 모델이 쓸 수 있는지 아주 짧은 요청 하나로 확인합니다. 블로킹 함수입니다.
pub fn test_connection(
    paths: &AppPaths,
    target: CliTarget<'_>,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<ConnectionReply, LlmError> {
    request(paths, target, TEST_PROMPT, TEST_INPUT, timeout, cancel)
}

fn request(
    paths: &AppPaths,
    target: CliTarget<'_>,
    instructions: &str,
    text: &str,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<ConnectionReply, LlmError> {
    let started = Instant::now();
    let work_dir = work_dir(paths)?;
    let prompt_file = PromptFile::write(&work_dir, instructions)?;
    if target.effort == ReasoningEffort::None {
        tracing::warn!("Claude Code CLI cannot turn thinking off; using its default effort");
    }
    let args = request_args(target.model, target.effort, prompt_file.path());
    tracing::debug!(model = target.model, reasoning_effort = ?target.effort, "sending Claude Code CLI request");
    let finished = run_cli(
        target.cli,
        &args,
        &work_dir,
        Some(text.to_owned()),
        timeout,
        cancel,
    )?;
    let (reply, model) = interpret(&finished)
        .inspect_err(|e| tracing::debug!("Claude Code CLI request failed: {e}"))?;
    Ok(ConnectionReply {
        text: reply,
        elapsed: started.elapsed(),
        model,
    })
}

/// 다듬기 실행 인자
fn request_args(model: &str, effort: ReasoningEffort, prompt_file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = FIXED_ARGS.iter().map(OsString::from).collect();
    args.push("--system-prompt-file".into());
    args.push(prompt_file.as_os_str().to_owned());
    let model = model.trim();
    if !model.is_empty() {
        args.push("--model".into());
        args.push(model.into());
    }
    if let Some(level) = effort_arg(effort) {
        args.push("--effort".into());
        args.push(level.into());
    }
    args
}

/// CLI를 실행하는 폴더. 프로젝트 폴더의 설정을 읽지 않게, 데이터 폴더 안의 빈 폴더에서 실행합니다.
fn work_dir(paths: &AppPaths) -> Result<PathBuf, LlmError> {
    let dir = paths.data_dir.join("claude-cli");
    std::fs::create_dir_all(&dir)
        .map_err(|e| LlmError::CliFailed(format!("could not prepare {}: {e}", dir.display())))?;
    Ok(dir)
}

/// 지시문을 넘기는 임시 파일. 버리면 지웁니다.
///
/// 지시문은 여러 줄이고 따옴표가 섞여, 명령줄로 넘기면 npm으로 설치한 CLI(`claude.cmd`)에서 깨집니다.
struct PromptFile(PathBuf);

impl PromptFile {
    fn write(dir: &Path, text: &str) -> Result<Self, LlmError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "prompt-{}-{}.txt",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = dir.join(name);
        std::fs::write(&path, text)
            .map_err(|e| LlmError::CliFailed(format!("could not write the prompt file: {e}")))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PromptFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 끝난 CLI 실행
struct Finished {
    success: bool,
    stdout: String,
    stderr: String,
}

/// CLI를 실행하고 끝나기를 기다립니다. `cancel`이 켜지거나 `timeout`이 지나면 프로세스를 끝내고 돌아옵니다.
fn run_cli(
    cli: &Path,
    args: &[OsString],
    work_dir: &Path,
    input: Option<String>,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<Finished, LlmError> {
    let mut command = Command::new(cli);
    command
        .args(args)
        .current_dir(work_dir)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in NESTED_SESSION_ENV {
        command.env_remove(name);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// 콘솔 창을 띄우지 않습니다.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => LlmError::CliNotFound,
        _ => LlmError::CliFailed(format!("could not start {}: {e}", cli.display())),
    })?;

    let writer = match (input, child.stdin.take()) {
        (Some(text), Some(mut stdin)) => Some(spawn_named("sulsul-claude-cli-in", move || {
            // 받아쓴 글은 로그에 남기지 않습니다.
            if let Err(error) = stdin.write_all(text.as_bytes()) {
                tracing::debug!("could not write to the Claude Code CLI: {error}");
            }
        })),
        _ => None,
    };
    let stdout = child
        .stdout
        .take()
        .map(|pipe| spawn_reader("sulsul-claude-cli-out", pipe));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| spawn_reader("sulsul-claude-cli-err", pipe));
    if writer.as_ref().is_some_and(Result::is_err)
        || stdout.as_ref().is_some_and(Result::is_err)
        || stderr.as_ref().is_some_and(Result::is_err)
    {
        stop(&mut child);
        return Err(LlmError::CliFailed(
            "could not start a worker thread".into(),
        ));
    }

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                stop(&mut child);
                return Err(LlmError::CliFailed(format!(
                    "could not wait for the CLI: {error}"
                )));
            }
        }
        if cancel.is_cancelled() {
            stop(&mut child);
            return Err(LlmError::Cancelled);
        }
        if started.elapsed() >= timeout {
            tracing::warn!("Claude Code CLI did not finish within {timeout:?}; stopping it");
            stop(&mut child);
            return Err(timeout_error(timeout));
        }
        thread::sleep(POLL_INTERVAL);
    };
    let collect = |reader: Option<Result<thread::JoinHandle<String>, std::io::Error>>| {
        reader
            .and_then(Result::ok)
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    };
    Ok(Finished {
        success: status.success(),
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

fn spawn_named(
    name: &str,
    work: impl FnOnce() + Send + 'static,
) -> Result<thread::JoinHandle<()>, std::io::Error> {
    thread::Builder::new().name(name.to_owned()).spawn(work)
}

fn spawn_reader(
    name: &str,
    mut pipe: impl Read + Send + 'static,
) -> Result<thread::JoinHandle<String>, std::io::Error> {
    thread::Builder::new().name(name.to_owned()).spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// `deadline`까지 남은 시간. 다 지났으면 시간 초과입니다.
fn left(deadline: Instant, timeout: Duration) -> Result<Duration, LlmError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(timeout_error(timeout))
    } else {
        Ok(remaining)
    }
}

/// CLI 출력을 (답, 응답한 모델)이나 오류로 바꿉니다.
///
/// 다듬은 글이 로그에 남지 않게, 읽지 못한 출력의 내용은 오류에 싣지 않습니다.
fn interpret(finished: &Finished) -> Result<(String, Option<String>), LlmError> {
    let Some(result) = last_json_object(&finished.stdout)
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("result"))
    else {
        if finished.success {
            return Err(LlmError::BadResponse(
                "the Claude Code CLI printed no result".into(),
            ));
        }
        // 오류 원인은 보통 표준 오류의 마지막 줄에 있습니다.
        let reason = finished
            .stderr
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .unwrap_or("the CLI exited with an error");
        return Err(LlmError::CliFailed(truncate(reason)));
    };

    let message = result
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let is_error = result
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let subtype = result
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !is_error && subtype == "success" {
        return Ok((message.trim().to_owned(), answered_model(&result)));
    }
    if !is_error {
        // 차례 수 한도 등으로 답 없이 끝났습니다.
        return Err(LlmError::CliFailed(format!(
            "the CLI stopped early ({subtype})"
        )));
    }

    let status = result
        .get("api_error_status")
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok());
    let message = truncate(message);
    let lower = message.to_ascii_lowercase();
    if lower.contains("not logged in") || lower.contains("/login") {
        return Err(LlmError::SignedOut);
    }
    match status {
        Some(404) => Err(LlmError::ModelNotFound(message)),
        Some(status) => Err(LlmError::Http { status, message }),
        None => Err(LlmError::CliFailed(message)),
    }
}

/// 결과의 `modelUsage`에서 응답한 모델. 여러 모델을 썼으면 가장 많이 쓴(출력 토큰) 모델입니다.
fn answered_model(result: &Value) -> Option<String> {
    let usage = result.get("modelUsage")?.as_object()?;
    usage
        .iter()
        .max_by_key(|(_, used)| {
            used.get("outputTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        })
        .map(|(model, _)| model.clone())
}

/// 출력의 JSON 객체. 출력 전체가 (여러 줄로 찍힌) JSON이면 그것을, 아니면 마지막 JSON 객체 줄을 씁니다
/// (앞에 다른 줄이 섞여 있어도 됩니다).
fn last_json_object(stdout: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(stdout.trim())
        && value.is_object()
    {
        return Some(value);
    }
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .find_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(success: bool, stdout: &str, stderr: &str) -> Finished {
        Finished {
            success,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
        }
    }

    fn result_line(is_error: bool, result: &str, status: Option<u16>) -> String {
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": is_error,
            "result": result,
            "api_error_status": status,
            "modelUsage": {
                "claude-haiku-5-5": { "outputTokens": 3 },
                "claude-sonnet-5-5": { "outputTokens": 40 },
            },
        })
        .to_string()
    }

    #[test]
    fn successful_result_gives_the_answer_and_the_model() {
        let out = finished(
            true,
            &result_line(false, " 이번 회의는 수요일입니다. \n", None),
            "",
        );
        assert_eq!(
            interpret(&out).unwrap(),
            (
                "이번 회의는 수요일입니다.".to_owned(),
                Some("claude-sonnet-5-5".to_owned())
            ),
            "출력 토큰을 가장 많이 쓴 모델이 응답한 모델입니다"
        );
    }

    #[test]
    fn cli_errors_are_told_apart() {
        let not_logged_in = finished(
            false,
            &result_line(true, "Not logged in · Please run /login", None),
            "",
        );
        assert_eq!(interpret(&not_logged_in), Err(LlmError::SignedOut));

        let bad_model = finished(
            false,
            &format!(
                "[claude-code:unrecognized_model] {{\"model\":\"x\"}}\n{}",
                result_line(
                    true,
                    "There's an issue with the selected model (x).",
                    Some(404)
                )
            ),
            "",
        );
        assert!(
            matches!(interpret(&bad_model), Err(LlmError::ModelNotFound(message)) if message.contains("selected model")),
            "앞에 다른 줄이 섞여도 결과 줄을 읽어야 합니다"
        );

        let limited = finished(false, &result_line(true, "usage limit", Some(429)), "");
        assert!(matches!(
            interpret(&limited),
            Err(LlmError::Http { status: 429, .. })
        ));

        let other = finished(
            false,
            &result_line(true, "API Error: Connection error.", None),
            "",
        );
        assert!(
            matches!(interpret(&other), Err(LlmError::CliFailed(message)) if message.contains("Connection"))
        );

        let crashed = finished(false, "", "warning\nError: something broke\n\n");
        assert_eq!(
            interpret(&crashed),
            Err(LlmError::CliFailed("Error: something broke".into()))
        );

        let silent = finished(true, "not json", "");
        assert!(matches!(interpret(&silent), Err(LlmError::BadResponse(_))));
        assert!(
            !format!("{:?}", interpret(&silent)).contains("not json"),
            "읽지 못한 출력(다듬은 글일 수 있음)은 오류에 싣지 않습니다"
        );
    }

    #[test]
    fn request_arguments_turn_off_tools_and_user_settings() {
        let args = request_args(
            " opus ",
            ReasoningEffort::Low,
            Path::new("C:/tmp/prompt.txt"),
        );
        let args: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let value_after = |flag: &str| {
            let at = args
                .iter()
                .position(|arg| arg == flag)
                .unwrap_or_else(|| panic!("{flag}가 없습니다: {args:?}"));
            args[at + 1].clone()
        };
        assert_eq!(args[0], "-p");
        assert_eq!(value_after("--output-format"), "json");
        assert_eq!(value_after("--tools"), "", "도구를 모두 꺼야 합니다");
        assert_eq!(value_after("--model"), "opus");
        assert_eq!(value_after("--effort"), "low");
        assert_eq!(value_after("--system-prompt-file"), "C:/tmp/prompt.txt");
        for flag in [
            "--no-session-persistence",
            "--safe-mode",
            "--strict-mcp-config",
        ] {
            assert!(
                args.iter().any(|arg| arg == flag),
                "{flag}가 없습니다: {args:?}"
            );
        }

        let plain = request_args("", ReasoningEffort::Default, Path::new("p.txt"));
        let plain: Vec<String> = plain
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !plain
                .iter()
                .any(|arg| arg == "--model" || arg == "--effort"),
            "{plain:?}"
        );
        let none = request_args("sonnet", ReasoningEffort::None, Path::new("p.txt"));
        assert!(
            !none.iter().any(|arg| arg == "--effort"),
            "CLI가 받지 않는 단계는 넘기지 않습니다"
        );
    }

    #[test]
    fn settings_round_trip_and_default() {
        let mut config = PostProcessConfig::default();
        assert!(!is_registered(&config));
        assert_eq!(chosen_model(&config), DEFAULT_MODEL);
        assert_eq!(model_setting(&config), "");
        assert_eq!(chosen_effort(&config), ReasoningEffort::Default);

        set_registered(&mut config, true);
        set_model(&mut config, " haiku ");
        set_effort(&mut config, ReasoningEffort::High);
        set_path(&mut config, " C:/bin/claude.exe ");
        assert!(is_registered(&config));
        assert_eq!(chosen_model(&config), "haiku");
        assert_eq!(chosen_effort(&config), ReasoningEffort::High);
        assert_eq!(path_setting(&config), "C:/bin/claude.exe");

        set_registered(&mut config, false);
        assert!(!is_registered(&config));
        assert_eq!(
            config.processors[CLAUDE_CLI_PROCESSOR_ID].get(REGISTERED_KEY),
            Some(&TomlValue::Boolean(false)),
            "빼도 키를 지우지 않고 false로 적어야 합니다"
        );
    }

    #[test]
    fn custom_cli_path_must_be_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("claude-custom.exe");
        let mut config = PostProcessConfig::default();
        set_path(&mut config, &cli.to_string_lossy());
        assert_eq!(find_cli_now(&config), None);
        std::fs::write(&cli, b"").unwrap();
        assert_eq!(find_cli_now(&config), Some(cli));
        set_path(&mut config, &dir.path().to_string_lossy());
        assert_eq!(find_cli_now(&config), None, "폴더는 CLI로 보지 않습니다");
    }

    #[test]
    fn status_output_is_read() {
        assert_eq!(
            parse_version("2.1.328 (Claude Code)\n"),
            Some("2.1.328".into())
        );
        assert_eq!(parse_version("\n"), None);
        assert_eq!(
            parse_login(
                r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max","email":"me@example.com"}"#
            ),
            CliLogin::LoggedIn {
                method: Some("claude.ai".into()),
                plan: Some("max".into()),
                email: Some("me@example.com".into()),
            }
        );
        assert_eq!(parse_login(r#"{"loggedIn":false}"#), CliLogin::LoggedOut);
        assert_eq!(
            parse_login("{\n  \"loggedIn\": false,\n  \"authMethod\": \"none\"\n}\n"),
            CliLogin::LoggedOut,
            "여러 줄로 찍힌 JSON도 읽어야 합니다"
        );
        assert_eq!(parse_login("error: unknown command"), CliLogin::Unknown);
    }

    #[test]
    fn missing_cli_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        let result = chat(
            &paths,
            CliTarget {
                cli: &dir.path().join("no-such-claude.exe"),
                model: "sonnet",
                effort: ReasoningEffort::Default,
            },
            "지시",
            "글",
            Duration::from_secs(5),
            &CancelToken::new(),
        );
        assert_eq!(result, Err(LlmError::CliNotFound));
        let leftovers: Vec<_> = std::fs::read_dir(paths.data_dir.join("claude-cli"))
            .unwrap()
            .collect();
        assert!(leftovers.is_empty(), "지시문 임시 파일을 지워야 합니다");
    }

    /// 가짜 CLI: 표준 입력을 다 읽은 뒤 `output`을 출력하거나, `sleep`이면 오래 기다립니다.
    ///
    /// Windows에서는 외부 명령을 전체 경로로 부릅니다. 여러 크레이트를 함께 테스트하면 cargo가 PATH를 길게
    /// 늘려, cmd가 PATH에서 명령을 찾지 못합니다.
    fn fake_cli(dir: &Path, output: &str, sleep: bool) -> PathBuf {
        let output_file = dir.join("output.json");
        std::fs::write(&output_file, output).unwrap();
        if cfg!(windows) {
            let script = dir.join("claude.cmd");
            let body = if sleep {
                "@echo off\r\n\"%SystemRoot%\\System32\\PING.EXE\" -n 30 127.0.0.1 >nul\r\n"
                    .to_owned()
            } else {
                format!(
                    "@echo off\r\n\"%SystemRoot%\\System32\\more.com\" >nul\r\ntype \"{}\"\r\n",
                    output_file.display()
                )
            };
            std::fs::write(&script, body).unwrap();
            script
        } else {
            let script = dir.join("claude");
            let body = if sleep {
                "#!/bin/sh\nsleep 30\n".to_owned()
            } else {
                format!(
                    "#!/bin/sh\ncat >/dev/null\ncat '{}'\n",
                    output_file.display()
                )
            };
            std::fs::write(&script, body).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            script
        }
    }

    #[test]
    fn runs_the_cli_and_reads_its_result() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        let cli = fake_cli(dir.path(), &result_line(false, "OK", None), false);
        let reply = test_connection(
            &paths,
            CliTarget {
                cli: &cli,
                model: "sonnet",
                effort: ReasoningEffort::Low,
            },
            Duration::from_secs(30),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(reply.text, "OK");
        assert_eq!(reply.model.as_deref(), Some("claude-sonnet-5-5"));
    }

    #[test]
    fn cancel_and_timeout_stop_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        let cli = fake_cli(dir.path(), "", true);

        let started = Instant::now();
        let result = test_connection(
            &paths,
            CliTarget {
                cli: &cli,
                model: "sonnet",
                effort: ReasoningEffort::Default,
            },
            Duration::from_millis(300),
            &CancelToken::new(),
        );
        assert!(matches!(result, Err(LlmError::Network(_))), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(10));

        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            canceller.cancel();
        });
        let started = Instant::now();
        let result = test_connection(
            &paths,
            CliTarget {
                cli: &cli,
                model: "sonnet",
                effort: ReasoningEffort::Default,
            },
            Duration::from_secs(30),
            &cancel,
        );
        assert_eq!(result, Err(LlmError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// 이 PC에 설치하고 로그인한 Claude Code CLI로 실제로 다듬어 봅니다.
    #[test]
    #[ignore = "설치하고 로그인한 Claude Code CLI가 필요합니다"]
    fn real_claude_cli_refines() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        let cli =
            find_cli_now(&PostProcessConfig::default()).expect("claude CLI를 찾지 못했습니다");
        let status = check(&paths, &cli, STATUS_TIMEOUT, &CancelToken::new()).unwrap();
        assert!(
            matches!(status.login, CliLogin::LoggedIn { .. }),
            "{status:?}"
        );
        let reply = chat(
            &paths,
            CliTarget {
                cli: &cli,
                model: "haiku",
                effort: ReasoningEffort::Low,
            },
            "받아쓴 글의 맞춤법과 띄어쓰기만 고쳐, 고친 글만 돌려주세요.",
            "안녕 하세요 오늘은 날씨가 좋네요",
            REQUEST_TIMEOUT,
            &CancelToken::new(),
        )
        .unwrap();
        assert!(reply.text.contains("안녕하세요"), "{reply:?}");
        println!("{:?} · {:?} · {:?}", reply.text, reply.model, reply.elapsed);
    }
}
