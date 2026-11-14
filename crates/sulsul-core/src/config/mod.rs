//! 앱 설정.
//!
//! TOML 파일이 설정의 기준입니다. GUI는 이 파일을 읽어 화면에 보여 주고,
//! 값을 바꾸면 [`ConfigStore::save`]로 파일에 씁니다(주석과 순서 보존).
//! 앱 실행 중 파일을 직접 수정하면 [`ConfigStore::watch`]로 감지해 다시 불러옵니다.

mod merge;
mod store;

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub use store::{ConfigError, ConfigStore, ConfigWatcher, LoadedConfig};

pub const CONFIG_VERSION: u32 = 1;

/// 새 설정 파일의 내용 (주석 포함)
pub const DEFAULT_CONFIG_TOML: &str = include_str!("default.toml");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub config_version: u32,
    pub general: GeneralConfig,
    pub hotkey: HotkeyConfig,
    pub audio: AudioConfig,
    pub stt: SttConfig,
    pub output: OutputConfig,
    pub postprocess: PostProcessConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            general: GeneralConfig::default(),
            hotkey: HotkeyConfig::default(),
            audio: AudioConfig::default(),
            stt: SttConfig::default(),
            output: OutputConfig::default(),
            postprocess: PostProcessConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// 화면 언어 (`locales/<코드>.yml`)
    pub ui_language: String,
    pub theme: Theme,
    pub launch_at_startup: bool,
    pub start_minimized: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            ui_language: "ko".into(),
            theme: Theme::Dark,
            launch_at_startup: false,
            start_minimized: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    /// 단일 키(`RightCtrl`) 또는 조합(`Ctrl+Shift+Space`)
    pub key: String,
    pub mode: HotkeyMode,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            key: "RightCtrl".into(),
            mode: HotkeyMode::Toggle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyMode {
    /// 한 번 눌러 시작, 다시 눌러 종료
    #[default]
    Toggle,
    /// 누르고 있는 동안만 녹음
    PushToTalk,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// 입력 장치 ID(권장) 또는 이름. 비어 있으면 시스템 기본 장치
    pub input_device: String,
    pub max_record_seconds: u32,
    pub feedback_sounds: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: String::new(),
            max_record_seconds: 120,
            feedback_sounds: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// 사용할 엔진 ID ([`crate::stt::SttRegistry`]에 등록된 것)
    pub engine: String,
    /// 인식 언어 (`ko`, `en`, `auto`)
    pub language: String,
    /// 용어 힌트
    pub initial_prompt: String,
    /// 엔진별 설정 (`[stt.engines.<ID>]`). 값 검증은 각 엔진의 스키마가 담당합니다.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub engines: BTreeMap<String, toml::Table>,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: "whisper_local".into(),
            language: "ko".into(),
            initial_prompt: String::new(),
            engines: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub mode: OutputMode,
    /// 붙여넣기 후 원래 클립보드 내용을 되돌릴지
    pub restore_clipboard: bool,
    /// 붙여넣기 후 클립보드를 되돌리기까지 기다리는 시간
    pub restore_delay_ms: u32,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            mode: OutputMode::Paste,
            restore_clipboard: true,
            restore_delay_ms: 150,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// 클립보드를 거쳐 커서 위치에 붙여넣기
    #[default]
    Paste,
    /// 키 입력으로 한 글자씩 입력
    Type,
    /// 클립보드에 복사만
    Clipboard,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PostProcessConfig {
    pub enabled: bool,
    /// 사용할 후처리기 ID (비어 있으면 사용 안 함)
    pub processor: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub processors: BTreeMap<String, toml::Table>,
}

/// 설정 값 문제 (파일은 읽혔지만 일부 값이 이상한 경우)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    /// 점으로 구분한 키 경로 (예: `audio.max_record_seconds`)
    pub path: String,
    pub problem: ConfigProblem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigProblem {
    /// 알 수 없는 키 (오타일 가능성)
    UnknownKey,
    OutOfRange {
        min: i64,
        max: i64,
    },
    Empty,
}

impl fmt::Display for ConfigIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.problem {
            ConfigProblem::UnknownKey => write!(f, "{}: unknown key", self.path),
            ConfigProblem::OutOfRange { min, max } => {
                write!(f, "{}: must be between {min} and {max}", self.path)
            }
            ConfigProblem::Empty => write!(f, "{}: must not be empty", self.path),
        }
    }
}

pub const MAX_RECORD_SECONDS: (i64, i64) = (1, 3600);
pub const RESTORE_DELAY_MS: (i64, i64) = (0, 5000);

impl Config {
    /// 값 범위 등을 검사합니다. 엔진별 설정은 [`crate::stt::SttRegistry::validate`]가 담당합니다.
    pub fn validate(&self) -> Vec<ConfigIssue> {
        let mut issues = Vec::new();
        let mut range = |path: &str, value: i64, (min, max): (i64, i64)| {
            if !(min..=max).contains(&value) {
                issues.push(ConfigIssue {
                    path: path.into(),
                    problem: ConfigProblem::OutOfRange { min, max },
                });
            }
        };
        range(
            "audio.max_record_seconds",
            i64::from(self.audio.max_record_seconds),
            MAX_RECORD_SECONDS,
        );
        range(
            "output.restore_delay_ms",
            i64::from(self.output.restore_delay_ms),
            RESTORE_DELAY_MS,
        );
        for (path, value) in [
            ("general.ui_language", &self.general.ui_language),
            ("hotkey.key", &self.hotkey.key),
            ("stt.engine", &self.stt.engine),
        ] {
            if value.trim().is_empty() {
                issues.push(ConfigIssue {
                    path: path.into(),
                    problem: ConfigProblem::Empty,
                });
            }
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_template_matches_default_config() {
        let parsed: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        assert_eq!(parsed, Config::default());
        assert!(parsed.validate().is_empty());
    }

    #[test]
    fn missing_sections_use_defaults() {
        let parsed: Config = toml::from_str("[hotkey]\nmode = \"push_to_talk\"\n").unwrap();
        assert_eq!(parsed.hotkey.mode, HotkeyMode::PushToTalk);
        assert_eq!(parsed.hotkey.key, "RightCtrl");
        assert_eq!(parsed.audio, AudioConfig::default());
    }

    #[test]
    fn validate_reports_out_of_range() {
        let mut config = Config::default();
        config.audio.max_record_seconds = 0;
        config.hotkey.key = " ".into();
        let issues = config.validate();
        assert_eq!(issues.len(), 2);
    }
}
