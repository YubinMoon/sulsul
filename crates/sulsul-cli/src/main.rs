//! 술술 개발용 명령줄 도구.
//!
//! 사용자용 화면이 아니라 개발 중 기능을 빠르게 확인하는 용도라 문구를 번역하지 않습니다.

mod engines;
mod models;
mod transcribe;

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sulsul_core::audio::{self, Recording, RecordingOptions};
use sulsul_core::stt::WHISPER_SAMPLE_RATE;
use sulsul_core::{AppPaths, Config, ConfigStore};

#[derive(Parser)]
#[command(name = "sulsul-cli", version, about = "술술 개발용 명령줄 도구")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 입력 장치 목록을 보여 줍니다
    Devices,
    /// 마이크로 녹음해 WAV 파일로 저장합니다
    Record {
        /// 녹음 길이(초)
        #[arg(short, long, default_value_t = 5.0)]
        seconds: f64,
        /// 저장할 파일
        #[arg(short, long, default_value = "recording.wav")]
        output: PathBuf,
        /// 장치 ID 또는 이름. 생략하면 설정 파일 값, 그것도 비어 있으면 기본 장치
        #[arg(short, long)]
        device: Option<String>,
        /// 16kHz로 바꾸지 않고 장치 샘플레이트 그대로 저장
        #[arg(long)]
        native_rate: bool,
        /// 자동 음량 조절을 하지 않고 저장 (앱은 늘 합니다)
        #[arg(long)]
        no_auto_gain: bool,
        /// 앞뒤 조용한 부분을 자르지 않고 저장 (앱은 늘 자릅니다). `speech` 확인용 녹음을 만들 때 씁니다
        #[arg(long)]
        no_trim_silence: bool,
    },
    /// WAV 파일을 받아쓰고 걸린 시간을 보여 줍니다
    Transcribe(transcribe::Args),
    /// WAV 파일에서 받아쓰기가 남길 말소리 범위를 찾아 보여 줍니다 (앞뒤 조용한 부분 자르기 확인용)
    Speech {
        /// 살펴볼 WAV 파일들
        #[arg(required = true)]
        wavs: Vec<PathBuf>,
    },
    /// 엔진이 쓰는 모델 파일을 확인하고 내려받습니다
    Models {
        #[command(subcommand)]
        command: models::Command,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().command {
        Command::Devices => devices(),
        Command::Record {
            seconds,
            output,
            device,
            native_rate,
            no_auto_gain,
            no_trim_silence,
        } => record(
            seconds,
            output,
            device,
            native_rate,
            Processing {
                auto_gain: !no_auto_gain,
                trim_silence: !no_trim_silence,
            },
        ),
        Command::Transcribe(args) => transcribe::run(args),
        Command::Speech { wavs } => speech(&wavs),
        Command::Models { command } => models::run(command),
    }
}

fn devices() -> Result<()> {
    let devices = audio::input_devices().context("입력 장치 목록을 가져오지 못했습니다")?;
    if devices.is_empty() {
        println!("입력 장치가 없습니다.");
        return Ok(());
    }
    for device in devices {
        let marker = if device.is_default { "*" } else { " " };
        let format = match (device.sample_rate, device.channels) {
            (Some(rate), Some(ch)) => format!("{rate} Hz, {ch}ch"),
            _ => "형식 알 수 없음".into(),
        };
        println!("{marker} {}  ({format})", device.name);
        println!("    id: {}", device.id);
    }
    println!("\n* 기본 장치");
    Ok(())
}

/// 녹음을 마친 뒤 앱의 받아쓰기처럼 손볼지 (앱은 둘 다 늘 합니다)
struct Processing {
    auto_gain: bool,
    trim_silence: bool,
}

