//! 큰 파일(모델 등) 내려받기.
//!
//! [`download`]는 `<dest>.part`에 받아 두었다가 크기와 SHA-256 검사를 통과한 뒤에만
//! `dest`로 이름을 바꿉니다. 그래서 중간에 실패하거나 취소해도 `dest`에는 덜 받은 파일이
//! 생기지 않고, 남은 `.part`는 다음 호출에서 이어받습니다.
//!
//! 끝날 때까지 기다리는 함수이므로 UI 스레드가 아닌 작업 스레드에서 부르세요.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use ureq::http::HeaderMap;

use crate::cancel::CancelToken;
use crate::stt::ModelRequirement;

/// 주소 찾기와 연결(TLS 포함)에 각각 허용하는 시간
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// 요청을 보낸 뒤 응답 헤더를 기다리는 시간
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
/// 데이터가 한 번도 오지 않은 채 기다리는 최대 시간. 전체 전송 시간에는 제한이 없습니다.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 진행률 콜백을 부르는 최소 간격
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const DOWNLOAD_BUFFER: usize = 64 * 1024;
const HASH_BUFFER: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadRequest {
    pub url: String,
    /// 다 받은 뒤의 최종 파일 위치
    pub dest: PathBuf,
    pub expected_size: Option<u64>,
    /// 소문자 16진수 SHA-256. 있으면 다 받은 뒤 검사합니다.
    pub sha256: Option<String>,
}

