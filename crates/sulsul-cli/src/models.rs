//! `models` 명령: 엔진이 쓰는 모델 파일을 확인하고 내려받습니다.
//!
//! 엔진이 알려 주는 정보([`ModelRequirement`])만 쓰므로 엔진마다 따로 코드를 두지 않습니다.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use sulsul_core::CancelToken;
use sulsul_core::download::{
    self, DownloadError, DownloadPhase, DownloadProgress, DownloadRequest,
};
use sulsul_core::stt::ModelRequirement;

use crate::engines::{EngineArgs, Selected};

#[derive(clap::Subcommand)]
pub enum Command {
    /// 지금 설정으로 엔진이 쓸 모델 파일과 받아져 있는지를 보여 줍니다
    Status(EngineArgs),
    /// 엔진이 쓸 모델 파일을 내려받습니다 (받다 만 파일이 있으면 이어받습니다)
    Download(EngineArgs),
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Status(args) => status(&args.select()?),
        Command::Download(args) => download_model(&args.select()?),
    }
}

fn status(selected: &Selected) -> Result<()> {
    println!("엔진: {}", selected.engine_id);
    if let Some(problem) = selected.invalid_settings() {
        // 엔진은 이 설정으로 만들어지지 않지만, 어떤 파일을 보게 되는지는 알려 줍니다.
        eprintln!("주의: {problem}");
        eprintln!("아래는 쓸 수 없는 값을 기본값으로 보고 정한 결과입니다.");
    }
    let Some(model) = selected.model()? else {
        println!("이 엔진은 모델 파일을 쓰지 않습니다.");
        return Ok(());
    };

    println!("모델 파일: {}", model.path.display());
    match std::fs::metadata(&model.path) {
        Ok(metadata) if metadata.is_file() => {
            println!("상태: 있음 ({})", size_text(metadata.len()));
        }
        _ => println!("상태: 없음"),
    }
    for extra in &model.extra_files {
        let state = match std::fs::metadata(&extra.path) {
            Ok(metadata) if metadata.is_file() => format!("있음 ({})", size_text(metadata.len())),
            _ => "없음".to_owned(),
        };
        println!("추가 파일: {} ({state})", extra.path.display());
    }
    match &model.asset {
        Some(asset) => {
            println!(
                "내려받을 수 있는 모델: {} ({})",
                asset.id,
                size_text(model.total_bytes().unwrap_or(asset.size_bytes))
            );
            println!("URL: {}", asset.url);
            let part = download::part_path(&model.path);
            if let Ok(metadata) = std::fs::metadata(&part) {
                println!(
                    "받다 만 파일: {} ({}, {})",
                    part.display(),
                    size_text(metadata.len()),
                    percent_text(metadata.len(), asset.size_bytes)
                );
            }
        }
        None => println!("직접 지정한 파일이라 내려받을 수 없습니다."),
    }
    Ok(())
}

fn download_model(selected: &Selected) -> Result<()> {
    // 잘못된 값을 기본값으로 바꿔 큰 파일을 받기 시작하면 헷갈리므로 먼저 멈춥니다.
    if let Some(problem) = selected.invalid_settings() {
        bail!("{problem}");
    }
    let Some(model) = selected.model()? else {
        println!(
            "'{}' 엔진은 모델 파일을 쓰지 않습니다. 받을 것이 없습니다.",
            selected.engine_id
        );
        return Ok(());
    };
    let requests = DownloadRequest::for_model_all(&model);
    if requests.is_empty() {
        bail!(
            "직접 지정한 모델 파일은 내려받을 수 없습니다: {}",
            model.path.display()
        );
    }
    if model.is_installed() {
        println!("이미 있습니다: {}", model.path.display());
        return Ok(());
    }

    announce(&model);
    for request in requests {
        if request.dest.is_file() {
            println!("이미 있습니다: {}", request.dest.display());
            continue;
        }
        eprintln!("받는 중: {} -> {}", request.url, request.dest.display());
        let mut display = ProgressDisplay::new(std::io::stderr().is_terminal());
        let result = download::download(&request, &CancelToken::new(), &mut |progress| {
            display.show(progress);
        });
        display.finish();
        match result {
            Ok(()) => println!("받았습니다: {}", request.dest.display()),
            Err(error) => bail!("내려받지 못했습니다: {error}\n{}", retry_hint(&error)),
        }
    }
    Ok(())
}

