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
use sulsul_core::config::AudioConfig;
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
    },
    /// WAV 파일을 받아쓰고 걸린 시간을 보여 줍니다
    Transcribe(transcribe::Args),
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
        } => record(seconds, output, device, native_rate),
        Command::Transcribe(args) => transcribe::run(args),
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

fn record(seconds: f64, output: PathBuf, device: Option<String>, native_rate: bool) -> Result<()> {
    if !(seconds.is_finite() && seconds > 0.0) {
        bail!("녹음 길이는 0보다 커야 합니다");
    }
    let duration = Duration::from_secs_f64(seconds);
    let audio_config = configured_audio();
    let device = device.unwrap_or(audio_config.input_device);

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
    if audio_config.auto_gain {
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

/// 설정 파일이 있으면 그 오디오 설정, 없으면 기본값
fn configured_audio() -> AudioConfig {
    match AppPaths::resolve() {
        Ok(paths) => load_config(&paths).audio,
        Err(_) => AudioConfig::default(),
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