impl DownloadRequest {
    /// 모델 파일용 요청. `asset`이 없으면(사용자가 파일을 직접 지정) `None`.
    pub fn for_model(requirement: &ModelRequirement) -> Option<Self> {
        let asset = requirement.asset.as_ref()?;
        Some(Self {
            url: asset.url.clone(),
            dest: requirement.path.clone(),
            expected_size: Some(asset.size_bytes),
            sha256: Some(asset.sha256.clone()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    /// [`DownloadPhase::Downloading`]에서는 받은 바이트 수(이어받은 부분 포함),
    /// [`DownloadPhase::Verifying`]에서는 검사를 마친 바이트 수
    pub downloaded: u64,
    /// 전체 크기. 요청에도 없고 서버도 알려 주지 않으면 `None`입니다.
    pub total: Option<u64>,
    pub phase: DownloadPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPhase {
    Downloading,
    /// 다 받은 파일의 SHA-256 검사 중. 요청에 해시가 있을 때만 거칩니다.
    Verifying,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("download was cancelled")]
    Cancelled,
    #[error("server responded with HTTP status {status}")]
    Http { status: u16 },
    /// 연결 실패, 시간 초과, 전송 중 끊김 등. `.part`가 남아 있으면 다시 불러 이어받습니다.
    #[error("network error: {0}")]
    Network(String),
    #[error("file error at {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("size mismatch: expected {expected} bytes but got {actual} bytes")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("SHA-256 mismatch: expected {expected} but got {actual}")]
    HashMismatch { expected: String, actual: String },
}

/// 받는 중인 임시 파일 위치 (`<dest>.part`)
pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

/// 파일을 내려받습니다. 끝날 때까지 기다리므로 작업 스레드에서 부르세요.
///
/// - `<dest>.part`가 있으면 이어받습니다. 취소하거나 연결이 끊기면 `.part`를 남겨 두므로
///   다시 부르면 이어서 받습니다.
/// - 해시가 다르거나 받은 파일이 기대보다 크면 `.part`를 지웁니다(다음에는 처음부터).
/// - `on_progress`는 시작과 각 단계의 끝에 반드시 불리고, 그 사이에는 약 100ms 간격으로
///   불립니다.
pub fn download(
    request: &DownloadRequest,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<(), DownloadError> {
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }
    if let Some(parent) = request.dest.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(io_error(parent))?;
    }

    let part = part_path(&request.dest);
    let mut reporter = Reporter::new(on_progress);

    let mut existing = file_len(&part)?;
    if let Some(expected) = request.expected_size
        && existing > expected
    {
        fs::remove_file(&part).map_err(io_error(&part))?;
        existing = 0;
    }
    reporter.force(DownloadProgress {
        downloaded: existing,
        total: request.expected_size,
        phase: DownloadPhase::Downloading,
    });

    // `.part`가 이미 다 받아져 있으면 요청 없이 검사로 넘어갑니다.
    let already_complete = part.is_file() && request.expected_size == Some(existing);
    if !already_complete {
        fetch(request, &part, cancel, &mut reporter)?;
    }
    verify(request, &part, cancel, &mut reporter)?;

    fs::rename(&part, &request.dest).map_err(io_error(&request.dest))
}

/// 서버에서 받아 `.part`에 씁니다. `.part`가 있으면 이어받기를 먼저 시도합니다.
fn fetch(
    request: &DownloadRequest,
    part: &Path,
    cancel: &CancelToken,
    reporter: &mut Reporter<'_>,
) -> Result<(), DownloadError> {
    let agent = new_agent();
    let mut resume = true;

    let (response, start, server_total) = loop {
        let offset = if resume { file_len(part)? } else { 0 };
        let mut builder = agent.get(&request.url);
        if offset > 0 {
            builder = builder.header("Range", format!("bytes={offset}-"));
        }
        let response = builder.call().map_err(network_error)?;

        match response.status().as_u16() {
            // 서버가 Range를 무시했거나 처음부터 받는 경우
            200 => {
                let total = response.body().content_length();
                break (response, 0, total);
            }
            206 => {
                let range = content_range(response.headers());
                let start = range.map_or(offset, |(start, _)| start);
                if start != offset {
                    // 요청한 위치와 다른 조각을 받았습니다. 이어 붙일 수 없으니 처음부터 받습니다.
                    if !resume {
                        return Err(DownloadError::Network(format!(
                            "unexpected Content-Range start {start} (requested {offset})"
                        )));
                    }
                    resume = false;
                    continue;
                }
                let total = range
                    .and_then(|(_, total)| total)
                    .or_else(|| response.body().content_length().map(|len| offset + len));
                break (response, offset, total);
            }
            // `.part`가 서버 파일보다 크거나 같습니다. 지우고 처음부터 받습니다.
            416 if offset > 0 => {
                fs::remove_file(part).map_err(io_error(part))?;
                resume = false;
            }
            status => return Err(DownloadError::Http { status }),
        }
    };

    // 서버가 알려 준 크기가 기대와 다르면 받기 전에 그만둡니다.
    if let (Some(expected), Some(actual)) = (request.expected_size, server_total)
        && expected != actual
    {
        return Err(DownloadError::SizeMismatch { expected, actual });
    }
    let total = request.expected_size.or(server_total);

    let mut file = if start > 0 {
        OpenOptions::new().append(true).open(part)
    } else {
        File::create(part)
    }
    .map_err(io_error(part))?;

    let mut downloaded = start;
    let progress = |downloaded| DownloadProgress {
        downloaded,
        total,
        phase: DownloadPhase::Downloading,
    };
    reporter.tick(progress(downloaded));

    let mut reader = response.into_body().into_reader();
    let mut buffer = vec![0u8; DOWNLOAD_BUFFER];
    loop {
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(DownloadError::Network(e.to_string())),
        };
        file.write_all(&buffer[..read]).map_err(io_error(part))?;
        downloaded += read as u64;
        reporter.tick(progress(downloaded));
    }
    file.sync_all().map_err(io_error(part))?;
    reporter.force(progress(downloaded));
    Ok(())
}

/// 다 받은 `.part`의 크기와 해시를 검사합니다.
fn verify(
    request: &DownloadRequest,
    part: &Path,
    cancel: &CancelToken,
    reporter: &mut Reporter<'_>,
) -> Result<(), DownloadError> {
    let actual = file_len(part)?;
    if let Some(expected) = request.expected_size
        && actual != expected
    {
        // 모자란 파일은 이어받을 수 있으니 남기고, 넘친 파일은 쓸 수 없으니 지웁니다.
        if actual > expected {
            fs::remove_file(part).map_err(io_error(part))?;
        }
        return Err(DownloadError::SizeMismatch { expected, actual });
    }

    let Some(expected) = &request.sha256 else {
        return Ok(());
    };
    let progress = |downloaded| DownloadProgress {
        downloaded,
        total: Some(actual),
        phase: DownloadPhase::Verifying,
    };
    reporter.force(progress(0));

    let mut file = File::open(part).map_err(io_error(part))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER];
    let mut hashed = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(part)(e)),
        };
        hasher.update(&buffer[..read]);
        hashed += read as u64;
        reporter.tick(progress(hashed));
    }
    drop(file);
    reporter.force(progress(hashed));

    let actual_hash = to_hex(&hasher.finalize());
    if !actual_hash.eq_ignore_ascii_case(expected) {
        fs::remove_file(part).map_err(io_error(part))?;
        return Err(DownloadError::HashMismatch {
            expected: expected.to_ascii_lowercase(),
            actual: actual_hash,
        });
    }
    Ok(())
}