fn announce(model: &ModelRequirement) {
    if let Some(asset) = &model.asset {
        eprintln!("모델: {} ({})", asset.id, size_text(asset.size_bytes));
        eprintln!("받는 곳: {}", asset.url);
    }
    eprintln!("저장 위치: {}", model.path.display());
}

/// 실패한 뒤 다시 실행하면 어떻게 되는지
fn retry_hint(error: &DownloadError) -> &'static str {
    match error {
        // 검사에 실패한 파일은 지워지므로 이어받을 것이 없습니다.
        DownloadError::HashMismatch { .. } => {
            "받은 파일이 손상돼 지웠습니다. 다시 실행하면 처음부터 받습니다."
        }
        _ => "다시 실행하면 이어받습니다.",
    }
}

/// 진행률을 한 줄로 갱신합니다. 터미널이 아니면(파일로 저장 등) 가끔 한 줄씩 남깁니다.
struct ProgressDisplay {
    interactive: bool,
    /// 이번 실행에서 받기 시작한 시각과 그때 이미 받아져 있던 크기 (속도 계산용)
    started: Option<(Instant, u64)>,
    phase: Option<DownloadPhase>,
    last_line: Option<Instant>,
    /// 줄바꿈 없이 써 둔 줄이 있는지
    open_line: bool,
}

/// 터미널이 아닐 때 진행률을 남기는 간격
const LOG_INTERVAL: Duration = Duration::from_secs(10);

impl ProgressDisplay {
    fn new(interactive: bool) -> Self {
        Self {
            interactive,
            started: None,
            phase: None,
            last_line: None,
            open_line: false,
        }
    }

    fn show(&mut self, progress: DownloadProgress) {
        let phase_changed = self.phase != Some(progress.phase);
        if phase_changed {
            self.finish();
            self.phase = Some(progress.phase);
            self.started = Some((Instant::now(), progress.downloaded));
        }
        let speed = self
            .started
            .and_then(|(at, from)| speed(progress.downloaded.saturating_sub(from), at.elapsed()));
        let line = progress_line(&progress, speed);

        let mut stderr = std::io::stderr();
        if self.interactive {
            // 이전 줄이 더 길었을 때 남는 글자를 지우도록 뒤를 채웁니다.
            let _ = write!(stderr, "\r{line:<72}");
            let _ = stderr.flush();
            self.open_line = true;
        } else {
            let done = progress.total == Some(progress.downloaded);
            let due = self
                .last_line
                .is_none_or(|last| last.elapsed() >= LOG_INTERVAL);
            if phase_changed || done || due {
                let _ = writeln!(stderr, "{line}");
                self.last_line = Some(Instant::now());
            }
        }
    }

    /// 갱신하던 줄을 끝냅니다.
    fn finish(&mut self) {
        if std::mem::take(&mut self.open_line) {
            eprintln!();
        }
    }
}

/// 초당 바이트 수. 잴 만큼 시간이 지나지 않았으면 `None`.
fn speed(bytes: u64, elapsed: Duration) -> Option<f64> {
    let seconds = elapsed.as_secs_f64();
    (seconds >= 0.5).then(|| bytes as f64 / seconds)
}

