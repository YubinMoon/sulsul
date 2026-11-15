//! `transcribe` 명령: WAV 파일을 엔진에 넣어 결과와 걸린 시간을 확인합니다.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sulsul_core::CancelToken;
use sulsul_core::audio;
use sulsul_core::stt::{ComputeDevice, SttError, TranscribeJob, Transcriber, Transcription};

use crate::engines::{EngineArgs, Selected};

#[derive(clap::Args)]
pub struct Args {
    /// 인식할 WAV 파일
    wav: PathBuf,
    #[command(flatten)]
    engine: EngineArgs,
    /// 이번 실행에만 쓸 언어 (auto, ko, en, zh, ja 가운데 모델이 지원하는 것). 생략하면 엔진 설정의 언어
    #[arg(short, long)]
    language: Option<String>,
    /// 같은 엔진으로 반복할 횟수 (엔진 재사용 효과 확인용)
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    repeat: u32,
}

pub fn run(args: Args) -> Result<()> {
    let selected = args.engine.select()?;

    // 잘못된 값이 있으면 엔진을 만들 수 없으므로 모델을 읽기 전에 알려 줍니다.
    if let Some(problem) = selected.invalid_settings() {
        bail!("{problem}");
    }
    // 받아쓰기와 같은 규칙으로 실제로 쓸 언어를 정합니다 (영어 전용 모델이면 영어).
    let language = match selected.registry.resolve_language(
        &selected.stt,
        &selected.engine_id,
        args.language.as_deref(),
    ) {
        Ok(language) => language,
        Err(SttError::UnsupportedLanguage(code)) => {
            let choices = selected
                .registry
                .languages(&selected.stt, &selected.engine_id)
                .map(|support| support.choices().join(", "))
                .unwrap_or_default();
            bail!(
                "'{code}' 언어는 지금 엔진·모델에서 인식할 수 없습니다.\n고를 수 있는 언어: {choices}"
            );
        }
        Err(error) => return Err(anyhow!("인식 언어를 정하지 못했습니다: {error}")),
    };
    if let Some(model) = selected.model()?
        && !model.is_installed()
    {
        let how = if model.asset.is_some() {
            "같은 --engine/--set 옵션으로 `sulsul-cli models download`를 실행해 내려받으세요."
        } else {
            "--set model_path=<파일>로 지정한 파일이 있는지 확인하세요."
        };
        bail!("모델 파일이 없습니다: {}\n{how}", model.path.display());
    }

    let clip = audio::read_wav(&args.wav)
        .with_context(|| format!("WAV 파일을 읽지 못했습니다: {}", args.wav.display()))?;
    if clip.is_empty() {
        bail!("오디오가 비어 있습니다: {}", args.wav.display());
    }
    let audio_length = clip.duration();

    let Selected {
        registry,
        context,
        stt,
        engine_id,
    } = selected;
    eprintln!("엔진: {engine_id} (언어 {})", language.as_config());
    eprintln!(
        "오디오: {:.2}초, {} Hz",
        audio_length.as_secs_f64(),
        clip.sample_rate
    );

    let mut transcriber = Transcriber::new(registry, context);
    for round in 1..=args.repeat {
        let job = TranscribeJob {
            stt: &stt,
            engine_id: &engine_id,
            language: args.language.as_deref(),
            cancel: CancelToken::new(),
        };
        let done = transcriber
            .transcribe(&clip, &job)
            .map_err(|error| anyhow!("받아쓰기에 실패했습니다: {error}"))?;
        if args.repeat > 1 {
            eprintln!("[{round}/{}]", args.repeat);
        }
        report(&done, audio_length);
        println!("{}", done.transcript.text);
    }
    Ok(())
}

/// 부가 정보를 stderr로 보여 줍니다.
fn report(done: &Transcription, audio_length: Duration) {
    eprintln!("장치: {}", device_text(&done.device));
    if done.reused {
        eprintln!("준비: 엔진 다시 씀");
    } else {
        eprintln!("준비: {:.3}초", done.prepare.as_secs_f64());
    }
    // 실시간 배율(RTF) = 인식 시간 / 오디오 길이. 낮을수록 빠릅니다.
    let rtf = done.recognize.as_secs_f64() / audio_length.as_secs_f64();
    eprintln!("인식: {:.3}초 (RTF {rtf:.3})", done.recognize.as_secs_f64());
    eprintln!(
        "감지된 언어: {}",
        done.transcript.language.as_deref().unwrap_or("알 수 없음")
    );
}

fn device_text(device: &ComputeDevice) -> String {
    match device {
        ComputeDevice::Unknown => "알 수 없음".to_owned(),
        ComputeDevice::Cpu => "CPU".to_owned(),
        ComputeDevice::Gpu(name) => format!("GPU ({name})"),
        ComputeDevice::Npu(name) => format!("NPU ({name})"),
    }
}