fn new_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        // 상태 코드는 직접 해석합니다(206, 416 처리).
        .http_status_as_error(false)
        .timeout_resolve(Some(CONNECT_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .timeout_per_read(Some(READ_TIMEOUT))
        .build()
        .into()
}

/// `Content-Range: bytes <시작>-<끝>/<전체>`에서 시작 위치와 전체 크기를 읽습니다.
fn content_range(headers: &HeaderMap) -> Option<(u64, Option<u64>)> {
    let value = headers.get("content-range")?.to_str().ok()?;
    let (range, total) = value.trim().strip_prefix("bytes")?.trim().split_once('/')?;
    let (start, _) = range.split_once('-')?;
    Some((start.trim().parse().ok()?, total.trim().parse().ok()))
}

/// 파일 크기. 파일이 없으면 0입니다.
fn file_len(path: &Path) -> Result<u64, DownloadError> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(io_error(path)(e)),
    }
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> DownloadError {
    let path = path.to_path_buf();
    move |source| DownloadError::Io { path, source }
}

fn network_error(error: ureq::Error) -> DownloadError {
    DownloadError::Network(error.to_string())
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// 진행률 콜백을 너무 자주 부르지 않게 간격을 둡니다.
struct Reporter<'a> {
    on_progress: &'a mut dyn FnMut(DownloadProgress),
    last: Option<Instant>,
}

impl<'a> Reporter<'a> {
    fn new(on_progress: &'a mut dyn FnMut(DownloadProgress)) -> Self {
        Self {
            on_progress,
            last: None,
        }
    }

    /// 간격과 상관없이 바로 알립니다.
    fn force(&mut self, progress: DownloadProgress) {
        self.last = Some(Instant::now());
        (self.on_progress)(progress);
    }