/// 진행률 한 줄 (예: `내려받는 중: 1.20 GiB / 2.88 GiB (41.6%), 23.5 MiB/s`)
fn progress_line(progress: &DownloadProgress, speed: Option<f64>) -> String {
    let label = match progress.phase {
        DownloadPhase::Downloading => "내려받는 중",
        DownloadPhase::Verifying => "파일 검사 중",
    };
    let mut line = match progress.total {
        Some(total) => format!(
            "{label}: {} / {} ({})",
            bytes_text(progress.downloaded),
            bytes_text(total),
            percent_text(progress.downloaded, total)
        ),
        None => format!("{label}: {}", bytes_text(progress.downloaded)),
    };
    if let Some(speed) = speed {
        line.push_str(&format!(", {}/s", bytes_text(speed as u64)));
    }
    line
}

fn percent_text(done: u64, total: u64) -> String {
    if total == 0 {
        return "100.0%".to_owned();
    }
    format!("{:.1}%", done as f64 / total as f64 * 100.0)
}

/// 읽기 쉬운 크기 (예: `2.88 GiB`)
fn bytes_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit >= 2 {
        format!("{value:.2} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 읽기 쉬운 크기와 정확한 바이트 수 (예: `2.88 GiB, 3,095,033,483바이트`)
fn size_text(bytes: u64) -> String {
    format!("{}, {}바이트", bytes_text(bytes), grouped(bytes))
}

/// 세 자리마다 쉼표를 넣은 수
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const LARGE_V3: u64 = 3_095_033_483;

    #[test]
    fn bytes_are_readable() {
        assert_eq!(bytes_text(0), "0 B");
        assert_eq!(bytes_text(1023), "1023 B");
        assert_eq!(bytes_text(1024), "1.0 KiB");
        assert_eq!(bytes_text(5 * 1024 * 1024 + 512 * 1024), "5.5 MiB");
        assert_eq!(bytes_text(LARGE_V3), "2.88 GiB");
        assert_eq!(bytes_text(u64::MAX), "16777216.00 TiB");
    }

    #[test]
    fn numbers_are_grouped() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(LARGE_V3), "3,095,033,483");
        assert_eq!(size_text(LARGE_V3), "2.88 GiB, 3,095,033,483바이트");
    }

    #[test]
    fn percent() {
        assert_eq!(percent_text(0, LARGE_V3), "0.0%");
        assert_eq!(percent_text(LARGE_V3 / 2, LARGE_V3), "50.0%");
        assert_eq!(percent_text(LARGE_V3, LARGE_V3), "100.0%");
        assert_eq!(percent_text(0, 0), "100.0%");
    }

    #[test]
    fn speed_needs_some_time() {
        assert_eq!(speed(1000, Duration::from_millis(100)), None);
        assert_eq!(speed(3000, Duration::from_secs(2)), Some(1500.0));
    }

    #[test]
    fn progress_lines() {
        let downloading = DownloadProgress {
            downloaded: LARGE_V3 / 2,
            total: Some(LARGE_V3),
            phase: DownloadPhase::Downloading,
        };
        assert_eq!(
            progress_line(&downloading, Some(20.0 * 1024.0 * 1024.0)),
            "내려받는 중: 1.44 GiB / 2.88 GiB (50.0%), 20.0 MiB/s"
        );
        assert_eq!(
            progress_line(&downloading, None),
            "내려받는 중: 1.44 GiB / 2.88 GiB (50.0%)"
        );

        let unknown_total = DownloadProgress {
            total: None,
            ..downloading
        };
        assert_eq!(progress_line(&unknown_total, None), "내려받는 중: 1.44 GiB");

        let verifying = DownloadProgress {
            downloaded: LARGE_V3,
            total: Some(LARGE_V3),
            phase: DownloadPhase::Verifying,
        };
        assert_eq!(
            progress_line(&verifying, None),
            "파일 검사 중: 2.88 GiB / 2.88 GiB (100.0%)"
        );
    }

    #[test]
    fn retry_hints() {
        assert_eq!(
            retry_hint(&DownloadError::Network("reset".into())),
            "다시 실행하면 이어받습니다."
        );
        assert!(
            retry_hint(&DownloadError::HashMismatch {
                expected: String::new(),
                actual: String::new(),
            })
            .contains("처음부터")
        );
    }
}
