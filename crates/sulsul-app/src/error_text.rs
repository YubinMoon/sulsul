//! 코어의 오류·문제를 화면에 보일 문구로 바꿉니다.
//!
//! 코어 크레이트의 오류 메시지는 로그용 영어라서, 화면에는 변형마다 번역한 문구를 씁니다.

use rust_i18n::t;
use sulsul_core::AppPaths;
use sulsul_core::audio::AudioError;
use sulsul_core::config::PostProcessConfig;
use sulsul_core::config::{ConfigError, ConfigIssue, ConfigProblem};
use sulsul_core::download::DownloadError;
use sulsul_core::settings::{SettingIssue, SettingProblem};
use sulsul_core::stt::{SttError, SttRegistry};
use sulsul_llm::client::LlmError;
use sulsul_llm::refine::RefineError;
use sulsul_llm::runner::{Failure, LocalError};

use crate::autostart::AutostartError;
use crate::dictation::DictationError;
use crate::dictation::paste::ClipboardError;
use crate::hotkey::HotkeyError;
use crate::i18n::{microphone_settings_hint, tr};
use crate::pages::models::llm_issue_text;

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
        SttError::Cancelled => t!("error.stt.cancelled").into_owned(),
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

pub fn download(error: &DownloadError) -> String {
    match error {
        DownloadError::Cancelled => t!("error.download.cancelled").into_owned(),
        DownloadError::Http { status } => t!("error.download.http", status = status).into_owned(),
        DownloadError::Network(message) => {
            t!("error.download.network", message = message).into_owned()
        }
        DownloadError::Io { path, source } => t!(
            "error.download.io",
            path = path.display().to_string(),
            message = source.to_string()
        )
        .into_owned(),
        DownloadError::SizeMismatch { expected, actual } => t!(
            "error.download.size_mismatch",
            expected = expected,
            actual = actual
        )
        .into_owned(),
        DownloadError::HashMismatch { .. } => t!("error.download.hash_mismatch").into_owned(),
    }
}

/// 글 다듬기 제공자에 요청을 보내다 난 오류
pub fn llm(error: &LlmError) -> String {
    match error {
        LlmError::InvalidEndpoint(message) => {
            t!("error.llm.invalid_endpoint", message = message).into_owned()
        }
        LlmError::MissingApiKey => t!("error.llm.missing_api_key").into_owned(),
        // 제공자가 이유를 알려 주지 않았으면 상태 코드만 보여 줍니다.
        LlmError::Http { status, message } if message.trim().is_empty() => {
            t!("error.llm.http_no_message", status = status).into_owned()
        }
        LlmError::Http { status, message } => {
            t!("error.llm.http", status = status, message = message).into_owned()
        }
        LlmError::Network(message) => t!("error.llm.network", message = message).into_owned(),
        LlmError::BadResponse(message) => {
            t!("error.llm.bad_response", message = message).into_owned()
        }
        LlmError::Cancelled => t!("error.llm.cancelled").into_owned(),
    }
}

/// 글 다듬기에 실패한 이유 (`dictation.refine_failed`·`dictation.refine_failed_clipboard`의 `message` 자리)
///
/// `paths`·`config`는 고른 모델·제공자의 이름을 찾는 데 씁니다.
pub fn refine(error: &RefineError, paths: &AppPaths, config: &PostProcessConfig) -> String {
    match error {
        RefineError::NoModel => t!("models.llm.issue.no_model").into_owned(),
        RefineError::Selection(issue) => llm_issue_text(paths, config, issue),
        RefineError::NoPrompt { id } if id.is_empty() => t!("error.refine.no_prompts").into_owned(),
        RefineError::NoPrompt { id } => t!("error.refine.missing_prompt", id = id).into_owned(),
        RefineError::EmptyPrompt { name } => {
            t!("error.refine.empty_prompt", name = name).into_owned()
        }
        RefineError::Cloud(error) => llm(error),
        RefineError::Local(error) => local_llm(error),
        RefineError::EmptyOutput => t!("error.refine.empty_output").into_owned(),
    }
}

/// 로컬 모델 실행기로 다듬다 난 오류
fn local_llm(error: &LocalError) -> String {
    match error {
        LocalError::RunnerMissing(path) => {
            t!("error.refine.runner_missing", path = path.display()).into_owned()
        }
        LocalError::Spawn(message) => {
            t!("error.refine.runner_spawn", message = message).into_owned()
        }
        LocalError::Crashed(message) => {
            t!("error.refine.runner_crashed", message = message).into_owned()
        }
        LocalError::Timeout(timeout) => t!(
            "error.refine.runner_timeout",
            seconds = timeout.as_secs().max(1)
        )
        .into_owned(),
        LocalError::Failed(failure) => match failure {
            Failure::ModelMissing(path) => {
                t!("error.refine.model_missing", path = path).into_owned()
            }
            Failure::Load(message) => t!("error.refine.model_load", message = message).into_owned(),
            Failure::PromptTooLong(tokens) => {
                t!("error.refine.prompt_too_long", tokens = tokens).into_owned()
            }
            Failure::Generate(message) => {
                t!("error.refine.generate", message = message).into_owned()
            }
            Failure::BadRequest(message) => {
                t!("error.refine.bad_request", message = message).into_owned()
            }
        },
        LocalError::Cancelled => t!("error.llm.cancelled").into_owned(),
    }
}

