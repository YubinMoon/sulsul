//! 엔진을 고르는 공통 옵션(`--engine`, `--set`)과 설정·레지스트리 준비.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use sulsul_core::AppPaths;
use sulsul_core::config::SttConfig;
use sulsul_core::settings::SettingProblem;
use sulsul_core::stt::mock::MockEngineFactory;
use sulsul_core::stt::{EngineContext, ModelRequirement, SttRegistry};
use sulsul_core::toml::Value;

use crate::load_config;

#[derive(clap::Args)]
pub struct EngineArgs {
    /// 엔진 ID (예: whisper_local, mock). 생략하면 설정 파일 값
    #[arg(short, long)]
    engine: Option<String>,
    /// 이번 실행에만 덮어쓸 엔진 설정 (예: --set use_gpu=false). 여러 번 쓸 수 있습니다
    #[arg(long = "set", value_name = "KEY=VALUE", value_parser = parse_setting)]
    settings: Vec<(String, Value)>,
}

/// 고른 엔진과 그 엔진을 다루는 데 필요한 것들
pub struct Selected {
    pub registry: SttRegistry,
    pub context: EngineContext,
    /// 설정 파일의 `[stt]`에 `--set` 값을 덮어쓴 것
    pub stt: SttConfig,
    pub engine_id: String,
}

impl EngineArgs {
    /// 설정 파일이 있으면 읽고, 엔진을 정하고, `--set` 값을 덮어씁니다.
    pub fn select(self) -> Result<Selected> {
        let paths = AppPaths::resolve().context("앱 폴더 위치를 정하지 못했습니다")?;
        let mut stt = load_config(&paths).stt;
        let engine_id = self.engine.unwrap_or_else(|| stt.engine.clone());

        let registry = registry();
        if registry.descriptor(&engine_id).is_none() {
            let known: Vec<_> = registry.descriptors().map(|d| d.id.as_str()).collect();
            bail!(
                "'{engine_id}' 엔진을 모릅니다. 쓸 수 있는 엔진: {}",
                known.join(", ")
            );
        }

        if !self.settings.is_empty() {
            let table = stt.engines.entry(engine_id.clone()).or_default();
            for (key, value) in self.settings {
                table.insert(key, value);
            }
        }

        Ok(Selected {
            registry,
            context: EngineContext { paths },
            stt,
            engine_id,
        })
    }
}

impl Selected {
    /// 지금 설정으로 이 엔진이 쓸 모델 파일 (모델 파일을 쓰지 않는 엔진이면 `None`).
    /// 잘못된 설정 값은 기본값으로 보고 정합니다.
    pub fn model(&self) -> Result<Option<ModelRequirement>> {
        self.registry
            .model(&self.context, &self.stt, &self.engine_id)
            .context("엔진 설정을 확인하지 못했습니다")
    }

    /// 엔진 설정에 쓸 수 없는 값이 있으면 그 설명. 이런 값이 있으면 엔진을 만들 수 없습니다.
    pub fn invalid_settings(&self) -> Option<String> {
        let schema = &self.registry.descriptor(&self.engine_id)?.settings;
        let table = self.stt.engines.get(&self.engine_id)?;
        let issues: Vec<_> = self
            .registry
            .validate(&self.stt, &self.engine_id)
            .ok()?
            .into_iter()
            .filter(|issue| issue.problem != SettingProblem::UnknownKey)
            .collect();
        let first = issues.first()?;

        let default_of = |key: &str| {
            schema
                .iter()
                .find(|field| field.key == key)
                .map(|field| plain(&field.default))
                .unwrap_or_default()
        };
        let mut text = format!(
            "엔진 설정 [stt.engines.{}]에 쓸 수 없는 값이 있습니다:\n",
            self.engine_id
        );
        for issue in &issues {
            let value = table.get(&issue.key).map(plain).unwrap_or_default();
            text.push_str(&format!(
                "  {} = {value}: {} (기본값 {})\n",
                issue.key,
                problem_text(&issue.problem),
                default_of(&issue.key)
            ));
        }
        text.push_str(&format!(
            "설정 파일을 고치거나 이번 실행에만 --set {}={}처럼 덮어쓰세요.",
            first.key,
            default_of(&first.key)
        ));
        Some(text)
    }
}

/// Whisper와 가짜 엔진을 등록한 레지스트리
fn registry() -> SttRegistry {
    let mut registry = SttRegistry::new();
    registry.register(sulsul_stt_whisper::factory());
    registry.register(Arc::new(MockEngineFactory::new()));
    registry
}

/// 따옴표 없이 보여 주는 설정 값
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn problem_text(problem: &SettingProblem) -> String {
    match problem {
        SettingProblem::WrongType { expected } => {
            format!("형식이 다릅니다({expected}이어야 함)")
        }
        SettingProblem::OutOfRange { min, max } => format!("{min} ~ {max} 사이여야 합니다"),
        SettingProblem::UnknownChoice(_) => "고를 수 없는 값입니다".to_owned(),
        SettingProblem::UnknownKey => "모르는 항목입니다".to_owned(),
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
    use sulsul_core::stt::mock::MOCK_ENGINE_ID;

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

    fn selected(table: &[(&str, Value)]) -> Selected {
        let mut stt = SttConfig::default();
        stt.engines.insert(
            MOCK_ENGINE_ID.to_owned(),
            table
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        );
        Selected {
            registry: registry(),
            context: EngineContext {
                paths: AppPaths::under("unused"),
            },
            stt,
            engine_id: MOCK_ENGINE_ID.to_owned(),
        }
    }

    #[test]
    fn valid_or_unknown_keys_are_not_problems() {
        assert_eq!(selected(&[]).invalid_settings(), None);
        // 스키마에 없는 키는 엔진을 만드는 데 지장이 없습니다.
        let extra = selected(&[("extra", Value::Integer(1))]);
        assert_eq!(extra.invalid_settings(), None);
    }

    #[test]
    fn invalid_settings_explain_default_and_override() {
        let text = selected(&[("delay_ms", Value::Integer(-1))])
            .invalid_settings()
            .unwrap();
        let expected = [
            "엔진 설정 [stt.engines.mock]에 쓸 수 없는 값이 있습니다:",
            "  delay_ms = -1: 0 ~ 10000 사이여야 합니다 (기본값 300)",
            "설정 파일을 고치거나 이번 실행에만 --set delay_ms=300처럼 덮어쓰세요.",
        ];
        assert_eq!(text, expected.join("\n"));
    }

    #[test]
    fn problem_texts() {
        assert_eq!(
            problem_text(&SettingProblem::UnknownChoice("x".into())),
            "고를 수 없는 값입니다"
        );
        assert_eq!(
            problem_text(&SettingProblem::WrongType {
                expected: "boolean"
            }),
            "형식이 다릅니다(boolean이어야 함)"
        );
    }
}
