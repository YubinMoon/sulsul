//! 엔진·후처리기별 설정 스키마.
//!
//! 각 구현체는 자기 설정 항목을 [`SettingField`] 목록으로 선언합니다. 실제 값은
//! TOML의 해당 테이블(예: `[stt.engines.whisper_local]`)에 저장되고,
//! GUI는 스키마만 보고 입력 화면을 자동으로 그립니다. 그래서 새 엔진을 추가해도
//! 설정 화면 코드를 고칠 필요가 없습니다.

use std::fmt;

use toml::{Table, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct SettingField {
    /// TOML 키
    pub key: String,
    /// 화면에 보일 이름의 번역 키
    pub label_key: String,
    /// 도움말 번역 키
    pub help_key: Option<String>,
    /// 이 빌드·환경에서 쓸 수 없는 항목이면 그 이유의 번역 키.
    ///
    /// 화면은 항목을 꺼진 상태로 잠그고 이유를 보여 줍니다. 저장된 값은 건드리지 않으므로
    /// 쓸 수 있는 빌드로 바꾸면 사용자가 정한 값이 그대로 적용됩니다.
    pub unavailable_key: Option<String>,
    pub kind: SettingKind,
    pub default: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingKind {
    Bool,
    Integer {
        min: i64,
        max: i64,
    },
    Text {
        multiline: bool,
    },
    /// API 키처럼 화면에서 가려야 하는 값
    Secret,
    Path {
        kind: PathKind,
    },
    Choice {
        options: Vec<ChoiceOption>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceOption {
    pub value: String,
    /// 없으면 `value`를 그대로 표시
    pub label_key: Option<String>,
    /// 번역하지 않는 부가 정보 (예: "547 MB")
    pub detail: Option<String>,
    /// 이 빌드·환경에서 고를 수 없는 선택지면 그 이유의 번역 키.
    ///
    /// 화면은 선택지를 꺼진 상태로 보여 주고 이유를 알려 줍니다. 저장된 값은 건드리지 않고
    /// 검사([`SettingField::check`])도 통과시키므로, 그 선택지를 어떻게 처리할지는 엔진이 정합니다.
    pub unavailable_key: Option<String>,
}

impl ChoiceOption {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label_key: None,
            detail: None,
            unavailable_key: None,
        }
    }

    pub fn with_label(mut self, label_key: impl Into<String>) -> Self {
        self.label_key = Some(label_key.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// 이 빌드·환경에서 고를 수 없는 선택지로 표시합니다 ([`ChoiceOption::unavailable_key`]).
    pub fn unavailable(mut self, reason_key: impl Into<String>) -> Self {
        self.unavailable_key = Some(reason_key.into());
        self
    }
}

impl SettingField {
    fn new(
        key: impl Into<String>,
        label_key: impl Into<String>,
        kind: SettingKind,
        default: Value,
    ) -> Self {
        Self {
            key: key.into(),
            label_key: label_key.into(),
            help_key: None,
            unavailable_key: None,
            kind,
            default,
        }
    }

    pub fn bool(key: impl Into<String>, label_key: impl Into<String>, default: bool) -> Self {
        Self::new(key, label_key, SettingKind::Bool, Value::Boolean(default))
    }

    pub fn integer(
        key: impl Into<String>,
        label_key: impl Into<String>,
        default: i64,
        min: i64,
        max: i64,
    ) -> Self {
        Self::new(
            key,
            label_key,
            SettingKind::Integer { min, max },
            Value::Integer(default),
        )
    }

    pub fn text(
        key: impl Into<String>,
        label_key: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        Self::new(
            key,
            label_key,
            SettingKind::Text { multiline: false },
            Value::String(default.into()),
        )
    }

    pub fn multiline_text(
        key: impl Into<String>,
        label_key: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        Self::new(
            key,
            label_key,
            SettingKind::Text { multiline: true },
            Value::String(default.into()),
        )
    }

    pub fn secret(key: impl Into<String>, label_key: impl Into<String>) -> Self {
        Self::new(
            key,
            label_key,
            SettingKind::Secret,
            Value::String(String::new()),
        )
    }

    pub fn path(key: impl Into<String>, label_key: impl Into<String>, kind: PathKind) -> Self {
        Self::new(
            key,
            label_key,
            SettingKind::Path { kind },
            Value::String(String::new()),
        )
    }

    pub fn choice(
        key: impl Into<String>,
        label_key: impl Into<String>,
        default: impl Into<String>,
        options: Vec<ChoiceOption>,
    ) -> Self {
        Self::new(
            key,
            label_key,
            SettingKind::Choice { options },
            Value::String(default.into()),
        )
    }

    pub fn with_help(mut self, help_key: impl Into<String>) -> Self {
        self.help_key = Some(help_key.into());
        self
    }

    /// 이 빌드·환경에서 쓸 수 없는 항목으로 표시합니다 ([`SettingField::unavailable_key`]).
    pub fn unavailable(mut self, reason_key: impl Into<String>) -> Self {
        self.unavailable_key = Some(reason_key.into());
        self
    }

    /// 값이 이 항목의 타입·범위에 맞는지 검사합니다.
    pub fn check(&self, value: &Value) -> Result<(), SettingProblem> {
        match (&self.kind, value) {
            (SettingKind::Bool, Value::Boolean(_)) => Ok(()),
            (SettingKind::Bool, _) => Err(SettingProblem::WrongType {
                expected: "boolean",
            }),
            (SettingKind::Integer { min, max }, Value::Integer(v)) => {
                if (*min..=*max).contains(v) {
                    Ok(())
                } else {
                    Err(SettingProblem::OutOfRange {
                        min: *min,
                        max: *max,
                    })
                }
            }
            (SettingKind::Integer { .. }, _) => Err(SettingProblem::WrongType {
                expected: "integer",
            }),
            (SettingKind::Choice { options }, Value::String(v)) => {
                if options.iter().any(|o| &o.value == v) {
                    Ok(())
                } else {
                    Err(SettingProblem::UnknownChoice(v.clone()))
                }
            }
            (
                SettingKind::Text { .. } | SettingKind::Secret | SettingKind::Path { .. },
                Value::String(_),
            ) => Ok(()),
            (
                SettingKind::Choice { .. }
                | SettingKind::Text { .. }
                | SettingKind::Secret
                | SettingKind::Path { .. },
                _,
            ) => Err(SettingProblem::WrongType { expected: "string" }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingIssue {
    pub key: String,
    pub problem: SettingProblem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingProblem {
    WrongType { expected: &'static str },
    OutOfRange { min: i64, max: i64 },
    UnknownChoice(String),
    UnknownKey,
}

impl fmt::Display for SettingIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.problem {
            SettingProblem::WrongType { expected } => {
                write!(f, "{}: expected {expected}", self.key)
            }
            SettingProblem::OutOfRange { min, max } => {
                write!(f, "{}: must be between {min} and {max}", self.key)
            }
            SettingProblem::UnknownChoice(v) => write!(f, "{}: unknown option '{v}'", self.key),
            SettingProblem::UnknownKey => write!(f, "{}: unknown key", self.key),
        }
    }
}

/// 스키마 기본값으로 채운 테이블
pub fn defaults(schema: &[SettingField]) -> Table {
    schema
        .iter()
        .map(|f| (f.key.clone(), f.default.clone()))
        .collect()
}

/// 저장된 값을 검사합니다. 스키마에 없는 키도 경고로 알려 줍니다.
pub fn validate(schema: &[SettingField], table: &Table) -> Vec<SettingIssue> {
    let mut issues = Vec::new();
    for (key, value) in table {
        match schema.iter().find(|f| &f.key == key) {
            Some(field) => {
                if let Err(problem) = field.check(value) {
                    issues.push(SettingIssue {
                        key: key.clone(),
                        problem,
                    });
                }
            }
            None => issues.push(SettingIssue {
                key: key.clone(),
                problem: SettingProblem::UnknownKey,
            }),
        }
    }
    issues
}

/// 기본값 위에 저장된 값을 덮어씁니다. 잘못된 값은 기본값으로 대체됩니다.
pub fn resolve(schema: &[SettingField], table: Option<&Table>) -> ResolvedSettings {
    let mut values = defaults(schema);
    if let Some(table) = table {
        for field in schema {
            if let Some(value) = table.get(&field.key)
                && field.check(value).is_ok()
            {
                values.insert(field.key.clone(), value.clone());
            }
        }
    }
    ResolvedSettings(values)
}

/// 스키마 기준으로 검증·보정된 설정 값
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedSettings(Table);

impl ResolvedSettings {
    pub fn from_table(table: Table) -> Self {
        Self(table)
    }

    pub fn set(&mut self, key: impl Into<String>, value: Value) {
        self.0.insert(key.into(), value);
    }

    pub fn into_table(self) -> Table {
        self.0
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn bool(&self, key: &str) -> bool {
        self.0.get(key).and_then(Value::as_bool).unwrap_or_default()
    }

    pub fn int(&self, key: &str) -> i64 {
        self.0
            .get(key)
            .and_then(Value::as_integer)
            .unwrap_or_default()
    }

    pub fn str(&self, key: &str) -> &str {
        self.0.get(key).and_then(Value::as_str).unwrap_or_default()
    }

    pub fn as_table(&self) -> &Table {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Vec<SettingField> {
        vec![
            SettingField::bool("use_gpu", "k.gpu", true),
            SettingField::integer("threads", "k.threads", 0, 0, 64),
            SettingField::choice(
                "model",
                "k.model",
                "small",
                vec![ChoiceOption::new("small"), ChoiceOption::new("large")],
            ),
        ]
    }

    #[test]
    fn resolve_fills_defaults_and_skips_invalid() {
        let mut table = Table::new();
        table.insert("threads".into(), Value::Integer(8));
        table.insert("model".into(), Value::String("huge".into()));

        let resolved = resolve(&schema(), Some(&table));
        assert!(resolved.bool("use_gpu"));
        assert_eq!(resolved.int("threads"), 8);
        assert_eq!(resolved.str("model"), "small");
    }

    #[test]
    fn validate_reports_problems() {
        let mut table = Table::new();
        table.insert("threads".into(), Value::Integer(100));
        table.insert("use_gpu".into(), Value::String("yes".into()));
        table.insert("typo".into(), Value::Boolean(true));

        let issues = validate(&schema(), &table);
        assert_eq!(issues.len(), 3);
        assert!(issues.contains(&SettingIssue {
            key: "threads".into(),
            problem: SettingProblem::OutOfRange { min: 0, max: 64 },
        }));
        assert!(issues.contains(&SettingIssue {
            key: "typo".into(),
            problem: SettingProblem::UnknownKey,
        }));
    }
}
