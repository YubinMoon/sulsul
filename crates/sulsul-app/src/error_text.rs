//! 코어의 오류·문제를 화면에 보일 문구로 바꿉니다.
//!
//! 코어 크레이트의 오류 메시지는 로그용 영어라서, 화면에는 변형마다 번역한 문구를 씁니다.
//! 화면 문구는 무엇이 안 됐는지와 할 일만 짧게 알리고, 오류 원문(경로, OS·서버 메시지 등)은
//! 부르는 쪽이 로그(`tracing`)에 남깁니다. 사용자에게 같은 말인 변형은 같은 문구로 묶습니다.

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
use crate::hotkey::HotkeyError;
use crate::i18n::{microphone_settings_hint, tr};
use crate::pages::models::llm_issue_text;

pub fn audio(error: &AudioError) -> String {
    match error {
        AudioError::NoInputDevice => t!("error.audio.no_input_device").into_owned(),
        AudioError::NoOutputDevice => t!("error.audio.no_output_device").into_owned(),
        AudioError::DeviceNotFound(device) => {
            t!("error.audio.device_not_found", device = device).into_owned()
        }
        AudioError::PermissionDenied => t!(
            "error.audio.permission_denied",
            hint = microphone_settings_hint()
        )
        .into_owned(),
        AudioError::DeviceBusy => t!("error.audio.device_busy").into_owned(),
        AudioError::UnsupportedFormat(_) => t!("error.audio.unsupported_format").into_owned(),
        AudioError::Device(_) => t!("error.audio.device").into_owned(),
        AudioError::Stream(_) => t!("error.audio.stream").into_owned(),
        AudioError::Resample(_) => t!("error.audio.resample").into_owned(),
        AudioError::Wav { .. } => t!("error.audio.wav").into_owned(),
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
        SttError::ModelNotFound(_) => t!("error.model_missing").into_owned(),
        SttError::SampleRateMismatch { .. } => t!("error.stt.sample_rate_mismatch").into_owned(),
        SttError::UnsupportedLanguage(language) => {
            t!("error.stt.unsupported_language", language = language).into_owned()
        }
        SttError::Cancelled => t!("error.stt.cancelled").into_owned(),
        SttError::Backend(_) => t!("error.stt.backend").into_owned(),
    }
}

/// 엔진 이름 (등록되지 않은 엔진이면 ID 그대로)
pub fn engine_name(registry: &SttRegistry, id: &str) -> String {
    registry
        .descriptor(id)
        .map_or_else(|| id.to_owned(), |descriptor| tr(&descriptor.name_key))
}

/// 설정 파일 문제. 문법·값 오류는 고칠 곳(줄·칸)만 알리고 파서의 설명은 로그에 남깁니다.
pub fn config(error: &ConfigError) -> String {
    match error {
        ConfigError::Read { .. } => t!("error.config.read").into_owned(),
        ConfigError::Write { .. } => t!("error.config.write").into_owned(),
        ConfigError::Parse { line, column, .. } => match (line, column) {
            (Some(line), Some(column)) => {
                t!("error.config.parse_at", line = line, column = column).into_owned()
            }
            (Some(line), None) => t!("error.config.parse_at_line", line = line).into_owned(),
            _ => t!("error.config.parse").into_owned(),
        },
        ConfigError::Serialize(_) => t!("error.config.serialize").into_owned(),
        ConfigError::Watch { .. } => t!("error.config.watch").into_owned(),
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
        DownloadError::Network(_) => t!("error.download.network").into_owned(),
        DownloadError::Io { .. } => t!("error.download.io").into_owned(),
        // 크기가 다르든 해시가 다르든 사용자에게는 받은 파일이 망가졌다는 같은 뜻입니다.
        DownloadError::SizeMismatch { .. } | DownloadError::HashMismatch { .. } => {
            t!("error.download.corrupt").into_owned()
        }
    }
}

