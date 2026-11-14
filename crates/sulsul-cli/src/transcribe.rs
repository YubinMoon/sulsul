//! `transcribe` 명령: WAV 파일을 엔진에 넣어 결과와 걸린 시간을 확인합니다.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sulsul_core::audio;
use sulsul_core::stt::mock::MockEngineFactory;
use sulsul_core::stt::{
    ComputeDevice, EngineContext, SttRegistry, TranscribeJob, Transcriber, Transcription,
};
use sulsul_core::toml::Value;
use sulsul_core::{AppPaths, CancelToken};

use crate::load_config;

#[derive(clap::Args)]
pub struct Args {
    /// 인식할 WAV 파일
    wav: PathBuf,
    /// 엔진 ID (예: whisper_local, mock). 생략하면 설정 파일 값
    #[arg(short, long)]
    engine: Option<String>,
    /// 언어 (ko, en, auto 등). 생략하면 설정 파일 값
    #[arg(short, long)]
    language: Option<String>,
    /// 이번 실행에만 덮어쓸 엔진 설정 (예: --set use_gpu=false). 여러 번 쓸 수 있습니다
    #[arg(long = "set", value_name = "KEY=VALUE", value_parser = parse_setting)]
    settings: Vec<(String, Value)>,
    /// 같은 엔진으로 반복할 횟수 (엔진 재사용 효과 확인용)
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    repeat: u32,
}

pub fn run(args: Args) -> Result<()> {
    let paths = AppPaths::resolve().context("앱 폴더 위치를 정하지 못했습니다")?;
    let mut stt = load_config(&paths).stt;
    let engine_id = args.engine.unwrap_or_else(|| stt.engine.clone());
    let language = args.language.unwrap_or_else(|| stt.language.clone());

    let mut registry = SttRegistry::new();
    registry.register(sulsul_stt_whisper::factory());
    registry.register(Arc::new(MockEngineFactory::new()));
    if registry.descriptor(&engine_id).is_none() {
        let known: Vec<_> = registry.descriptors().map(|d| d.id.as_str()).collect();
        bail!(
            "'{engine_id}' 엔진을 모릅니다. 쓸 수 있는 엔진: {}",
            known.join(", ")
        );
    }

    if !args.settings.is_empty() {
        let table = stt.engines.entry(engine_id.clone()).or_default();
        for (key, value) in args.settings {
            table.insert(key, value);
        }
    }

    let context = EngineContext { paths };
    let model = registry
        .model(&context, &stt, &engine_id)
        .context("엔진 설정을 확인하지 못했습니다")?;
    if let Some(model) = model
        && !model.is_installed()
    {
        bail!(
            "모델 파일이 없습니다: {}\n파일을 그 위치에 두거나 --set model_path=<파일>로 지정하세요.",
            model.path.display()
        );
    }

    let clip = audio::read_wav(&args.wav)
        .with_context(|| format!("WAV 파일을 읽지 못했습니다: {}", args.wav.display()))?;
    if clip.is_empty() {
        bail!("오디오가 비어 있습니다: {}", args.wav.display());
    }
    let audio_length = clip.duration();
    eprintln!("엔진: {engine_id} (언어 {language})");
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
            language: &language,
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
    }
}

/// `key=value`를 엔진 설정 값으로 해석합니다.
/// 값은 `true`/`false`면 불리언, 정수면 정수, 그 밖은 문자열입니다.
fn parse_setting(text: &str) -> Result<(String, Value), String> {
    let (key, value) = text
        .split_once('=')
        .ok_or_else(|| format!("'{text}'는 key=value 형식이 아닙니다"))?;
    let key = key.trim();
    if key.is_empty() {
        return Err(format!("'{text}'에 키가 없습니다"));
    }
    let value = match value {
        "true" => Value::Boolean(true),
        "false" => Value::Boolean(false),
        other => other
            .parse::<i64>()
            .map_or_else(|_| Value::String(other.to_owned()), Value::Integer),
    };
    Ok((key.to_owned(), value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> (String, Value) {
        parse_setting(text).unwrap()
    }

    #[test]
    fn setting_value_types() {
        assert_eq!(
            parsed("use_gpu=false"),
            ("use_gpu".into(), Value::Boolean(false))
        );
        assert_eq!(
            parsed("use_gpu=true"),
            ("use_gpu".into(), Value::Boolean(true))
        );
        assert_eq!(parsed("threads=4"), ("threads".into(), Value::Integer(4)));
        assert_eq!(
            parsed("delay_ms=-1"),
            ("delay_ms".into(), Value::Integer(-1))
        );
        assert_eq!(
            parsed("model=large-v3"),
            ("model".into(), Value::String("large-v3".into()))
        );
        // 대소문자가 다르거나 소수면 문자열로 둡니다.
        assert_eq!(parsed("x=True"), ("x".into(), Value::String("True".into())));
        assert_eq!(parsed("x=1.5"), ("x".into(), Value::String("1.5".into())));
    }

    #[test]
    fn setting_value_keeps_everything_after_the_first_equals() {
        assert_eq!(
            parsed("model_path=D:/models/a=b.bin"),
            (
                "model_path".into(),
                Value::String("D:/models/a=b.bin".into())
            )
        );
        assert_eq!(
            parsed("text="),
            ("text".into(), Value::String(String::new()))
        );
        assert_eq!(
            parsed("text=안녕 하세요 "),
            ("text".into(), Value::String("안녕 하세요 ".into()))
        );
    }

    #[test]
    fn setting_needs_key_and_equals() {
        assert!(parse_setting("use_gpu").is_err());
        assert!(parse_setting("=1").is_err());
        assert!(parse_setting(" =1").is_err());
    }
}
