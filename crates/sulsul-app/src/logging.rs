//! 앱 로그: 표준 오류와 로그 파일에 함께 남깁니다.
//!
//! 릴리스 빌드는 콘솔 창이 없어(`windows_subsystem = "windows"`) 표준 오류를 볼 곳이 없습니다. 화면에는 짧은
//! 안내만 보이고 오류 원문(경로, OS·서버 메시지 등)은 로그에만 남기므로, 같은 로그를 데이터 폴더의
//! 로그 파일([`AppPaths::log_file`](sulsul_core::AppPaths::log_file))에도 씁니다. 파일이
//! [`MAX_FILE_BYTES`]를 넘으려 하면 `sulsul.old.log`로 옮기고(있던 것은 지움) 새로 시작하므로, 로그 파일은
//! 많아야 두 개입니다.
//!
//! 기록 수준은 기본이 [`DEFAULT_FILTER`]이고 `RUST_LOG`로 바꿉니다. 로그 파일을 열지 못하면 표준 오류에만 씁니다.
//! 로그에는 받아쓴 글·다듬은 글 본문이나 API 키를 남기지 않습니다. 글은 길이(글자 수)만 적습니다.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer, fmt};

/// 기본 기록 수준 (`RUST_LOG`가 없거나 읽지 못할 때)
const DEFAULT_FILTER: &str = "warn,sulsul=info";

/// 로그 파일 하나의 크기 상한 (5MB). 이전 파일까지 합쳐 많아야 이 두 배입니다.
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// 로그를 표준 오류와 `log_file`(주면)에 남기도록 설정합니다. 프로그램을 시작할 때 한 번만 부릅니다.
pub fn init(log_file: Option<&Path>) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_FILTER.into());
    let (file, file_error) = match log_file.map(|path| RotatingFile::open(path, MAX_FILE_BYTES)) {
        Some(Ok(file)) => (Some(file), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_timer(LocalTime).with_writer(io::stderr))
        .with(file.map(file_layer))
        .init();
    if let (Some(path), Some(error)) = (log_file, file_error) {
        tracing::warn!(
            "could not open the log file {}; logging to stderr only: {error}",
            path.display()
        );
    }
}

/// 로그 파일에 쓰는 층. 파일에는 색 코드를 넣지 않습니다.
fn file_layer<S>(file: RotatingFile) -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fmt::layer()
        .with_ansi(false)
        .with_timer(LocalTime)
        .with_writer(Mutex::new(file))
}

/// 로그 줄 앞의 시각. Windows는 이 컴퓨터의 지역 시각(밀리초까지)이고, 다른 OS는 UTC입니다.
struct LocalTime;

impl FormatTime for LocalTime {
    #[cfg(windows)]
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;

        let mut now = SYSTEMTIME::default();
        // SAFETY: 이 함수의 지역 변수에 지금 지역 시각을 써 줍니다.
        unsafe { GetLocalTime(&mut now) };
        write!(
            w,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds
        )
    }

    #[cfg(not(windows))]
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        fmt::time::SystemTime.format_time(w)
    }
}

/// 크기 상한이 있는 로그 파일.
///
/// 쓰려는 줄이 상한을 넘기게 되면 먼저 지금 파일을 이전 파일(`<이름>.old.log`)로 옮기고 빈 파일로 새로
/// 시작합니다. 이전 파일은 하나만 둡니다. 줄은 나누지 않으므로, 빈 파일에는 상한보다 긴 줄도 그대로 씁니다.
pub struct RotatingFile {
    path: PathBuf,
    previous: PathBuf,
    max_bytes: u64,
    /// 열린 파일. 돌리다가 다시 열지 못하면 `None`이고, 다음에 쓸 때 다시 열어 봅니다.
    file: Option<File>,
    /// 지금 파일의 크기
    len: u64,
}

