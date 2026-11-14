//! 코어의 오류·문제를 화면에 보일 문구로 바꿉니다.
//!
//! 코어 크레이트의 오류 메시지는 로그용 영어라서, 화면에는 변형마다 번역한 문구를 씁니다.

use rust_i18n::t;
use sulsul_core::audio::AudioError;
use sulsul_core::config::{ConfigError, ConfigIssue, ConfigProblem};
use sulsul_core::settings::{SettingIssue, SettingProblem};
use sulsul_core::stt::{SttError, SttRegistry};

use crate::i18n::{microphone_settings_hint, tr};

pub fn audio(error: &AudioError) -> String {
    match error {
        AudioError::NoInputDevice => t!("error.audio.no_input_device").into_owned(),
        AudioError::DeviceNotFound(device) => {
            t!("error.audio.device_not_found", device = device).into_owned()
        }
        AudioError::PermissionDenied => t!(
            "error.audio.permission_denied",
            hint = microphone_settings_hint()
        )
        .into_owned(),
        AudioError::DeviceBusy => t!("error.audio.device_busy").into_owned(),
        AudioError::UnsupportedFormat(format) => {
            t!("error.audio.unsupported_format", format = format).into_owned()
        }
        AudioError::Device(message) => t!("error.audio.device", message = message).into_owned(),
        AudioError::Stream(message) => t!("error.audio.stream", message = message).into_owned(),
        AudioError::Resample(message) => t!("error.audio.resample", message = message).into_owned(),
        AudioError::Wav { path, message } => {
            t!("error.audio.wav", path = path.display(), message = message).into_owned()
        }
    }
}

pub fn stt(error: &SttError, registry: &SttRegistry) -> String {
    match error {
        SttError::UnknownEngine(id) => t!("error.stt.unknown_engine", engine = id).into_owned(),
        SttError::NotImplemented(id) => t!(
            "error.stt.not_implemented",
            engine = engine_name(registry, id)
        )
        .into_owned(),
        SttError::InvalidSettings { engine, issues } => t!(
            "error.stt.invalid_settings",
            engine = engine_name(registry, engine),
            issues = issues
                .iter()
                .map(setting_issue)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into_owned(),
        SttError::ModelNotFound(path) => {
            t!("error.stt.model_not_found", path = path.display()).into_owned()
        }
        SttError::SampleRateMismatch { expected, actual } => t!(
            "error.stt.sample_rate_mismatch",
            expected = expected,
            actual = actual
        )
        .into_owned(),
        SttError::UnsupportedLanguage(language) => {
            t!("error.stt.unsupported_language", language = language).into_owned()
        }
        SttError::Backend(message) => t!("error.stt.backend", message = message).into_owned(),
    }
}

/// 엔진 이름 (등록되지 않은 엔진이면 ID 그대로)
pub fn engine_name(registry: &SttRegistry, id: &str) -> String {
    registry
        .descriptor(id)
        .map_or_else(|| id.to_owned(), |descriptor| tr(&descriptor.name_key))
}

pub fn config(error: &ConfigError) -> String {
    match error {
        ConfigError::Read { path, source } => {
            t!("error.config.read", path = path.display(), message = source).into_owned()
        }
        ConfigError::Write { path, source } => t!(
            "error.config.write",
            path = path.display(),
            message = source
        )
        .into_owned(),
        ConfigError::Parse {
            message,
            line,
            column,
            ..
        } => match (line, column) {
            (Some(line), Some(column)) => t!(
                "error.config.parse_at",
                line = line,
                column = column,
                message = message
            )
            .into_owned(),
            (Some(line), None) => {
                t!("error.config.parse_at_line", line = line, message = message).into_owned()
            }
            _ => t!("error.config.parse", message = message).into_owned(),
        },
        ConfigError::Serialize(message) => {
            t!("error.config.serialize", message = message).into_owned()
        }
        ConfigError::Watch { message, .. } => {
            t!("error.config.watch", message = message).into_owned()
        }
    }
}

pub fn config_issue(issue: &ConfigIssue) -> String {
    match &issue.problem {
        ConfigProblem::UnknownKey => t!("issue.unknown_key", key = issue.path).into_owned(),
        ConfigProblem::OutOfRange { min, max } => {
            t!("issue.out_of_range", key = issue.path, min = min, max = max).into_owned()
        }
        ConfigProblem::Empty => t!("issue.empty", key = issue.path).into_owned(),
    }
}

pub fn setting_issue(issue: &SettingIssue) -> String {
    match &issue.problem {
        SettingProblem::WrongType { expected } => t!(
            "issue.wrong_type",
            key = issue.key,
            expected = tr(&format!("issue.type.{expected}"))
        )
        .into_owned(),
        SettingProblem::OutOfRange { min, max } => {
            t!("issue.out_of_range", key = issue.key, min = min, max = max).into_owned()
        }
        SettingProblem::UnknownChoice(value) => {
            t!("issue.unknown_choice", key = issue.key, value = value).into_owned()
        }
        SettingProblem::UnknownKey => t!("issue.unknown_key", key = issue.key).into_owned(),
    }
}