/// 글 다듬기 제공자에 요청을 보내다 난 오류.
///
/// 제공자가 돌려준 설명은 길고 영어인 경우가 많아, 상태 코드로 할 일을 알리고 설명은 로그에 남깁니다.
pub fn llm(error: &LlmError) -> String {
    match error {
        LlmError::InvalidEndpoint(_) => t!("error.llm.invalid_endpoint").into_owned(),
        LlmError::MissingApiKey => t!("error.llm.missing_api_key").into_owned(),
        LlmError::Http { status, .. } => match status {
            401 | 403 => t!("error.llm.auth").into_owned(),
            404 => t!("error.llm.not_found").into_owned(),
            429 => t!("error.llm.rate_limited").into_owned(),
            _ => t!("error.llm.http", status = status).into_owned(),
        },
        LlmError::ReasoningRejected { .. } => t!("error.llm.reasoning_rejected").into_owned(),
        LlmError::Network(_) => t!("error.llm.network").into_owned(),
        LlmError::BadResponse(_) => t!("error.llm.bad_response").into_owned(),
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
        RefineError::NoPrompt { .. } => t!("error.refine.missing_prompt").into_owned(),
        RefineError::EmptyPrompt { name } => {
            t!("error.refine.empty_prompt", name = name).into_owned()
        }
        RefineError::Cloud(error) => llm(error),
        RefineError::Local(error) => local_llm(error),
        RefineError::EmptyOutput => t!("error.refine.empty_output").into_owned(),
    }
}

/// 로컬 모델 실행기로 다듬다 난 오류.
///
/// 실행기를 띄우지 못했거나, 멈췄거나, 글을 만들다 오류가 난 경우는 사용자에게 같은 뜻이라 한 문구로 묶습니다.
fn local_llm(error: &LocalError) -> String {
    match error {
        LocalError::RunnerMissing(_) => t!("error.refine.runner_missing").into_owned(),
        LocalError::Spawn(_)
        | LocalError::Crashed(_)
        | LocalError::Failed(Failure::Generate(_) | Failure::BadRequest(_)) => {
            t!("error.refine.runner_failed").into_owned()
        }
        LocalError::Timeout(timeout) => t!(
            "error.refine.runner_timeout",
            seconds = timeout.as_secs().max(1)
        )
        .into_owned(),
        LocalError::Failed(Failure::ModelMissing(_)) => t!("error.model_missing").into_owned(),
        LocalError::Failed(Failure::Load(_)) => t!("error.refine.model_load").into_owned(),
        LocalError::Failed(Failure::PromptTooLong(_)) => {
            t!("error.refine.prompt_too_long").into_owned()
        }
        LocalError::Cancelled => t!("error.llm.cancelled").into_owned(),
    }
}