impl RotatingFile {
    /// `path`를 이어 쓰기로 엽니다. 폴더가 없으면 만듭니다.
    pub fn open(path: &Path, max_bytes: u64) -> io::Result<Self> {
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let file = open_append(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            previous: previous_path(path),
            max_bytes,
            file: Some(file),
            len,
        })
    }

    /// 지금 파일을 이전 파일로 옮깁니다. 옮기지 못하면(다른 프로그램이 이전 파일을 잡고 있는 등)
    /// 지금 파일을 비워 상한을 지킵니다. 파일은 [`reopen`](Self::reopen)이 다시 엽니다.
    fn rotate(&mut self) {
        // 열린 채로 옮기지 않도록 먼저 닫습니다.
        self.file = None;
        if fs::rename(&self.path, &self.previous).is_err() {
            let _ = File::create(&self.path);
        }
    }

    fn reopen(&mut self) {
        if let Ok(file) = open_append(&self.path) {
            self.len = file.metadata().map_or(0, |meta| meta.len());
            self.file = Some(file);
        }
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let incoming = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        if self.len > 0 && self.len.saturating_add(incoming) > self.max_bytes {
            self.rotate();
        }
        if self.file.is_none() {
            self.reopen();
        }
        let Some(file) = self.file.as_mut() else {
            // 파일을 다시 열지 못했으면 이 줄은 버립니다 (표준 오류에는 남습니다).
            return Ok(buf.len());
        };
        let written = file.write(buf)?;
        self.len = self.len.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().map_or(Ok(()), Write::flush)
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// 돌려 둔 이전 로그 파일 (`sulsul.log` → `sulsul.old.log`)
fn previous_path(path: &Path) -> PathBuf {
    path.with_extension("old.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    fn write_line(file: &mut RotatingFile, line: &str) {
        file.write_all(line.as_bytes())
            .expect("로그 줄을 쓰지 못했습니다");
    }

    #[test]
    fn creates_the_folder_and_appends_across_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("sulsul.log");

        let mut file = RotatingFile::open(&path, 1024).unwrap();
        write_line(&mut file, "first\n");
        drop(file);
        let mut file = RotatingFile::open(&path, 1024).unwrap();
        write_line(&mut file, "second\n");

        assert_eq!(read(&path), "first\nsecond\n", "다시 열면 이어 써야 합니다");
        assert!(
            !previous_path(&path).exists(),
            "상한 전에는 돌리지 않습니다"
        );
    }

    #[test]
    fn rotates_before_a_line_would_pass_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sulsul.log");
        let previous = dir.path().join("sulsul.old.log");
        assert_eq!(previous_path(&path), previous);

        let mut file = RotatingFile::open(&path, 10).unwrap();
        write_line(&mut file, "aaaa\n");
        write_line(&mut file, "bbbb\n");
        assert_eq!(
            read(&path),
            "aaaa\nbbbb\n",
            "상한에 딱 맞으면 그대로 씁니다"
        );
        assert!(!previous.exists());

        write_line(&mut file, "cc\n");
        assert_eq!(read(&previous), "aaaa\nbbbb\n");
        assert_eq!(read(&path), "cc\n");
    }

    #[test]
    fn keeps_only_one_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sulsul.log");
        let mut file = RotatingFile::open(&path, 8).unwrap();
        for line in ["one---\n", "two---\n", "three-\n", "four--\n"] {
            write_line(&mut file, line);
        }

        assert_eq!(read(&path), "four--\n");
        assert_eq!(read(&previous_path(&path)), "three-\n");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "로그 파일은 둘만 남아야 합니다: {names:?}");
    }

    #[test]
    fn existing_file_over_the_limit_rotates_on_the_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sulsul.log");
        fs::write(&path, "x".repeat(32)).unwrap();

        let mut file = RotatingFile::open(&path, 16).unwrap();
        write_line(&mut file, "new\n");

        assert_eq!(read(&previous_path(&path)), "x".repeat(32));
        assert_eq!(read(&path), "new\n");
    }

    #[test]
    fn a_line_longer_than_the_limit_goes_into_a_fresh_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sulsul.log");
        let long = format!("{}\n", "y".repeat(40));

        let mut file = RotatingFile::open(&path, 16).unwrap();
        write_line(&mut file, &long);
        assert_eq!(read(&path), long, "빈 파일에는 긴 줄도 그대로 씁니다");
        write_line(&mut file, "next\n");

        assert_eq!(read(&previous_path(&path)), long);
        assert_eq!(read(&path), "next\n");
    }

    #[test]
    fn open_fails_when_the_folder_cannot_be_made() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-folder");
        fs::write(&blocker, "").unwrap();

        let result = RotatingFile::open(&blocker.join("logs").join("sulsul.log"), 1024);
        assert!(
            result.is_err(),
            "파일 아래에는 로그 폴더를 만들 수 없습니다"
        );
    }

    #[test]
    fn events_are_written_without_color_codes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sulsul.log");
        let file = RotatingFile::open(&path, MAX_FILE_BYTES).unwrap();
        let subscriber = tracing_subscriber::registry().with(file_layer(file));

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(chars = 12, "dictation copied to the clipboard");
        });

        let text = read(&path);
        assert!(
            text.contains("WARN") && text.contains("dictation copied to the clipboard chars=12"),
            "로그 줄이 파일에 남아야 합니다: {text:?}"
        );
        assert!(
            !text.contains('\u{1b}'),
            "파일에는 색 코드가 없어야 합니다: {text:?}"
        );
    }
}