    /// 마지막으로 알린 뒤 충분히 지났을 때만 알립니다.
    fn tick(&mut self, progress: DownloadProgress) {
        if self
            .last
            .is_none_or(|last| last.elapsed() >= PROGRESS_INTERVAL)
        {
            self.force(progress);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use super::*;
    use crate::stt::ModelAsset;

    #[derive(Debug, Clone)]
    struct Request {
        path: String,
        /// `Range: bytes=<시작>-`의 시작 위치
        range_start: Option<u64>,
        /// 요청 헤더 이름(소문자)
        header_names: Vec<String>,
    }

    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        /// `Content-Length` 값. `None`이면 헤더 없이 연결을 닫아 끝을 알립니다.
        content_length: Option<usize>,
        /// 본문을 (조각 크기, 쉬는 시간)으로 천천히 보냅니다.
        pace: Option<(usize, Duration)>,
    }

    impl Reply {
        fn new(status: u16, body: Vec<u8>) -> Self {
            Self {
                status,
                headers: Vec::new(),
                content_length: Some(body.len()),
                body,
                pace: None,
            }
        }

        fn header(mut self, name: &str, value: impl Into<String>) -> Self {
            self.headers.push((name.to_owned(), value.into()));
            self
        }
    }

    /// 테스트용 HTTP 서버. 요청 하나에 연결 하나를 씁니다.
    struct TestServer {
        addr: SocketAddr,
        requests: Arc<Mutex<Vec<Request>>>,
    }

    impl TestServer {
        fn start(handler: impl Fn(&Request) -> Reply + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&requests);
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    // 클라이언트가 먼저 끊는 테스트가 있어 전송 오류는 무시합니다.
                    let _ = serve(stream, &handler, &seen);
                }
            });
            Self { addr, requests }
        }

        fn url(&self, path: &str) -> String {
            format!("http://{}{path}", self.addr)
        }

        fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn serve(
        mut stream: TcpStream,
        handler: &dyn Fn(&Request) -> Reply,
        seen: &Mutex<Vec<Request>>,
    ) -> io::Result<()> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
        let mut request = Request {
            path,
            range_start: None,
            header_names: Vec::new(),
        };
        loop {
            line.clear();
            reader.read_line(&mut line)?;
            let Some((name, value)) = line.trim_end().split_once(':') else {
                break;
            };
            let name = name.to_ascii_lowercase();
            if name == "range" {
                request.range_start = value
                    .trim()
                    .strip_prefix("bytes=")
                    .and_then(|range| range.strip_suffix('-'))
                    .and_then(|start| start.parse().ok());
            }
            request.header_names.push(name);
        }
        seen.lock().unwrap().push(request.clone());

        let reply = handler(&request);
        let mut head = format!("HTTP/1.1 {} Test\r\nConnection: close\r\n", reply.status);
        if let Some(length) = reply.content_length {
            head.push_str(&format!("Content-Length: {length}\r\n"));
        }
        for (name, value) in &reply.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        match reply.pace {
            Some((chunk, pause)) => {
                for piece in reply.body.chunks(chunk) {
                    stream.write_all(piece)?;
                    stream.flush()?;
                    thread::sleep(pause);
                }
            }
            None => stream.write_all(&reply.body)?,
        }
        stream.flush()
    }

    /// `/file`은 `data`를 주고, `/redirect`는 `/file`로 302를 보냅니다. 그 밖은 404.
    fn file_server(data: Vec<u8>, honor_range: bool) -> TestServer {
        TestServer::start(move |request| match request.path.as_str() {
            "/file" => file_reply(&data, request, honor_range),
            "/redirect" => Reply::new(302, Vec::new()).header("Location", "/file"),
            _ => Reply::new(404, b"not found".to_vec()),
        })
    }

    fn file_reply(data: &[u8], request: &Request, honor_range: bool) -> Reply {
        match request.range_start {
            Some(start) if honor_range => {
                let start = start as usize;
                if start >= data.len() {
                    return Reply::new(416, Vec::new())
                        .header("Content-Range", format!("bytes */{}", data.len()));
                }
                Reply::new(206, data[start..].to_vec()).header(
                    "Content-Range",
                    format!("bytes {start}-{}/{}", data.len() - 1, data.len()),
                )
            }
            _ => Reply::new(200, data.to_vec()),
        }
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 % 251) as u8).collect()
    }

    fn sha256_hex(data: &[u8]) -> String {
        to_hex(&Sha256::digest(data))
    }

    fn request_for(url: String, dest: &Path, data: &[u8]) -> DownloadRequest {
        DownloadRequest {
            url,
            dest: dest.to_path_buf(),
            expected_size: Some(data.len() as u64),
            sha256: Some(sha256_hex(data)),
        }
    }

    fn run(request: &DownloadRequest) -> Result<(), DownloadError> {
        download(request, &CancelToken::new(), &mut |_| {})
    }

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(
            part_path(Path::new("models/ggml-large-v3.bin")),
            PathBuf::from("models/ggml-large-v3.bin.part")
        );
    }

    #[test]
    fn for_model_uses_asset() {
        let mut requirement = ModelRequirement {
            path: PathBuf::from("models/a.bin"),
            asset: Some(ModelAsset {
                id: "a".into(),
                file_name: "a.bin".into(),
                url: "https://example.com/a.bin".into(),
                size_bytes: 42,
                sha256: "ab".repeat(32),
            }),
        };
        assert_eq!(
            DownloadRequest::for_model(&requirement),
            Some(DownloadRequest {
                url: "https://example.com/a.bin".into(),
                dest: PathBuf::from("models/a.bin"),
                expected_size: Some(42),
                sha256: Some("ab".repeat(32)),
            })
        );

        requirement.asset = None;
        assert_eq!(DownloadRequest::for_model(&requirement), None);
    }

    #[test]
    fn downloads_whole_file() {
        let data = sample(300_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        // 없는 폴더도 만들어야 합니다.
        let dest = dir.path().join("models").join("model.bin");
        let request = request_for(server.url("/file"), &dest, &data);

        let mut events = Vec::new();
        download(&request, &CancelToken::new(), &mut |p| events.push(p)).unwrap();

        assert_eq!(fs::read(&dest).unwrap(), data);
        assert!(!part_path(&dest).exists());

        let total = Some(data.len() as u64);
        assert_eq!(
            events.first(),
            Some(&DownloadProgress {
                downloaded: 0,
                total,
                phase: DownloadPhase::Downloading,
            })
        );
        assert!(events.contains(&DownloadProgress {
            downloaded: data.len() as u64,
            total,
            phase: DownloadPhase::Downloading,
        }));
        assert_eq!(
            events.last(),
            Some(&DownloadProgress {
                downloaded: data.len() as u64,
                total,
                phase: DownloadPhase::Verifying,
            })
        );

        // 라이브러리 기본 헤더 말고는 보내지 않습니다(처음 받을 때는 Range도 없음).
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        let mut names = requests[0].header_names.clone();
        names.sort();
        assert_eq!(names, ["accept", "host", "user-agent"]);
    }

    #[test]
    fn replaces_existing_dest() {
        let data = sample(10_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        fs::write(&dest, b"old").unwrap();

        run(&request_for(server.url("/file"), &dest, &data)).unwrap();

        assert_eq!(fs::read(&dest).unwrap(), data);
    }

    #[test]
    fn downloads_without_size_or_hash() {
        let data = sample(10_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        let request = DownloadRequest {
            url: server.url("/file"),
            dest: dest.clone(),
            expected_size: None,
            sha256: None,
        };

        let mut events = Vec::new();
        download(&request, &CancelToken::new(), &mut |p| events.push(p)).unwrap();

        assert_eq!(fs::read(&dest).unwrap(), data);
        assert_eq!(
            events.last(),
            Some(&DownloadProgress {
                downloaded: data.len() as u64,
                total: Some(data.len() as u64),
                phase: DownloadPhase::Downloading,
            })
        );
    }

    #[test]
    fn resumes_partial_file() {
        let data = sample(200_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        fs::write(part_path(&dest), &data[..80_000]).unwrap();

        run(&request_for(server.url("/file"), &dest, &data)).unwrap();

        assert_eq!(fs::read(&dest).unwrap(), data);
        assert!(!part_path(&dest).exists());
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].range_start, Some(80_000));
    }

    #[test]
    fn restarts_when_server_ignores_range() {
        let data = sample(200_000);
        let server = file_server(data.clone(), false);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        // 이어 붙이면 해시가 틀어지도록 일부러 다른 내용을 둡니다.
        fs::write(part_path(&dest), vec![0xFF; 80_000]).unwrap();

        run(&request_for(server.url("/file"), &dest, &data)).unwrap();

        assert_eq!(fs::read(&dest).unwrap(), data);
        assert_eq!(server.requests()[0].range_start, Some(80_000));
    }

    #[test]
    fn resumes_after_redirect() {
        let data = sample(200_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        fs::write(part_path(&dest), &data[..80_000]).unwrap();

        run(&request_for(server.url("/redirect"), &dest, &data)).unwrap();

        assert_eq!(fs::read(&dest).unwrap(), data);
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/redirect");
        assert_eq!(requests[1].path, "/file");
        // 리다이렉트 뒤에도 Range가 유지돼야 이어받을 수 있습니다.
        assert_eq!(requests[1].range_start, Some(80_000));
    }

    #[test]
    fn restarts_on_range_not_satisfiable() {
        let data = sample(50_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        // 크기를 모르는 요청이라 `.part`가 너무 큰지 미리 알 수 없습니다.
        fs::write(part_path(&dest), vec![0u8; 60_000]).unwrap();
        let request = DownloadRequest {
            expected_size: None,
            ..request_for(server.url("/file"), &dest, &data)
        };

        run(&request).unwrap();

        assert_eq!(fs::read(&dest).unwrap(), data);
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].range_start, Some(60_000));
        assert_eq!(requests[1].range_start, None);
    }

    #[test]
    fn restarts_when_part_is_larger_than_expected() {
        let data = sample(50_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        fs::write(part_path(&dest), vec![0u8; 60_000]).unwrap();

        run(&request_for(server.url("/file"), &dest, &data)).unwrap();

        assert_eq!(fs::read(&dest).unwrap(), data);
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].range_start, None);
    }

    #[test]
    fn interrupted_download_keeps_part_and_resumes() {
        let data = sample(200_000);
        let half = data.len() / 2;
        let served = data.clone();
        let server = TestServer::start(move |request| {
            if request.range_start.is_some() {
                return file_reply(&served, request, true);
            }
            // 전체 길이를 알려 놓고 절반만 보낸 뒤 끊습니다.
            let mut reply = Reply::new(200, served[..half].to_vec());
            reply.content_length = Some(served.len());
            reply
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        let request = request_for(server.url("/file"), &dest, &data);

        let error = run(&request).unwrap_err();
        assert!(matches!(error, DownloadError::Network(_)), "{error:?}");
        assert!(!dest.exists());
        assert_eq!(fs::read(part_path(&dest)).unwrap(), &data[..half]);

        run(&request).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert!(!part_path(&dest).exists());
        assert_eq!(server.requests()[1].range_start, Some(half as u64));
    }

    #[test]
    fn hash_mismatch_removes_part() {
        let data = sample(50_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        let wrong = "0".repeat(64);
        let request = DownloadRequest {
            sha256: Some(wrong.clone()),
            ..request_for(server.url("/file"), &dest, &data)
        };

        match run(&request).unwrap_err() {
            DownloadError::HashMismatch { expected, actual } => {
                assert_eq!(expected, wrong);
                assert_eq!(actual, sha256_hex(&data));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn accepts_uppercase_hash() {
        let data = sample(10_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        let request = DownloadRequest {
            sha256: Some(sha256_hex(&data).to_ascii_uppercase()),
            ..request_for(server.url("/file"), &dest, &data)
        };

        run(&request).unwrap();
        assert!(dest.exists());
    }

    #[test]
    fn size_mismatch_from_declared_length() {
        let data = sample(50_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        let request = DownloadRequest {
            expected_size: Some(50_001),
            sha256: None,
            ..request_for(server.url("/file"), &dest, &data)
        };

        let error = run(&request).unwrap_err();
        assert!(
            matches!(
                error,
                DownloadError::SizeMismatch {
                    expected: 50_001,
                    actual: 50_000
                }
            ),
            "{error:?}"
        );
        assert!(!dest.exists());
    }

    #[test]
    fn size_mismatch_after_download() {
        let data = sample(50_000);
        let served = data.clone();
        // 길이를 알려 주지 않는 서버: 다 받은 뒤에야 크기를 알 수 있습니다.
        let server = TestServer::start(move |_| {
            let mut reply = Reply::new(200, served.clone());
            reply.content_length = None;
            reply
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        let request = DownloadRequest {
            expected_size: Some(60_000),
            sha256: None,
            ..request_for(server.url("/file"), &dest, &data)
        };

        let error = run(&request).unwrap_err();
        assert!(
            matches!(
                error,
                DownloadError::SizeMismatch {
                    expected: 60_000,
                    actual: 50_000
                }
            ),
            "{error:?}"
        );
        assert!(!dest.exists());
        // 모자란 파일은 이어받을 수 있게 남깁니다.
        assert_eq!(fs::read(part_path(&dest)).unwrap(), data);
    }

    #[test]
    fn cancel_during_download_keeps_part() {
        let data = sample(400_000);
        let served = data.clone();
        let server = TestServer::start(move |request| {
            let mut reply = file_reply(&served, request, true);
            reply.pace = Some((4096, Duration::from_millis(10)));
            reply
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        let request = request_for(server.url("/file"), &dest, &data);

        let cancel = CancelToken::new();
        let trigger = cancel.clone();
        let error = download(&request, &cancel, &mut |progress| {
            if progress.downloaded > 0 {
                trigger.cancel();
            }
        })
        .unwrap_err();

        assert!(matches!(error, DownloadError::Cancelled), "{error:?}");
        assert!(!dest.exists());
        let part = fs::read(part_path(&dest)).unwrap();
        assert!(!part.is_empty() && part.len() < data.len());
        assert_eq!(part, &data[..part.len()]);
    }

    #[test]
    fn cancel_during_verification_keeps_part() {
        let data = sample(50_000);
        let server = file_server(data.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        let request = request_for(server.url("/file"), &dest, &data);

        let cancel = CancelToken::new();
        let trigger = cancel.clone();
        let error = download(&request, &cancel, &mut |progress| {
            if progress.phase == DownloadPhase::Verifying {
                trigger.cancel();
            }
        })
        .unwrap_err();

        assert!(matches!(error, DownloadError::Cancelled), "{error:?}");
        assert!(!dest.exists());
        assert_eq!(fs::read(part_path(&dest)).unwrap(), data);
    }

    #[test]
    fn cancelled_before_start() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        let cancel = CancelToken::new();
        cancel.cancel();
        let request = DownloadRequest {
            url: "http://127.0.0.1:1/file".into(),
            dest,
            expected_size: None,
            sha256: None,
        };

        let error = download(&request, &cancel, &mut |_| {}).unwrap_err();
        assert!(matches!(error, DownloadError::Cancelled), "{error:?}");
    }

    #[test]
    fn not_found_is_http_error() {
        let server = file_server(Vec::new(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        let request = DownloadRequest {
            url: server.url("/missing"),
            dest: dest.clone(),
            expected_size: None,
            sha256: None,
        };

        let error = run(&request).unwrap_err();
        assert!(
            matches!(error, DownloadError::Http { status: 404 }),
            "{error:?}"
        );
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn connection_failure_is_network_error() {
        // 포트를 잡았다가 놓아, 아무도 듣지 않는 주소를 만듭니다.
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let request = DownloadRequest {
            url: format!("http://{addr}/file"),
            dest: dir.path().join("model.bin"),
            expected_size: None,
            sha256: None,
        };

        let error = run(&request).unwrap_err();
        assert!(matches!(error, DownloadError::Network(_)), "{error:?}");
    }

    #[test]
    fn complete_part_skips_request() {
        let data = sample(50_000);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        fs::write(part_path(&dest), &data).unwrap();
        // 아무도 듣지 않는 주소: 요청을 보내면 실패합니다.
        let request = request_for("http://127.0.0.1:1/file".into(), &dest, &data);

        let mut events = Vec::new();
        download(&request, &CancelToken::new(), &mut |p| events.push(p)).unwrap();

        assert_eq!(fs::read(&dest).unwrap(), data);
        assert!(!part_path(&dest).exists());
        assert_eq!(events.first().unwrap().phase, DownloadPhase::Downloading);
        assert_eq!(events.last().unwrap().phase, DownloadPhase::Verifying);
    }

    /// 실제 네트워크 확인용: Hugging Face의 작은 파일(약 15MB)을 받고, 절반만 남긴 뒤 이어받습니다.
    /// `cargo test -p sulsul-core -- --ignored hugging_face`
    #[test]
    #[ignore = "외부 네트워크가 필요합니다"]
    fn hugging_face_download_and_resume() {
        const URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en-encoder.mlmodelc.zip";
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("hf-test.bin");
        let request = DownloadRequest {
            url: URL.into(),
            dest: dest.clone(),
            expected_size: None,
            sha256: None,
        };

        run(&request).unwrap();
        let data = fs::read(&dest).unwrap();
        assert!(!data.is_empty());
        println!("size: {} bytes, sha256: {}", data.len(), sha256_hex(&data));

        // CDN 리다이렉트 뒤에도 이어받기가 되는지 확인합니다.
        fs::remove_file(&dest).unwrap();
        fs::write(part_path(&dest), &data[..data.len() / 2]).unwrap();
        let mut first = None;
        download(
            &request_for(URL.into(), &dest, &data),
            &CancelToken::new(),
            &mut |progress| {
                first.get_or_insert(progress);
            },
        )
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert_eq!(first.unwrap().downloaded, (data.len() / 2) as u64);
    }
}