/// 전역 단축키를 쓸 수 없는 이유
pub fn hotkey(error: &HotkeyError) -> String {
    match error {
        HotkeyError::Unsupported => t!("dictation.hotkey_unsupported").into_owned(),
        HotkeyError::Install(error) => {
            t!("dictation.hotkey_unavailable", message = error).into_owned()
        }
    }
}

/// 단축키 받아쓰기가 실패한 이유 (`dictation.failed`의 `message` 자리)
pub fn dictation(error: &DictationError, registry: &SttRegistry) -> String {
    match error {
        DictationError::Stt(error) => stt(error, registry),
        DictationError::Audio(error) => audio(error),
        DictationError::Crashed => t!("transcribe.error.crashed").into_owned(),
    }
}

/// 클립보드를 쓰지 못한 이유 (`dictation.clipboard_failed`의 `message` 자리)
pub fn clipboard(error: &ClipboardError) -> String {
    match error {
        ClipboardError::Busy => t!("dictation.clipboard_busy").into_owned(),
        ClipboardError::Os(error) => error.to_string(),
        ClipboardError::Unsupported => t!("dictation.clipboard_unsupported").into_owned(),
    }
}

/// 로그인할 때 자동 실행 설정을 바꾸지 못한 이유
pub fn autostart(error: &AutostartError) -> String {
    match error {
        AutostartError::Unsupported => t!("autostart.unsupported").into_owned(),
        AutostartError::Os(error) => t!("autostart.failed", message = error).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_errors_have_their_own_wording() {
        rust_i18n::set_locale("ko");
        let errors = [
            LlmError::InvalidEndpoint("empty".into()),
            LlmError::MissingApiKey,
            LlmError::Http {
                status: 401,
                message: "invalid key".into(),
            },
            LlmError::Http {
                status: 500,
                message: String::new(),
            },
            LlmError::Network("timed out".into()),
            LlmError::BadResponse("no choices".into()),
            LlmError::Cancelled,
        ];
        let texts: Vec<String> = errors.iter().map(llm).collect();
        for (error, text) in errors.iter().zip(&texts) {
            assert!(
                !text.starts_with("error."),
                "{error:?}: 번역이 없어 키가 그대로 나왔습니다 ({text})"
            );
        }
        // 오류마다 문구가 달라야 무엇이 문제인지 알 수 있습니다.
        let mut unique = texts.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), texts.len(), "{texts:#?}");

        assert!(texts[2].contains("401") && texts[2].contains("invalid key"));
        assert!(
            texts[3].contains("500") && !texts[3].contains("%{"),
            "이유가 없으면 상태 코드만 보여 줍니다: {}",
            texts[3]
        );
    }

    #[test]
    fn refine_errors_have_their_own_wording() {
        use std::path::PathBuf;
        use std::time::Duration;

        use sulsul_llm::entries::SelectionIssue;

        rust_i18n::set_locale("ko");
        let paths = AppPaths::under("unused");
        let config = PostProcessConfig::default();
        let errors = [
            RefineError::NoModel,
            RefineError::Selection(SelectionIssue::NoProvider),
            RefineError::NoPrompt { id: String::new() },
            RefineError::NoPrompt { id: "gone".into() },
            RefineError::EmptyPrompt {
                name: "빈 지시문".into(),
            },
            RefineError::Cloud(LlmError::MissingApiKey),
            RefineError::Local(LocalError::RunnerMissing(PathBuf::from("runner.exe"))),
            RefineError::Local(LocalError::Spawn("denied".into())),
            RefineError::Local(LocalError::Crashed("exit code: 1".into())),
            RefineError::Local(LocalError::Timeout(Duration::from_secs(240))),
            RefineError::Local(LocalError::Failed(Failure::ModelMissing("a.gguf".into()))),
            RefineError::Local(LocalError::Failed(Failure::Load("bad".into()))),
            RefineError::Local(LocalError::Failed(Failure::PromptTooLong(9000))),
            RefineError::Local(LocalError::Failed(Failure::Generate("decode".into()))),
            RefineError::Local(LocalError::Failed(Failure::BadRequest("json".into()))),
            RefineError::EmptyOutput,
        ];
        let texts: Vec<String> = errors
            .iter()
            .map(|error| refine(error, &paths, &config))
            .collect();
        for (error, text) in errors.iter().zip(&texts) {
            assert!(
                !text.starts_with("error.") && !text.starts_with("models.") && !text.contains("%{"),
                "{error:?}: 번역이 빠졌거나 자리 표시가 남았습니다 ({text})"
            );
        }
        let mut unique = texts.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), texts.len(), "{texts:#?}");
        assert!(texts[9].contains("240"), "{}", texts[9]);
    }

    #[test]
    fn refine_failure_clipboard_text_cannot_pass_for_a_result() {
        rust_i18n::set_locale("ko");
        let text = t!("dictation.refine_failed_clipboard", message = "원인 설명");
        assert!(
            text.starts_with("[술술 오류]"),
            "다듬은 결과로 착각하지 않게 첫머리에 오류임을 밝혀야 합니다: {text}"
        );
        assert!(text.contains("원인 설명"), "{text}");
    }
}