fn record(
    seconds: f64,
    output: PathBuf,
    device: Option<String>,
    native_rate: bool,
    processing: Processing,
) -> Result<()> {
    if !(seconds.is_finite() && seconds > 0.0) {
        bail!("녹음 길이는 0보다 커야 합니다");
    }
    let duration = Duration::from_secs_f64(seconds);
    let device = device.unwrap_or_else(configured_device);

    let recording = Recording::start(&RecordingOptions {
        device,
        max_duration: Some(duration),
    })
    .context("녹음을 시작하지 못했습니다")?;
    println!(
        "녹음 중: {} ({} Hz, {}ch), {seconds:.1}초",
        recording.device_name(),
        recording.sample_rate(),
        recording.channels(),
    );

    // 장치가 데이터를 보내지 않는 경우에도 끝나도록 실제 시간 기준 상한을 둡니다.
    let deadline = Instant::now() + duration + Duration::from_secs(2);
    let mut stdout = std::io::stdout();
    while !recording.limit_reached() && Instant::now() < deadline {
        if let Some(error) = recording.error() {
            bail!("녹음 중 장치 오류: {error}");
        }
        print!(
            "\r  {} {:>5.1}s",
            meter(recording.level(), 30),
            recording.elapsed().as_secs_f64()
        );
        stdout.flush().ok();
        std::thread::sleep(Duration::from_millis(50));
    }
    println!();

    let mut clip = recording.stop()?;
    // 앱의 받아쓰기와 같은 순서: 자르기 전의 녹음 전체로 말소리를 찾고 음량을 맞춘 뒤 자릅니다.
    let speech = if processing.trim_silence {
        Some(sulsul_vad::find_speech(&clip).context("말소리를 찾지 못했습니다")?)
    } else {
        None
    };
    if processing.auto_gain {
        let gain = clip.auto_gain();
        if gain.applied() {
            println!(
                "자동 음량 조절: +{:.1} dB (말소리 {:.1} dBFS, 소음 {:.1} dBFS)",
                gain.gain_db, gain.speech_dbfs, gain.noise_dbfs
            );
        } else {
            println!(
                "자동 음량 조절: 키우지 않음 (말소리 {:.1} dBFS, 소음 {:.1} dBFS)",
                gain.speech_dbfs, gain.noise_dbfs
            );
        }
    }
    match speech {
        Some(Some(speech)) => {
            let before = clip.duration();
            println!("말소리 구간: {}", segments_text(&speech, clip.sample_rate));
            clip.keep_range(speech.range.clone());
            println!(
                "앞뒤 조용한 부분 자르기: {} 남김 ({:.2}초 잘라 냄)",
                seconds_text(&speech.range, clip.sample_rate),
                before.saturating_sub(clip.duration()).as_secs_f64(),
            );
        }
        Some(None) => println!(
            "말소리를 찾지 못했습니다. 받아쓰기 단축키는 이런 녹음을 받아쓰지 않습니다 (녹음은 자르지 않고 저장)."
        ),
        None => {}
    }
    let clip = if native_rate {
        clip
    } else {
        clip.resampled(WHISPER_SAMPLE_RATE)?
    };
    audio::write_wav(&output, &clip)?;

    println!(
        "저장: {} ({:.2}초, {} Hz, 최대 {:.0}%, RMS {:.1} dBFS)",
        output.display(),
        clip.duration().as_secs_f64(),
        clip.sample_rate,
        clip.peak() * 100.0,
        dbfs(clip.rms()),
    );
    if dbfs(clip.rms()) < SILENCE_DBFS {
        println!(
            "소리가 거의 들어오지 않았습니다. 입력 장치와 마이크 권한(Windows 설정 > 개인 정보 > 마이크)을 확인하세요."
        );
    }
    Ok(())
}

fn speech(wavs: &[PathBuf]) -> Result<()> {
    for path in wavs {
        let clip = audio::read_wav(path)
            .with_context(|| format!("{}을(를) 읽지 못했습니다", path.display()))?;
        let seconds = clip.duration().as_secs_f64();
        match sulsul_vad::find_speech(&clip).context("말소리를 찾지 못했습니다")? {
            Some(span) => println!(
                "{}: {seconds:.2}초 중 {} 남김 (말소리 구간: {})",
                path.display(),
                seconds_text(&span.range, clip.sample_rate),
                segments_text(&span, clip.sample_rate),
            ),
            None => println!(
                "{}: {seconds:.2}초, 말소리 없음 (받아쓰기 단축키라면 받아쓰지 않음)",
                path.display()
            ),
        }
    }
    Ok(())
}

/// 샘플 범위를 "1.20~3.40초"로
fn seconds_text(range: &std::ops::Range<usize>, sample_rate: u32) -> String {
    let rate = f64::from(sample_rate);
    format!(
        "{:.2}~{:.2}초",
        range.start as f64 / rate,
        range.end as f64 / rate
    )
}

/// 말로 본 구간들을 "0.30~1.20초, 2.00~3.10초"로
fn segments_text(span: &sulsul_vad::SpeechSpan, sample_rate: u32) -> String {
    span.segments
        .iter()
        .map(|segment| seconds_text(segment, sample_rate))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 설정 파일이 있으면 그 입력 장치, 없으면 기본 장치(빈 문자열)
fn configured_device() -> String {
    match AppPaths::resolve() {
        Ok(paths) => load_config(&paths).audio.input_device,
        Err(_) => String::new(),
    }
}

/// 설정 파일이 있으면 그 내용, 없거나 읽지 못하면 기본값
fn load_config(paths: &AppPaths) -> Config {
    let store = ConfigStore::new(paths.config_file());
    if !store.path().exists() {
        return Config::default();
    }
    match store.load() {
        Ok(loaded) => loaded.config,
        Err(e) => {
            tracing::warn!("설정 파일을 읽지 못해 기본값을 씁니다: {e}");
            Config::default()
        }
    }
}

/// 이보다 조용하면 소리가 들어오지 않은 것으로 봅니다 (일반적인 방 소음은 -60 ~ -45 dBFS)
const SILENCE_DBFS: f32 = -60.0;

fn dbfs(level: f32) -> f32 {
    20.0 * level.max(1e-6).log10()
}

/// -60 ~ 0 dBFS를 막대로 표시
fn meter(level: f32, width: usize) -> String {
    let ratio = ((dbfs(level) + 60.0) / 60.0).clamp(0.0, 1.0);
    let filled = (ratio * width as f32).round() as usize;
    format!("[{}{}]", "#".repeat(filled), "-".repeat(width - filled))
}