/// 전역 단축키를 쓸 수 없는 이유
pub fn hotkey(error: &HotkeyError) -> String {
    match error {
        HotkeyError::Unsupported => t!("dictation.hotkey_unsupported").into_owned(),
        HotkeyError::Install(_) => t!("dictation.hotkey_unavailable").into_owned(),
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

/// 로그인할 때 자동 실행 설정을 바꾸지 못한 이유
pub fn autostart(error: &AutostartError) -> String {
    match error {
        AutostartError::Unsupported => t!("autostart.unsupported").into_owned(),
        AutostartError::Os(_) => t!("autostart.failed").into_owned(),
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
                status: 404,
                message: String::new(),
            },
            LlmError::Http {
                status: 429,
                message: "slow down".into(),
            },
            LlmError::Http {
                status: 500,
                message: String::new(),
            },
            LlmError::ReasoningRejected {
                status: 400,
                message: "unsupported value".into(),
            },
            LlmError::Network("timed out".into()),
            LlmError::BadResponse("no choices".into()),
            LlmError::Cancelled,
        ];
        let texts: Vec<String> = errors.iter().map(llm).collect();
        for (error, text) in errors.iter().zip(&texts) {
            assert!(
                !text.starts_with("error.") && !text.contains("%{"),
                "{error:?}: 번역이 빠졌거나 자리 표시가 남았습니다 ({text})"
            );
        }
        // 오류마다 문구가 달라야 무엇이 문제인지 알 수 있습니다.
        let mut unique = texts.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), texts.len(), "{texts:#?}");

        // 제공자의 설명(대개 영어)은 화면에 옮기지 않고, 상태 코드로 할 일을 알립니다.
        assert!(
            texts[2].contains("API 키") && !texts[2].contains("invalid key"),
            "{}",
            texts[2]
        );
        assert!(!texts[4].contains("slow down"), "{}", texts[4]);
        assert!(
            texts[5].contains("500"),
            "따로 풀이하지 않는 상태 코드는 그대로 보여 줍니다: {}",
            texts[5]
        );
        let forbidden = LlmError::Http {
            status: 403,
            message: String::new(),
        };
        assert_eq!(llm(&forbidden), texts[2], "403도 401과 같은 뜻입니다");
    }

    #[test]
    fn refine_errors_are_short_and_grouped() {
        use std::path::PathBuf;
        use std::time::Duration;

        use sulsul_llm::entries::SelectionIssue;

        rust_i18n::set_locale("ko");
        let paths = AppPaths::under("unused");
        let config = PostProcessConfig::default();
        let text = |error: &RefineError| refine(error, &paths, &config);

        // 사용자에게 뜻이 다른 오류는 문구도 다릅니다.
        let distinct = [
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
            RefineError::Local(LocalError::Timeout(Duration::from_secs(240))),
            RefineError::Local(LocalError::Failed(Failure::ModelMissing("a.gguf".into()))),
            RefineError::Local(LocalError::Failed(Failure::Load("bad".into()))),
            RefineError::Local(LocalError::Failed(Failure::PromptTooLong(9000))),
            RefineError::EmptyOutput,
        ];
        let texts: Vec<String> = distinct.iter().map(text).collect();
        for (error, text) in distinct.iter().zip(&texts) {
            assert!(
                !text.starts_with("error.") && !text.starts_with("models.") && !text.contains("%{"),
                "{error:?}: 번역이 빠졌거나 자리 표시가 남았습니다 ({text})"
            );
        }
        let mut unique = texts.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), texts.len(), "{texts:#?}");
        assert!(texts[8].contains("240"), "{}", texts[8]);

        // 실행기를 띄우지 못했거나, 멈췄거나, 글을 만들다 난 오류는 같은 문구로 알립니다.
        let runner_failed = &texts[7];
        for error in [
            RefineError::Local(LocalError::Crashed("exit code: 1".into())),
            RefineError::Local(LocalError::Failed(Failure::Generate("decode".into()))),
            RefineError::Local(LocalError::Failed(Failure::BadRequest("json".into()))),
        ] {
            assert_eq!(&text(&error), runner_failed, "{error:?}");
        }

        // 경로나 내부 오류 원문은 화면 문구에 옮기지 않습니다.
        for raw in ["runner.exe", "denied", "a.gguf", "bad", "9000"] {
            assert!(
                texts.iter().all(|text| !text.contains(raw)),
                "'{raw}'이(가) 화면 문구에 들어갔습니다: {texts:#?}"
            );
        }
    }

    #[test]
    fn internal_details_stay_in_the_log() {
        use std::path::PathBuf;

        rust_i18n::set_locale("ko");
        let audio_text = audio(&AudioError::Stream("WASAPI 0x88890004".into()));
        assert!(!audio_text.contains("0x88890004"), "{audio_text}");

        let stt_text = stt(
            &SttError::Backend("ggml_init failed".into()),
            &SttRegistry::new(),
        );
        assert!(!stt_text.contains("ggml"), "{stt_text}");

        let network = download(&DownloadError::Network("dns error".into()));
        assert!(!network.contains("dns"), "{network}");
        let size = download(&DownloadError::SizeMismatch {
            expected: 1,
            actual: 2,
        });
        let hash = download(&DownloadError::HashMismatch {
            expected: "a".into(),
            actual: "b".into(),
        });
        assert_eq!(size, hash, "크기·해시가 다른 것은 같은 뜻입니다");

        // 설정 파일의 문법 오류는 고칠 곳만 알리고 파서의 설명은 옮기지 않습니다.
        let parse = config(&ConfigError::Parse {
            path: PathBuf::from("config.toml"),
            message: "expected `=`".into(),
            line: Some(3),
            column: Some(5),
        });
        assert!(parse.contains('3') && parse.contains('5'), "{parse}");
        assert!(!parse.contains("expected"), "{parse}");
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
