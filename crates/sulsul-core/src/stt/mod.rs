//! 음성 인식(STT) 엔진 추상화.
//!
//! 새 엔진(로컬 모델, 클라우드 API 등)을 추가하는 방법:
//! 1. [`SttEngineFactory`]를 구현해 [`EngineDescriptor`](ID, 번역 키, 설정 스키마)를 제공
//! 2. [`SttEngine`]을 구현해 실제 인식을 수행
//! 3. 앱 시작 시 [`SttRegistry::register`]로 등록
//! 4. `locales/*.yml`의 `engine.<ID>.*` 아래에 이름·설명·설정 항목 문구 추가
//!
//! 엔진별 설정 값은 TOML의 `[stt.engines.<ID>]`에 저장됩니다.

pub mod mock;
mod registry;

use std::path::PathBuf;
use std::time::Duration;

pub use registry::SttRegistry;

use crate::audio::AudioClip;
use crate::paths::AppPaths;
use crate::settings::{ResolvedSettings, SettingField, SettingIssue};

/// Whisper 계열 모델이 기대하는 입력 샘플레이트
pub const WHISPER_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, PartialEq)]
pub struct EngineDescriptor {
    /// 설정 파일과 코드에서 쓰는 고유 ID (예: `whisper_local`)
    pub id: String,
    pub name_key: String,
    pub description_key: String,
    pub kind: EngineKind,
    pub capabilities: EngineCapabilities,
    pub settings: Vec<SettingField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    /// 이 컴퓨터에서 실행 (오프라인 가능)
    Local,
    /// 외부 서비스 호출 (네트워크 필요)
    Cloud,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineCapabilities {
    /// [`SttEngine::transcribe`]에 넘길 오디오의 샘플레이트. 호출하는 쪽에서 맞춰 줍니다.
    pub input_sample_rate: u32,
    pub languages: LanguageSupport,
    /// 용어 힌트(initial prompt)를 활용하는지
    pub supports_prompt: bool,
    /// 말하는 도중 부분 결과를 낼 수 있는지 (추후 확장)
    pub streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageSupport {
    /// 자동 감지를 포함해 대부분의 언어 지원
    Any,
    /// 지정한 언어 코드만 지원
    Only(Vec<String>),
}

impl LanguageSupport {
    pub fn supports(&self, language: &Language) -> bool {
        match (self, language) {
            (Self::Any, _) => true,
            (Self::Only(_), Language::Auto) => false,
            (Self::Only(codes), Language::Code(code)) => codes.iter().any(|c| c == code),
        }
    }
}

/// 인식 언어
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Language {
    Auto,
    /// ISO 639-1 코드 (예: `ko`, `en`)
    Code(String),
}

impl Language {
    /// 설정 문자열(`"auto"`, `"ko"` 등)을 해석합니다.
    pub fn from_config(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("auto") {
            Self::Auto
        } else {
            Self::Code(value.to_ascii_lowercase())
        }
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Auto => None,
            Self::Code(code) => Some(code),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscribeRequest {
    pub language: Language,
    /// 고유명사·전문용어 힌트
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    pub text: String,
    /// 엔진이 판단한 언어 (알 수 있을 때)
    pub language: Option<String>,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
}

/// 엔진을 만들 때 넘겨주는 앱 환경
#[derive(Debug, Clone)]
pub struct EngineContext {
    pub paths: AppPaths,
}

/// 실제 인식을 수행하는 엔진 인스턴스.
///
/// 모델 로딩처럼 비싼 준비는 생성 시점에 끝내고, 인스턴스를 재사용합니다.
/// 인식은 오래 걸릴 수 있으므로 UI 스레드가 아닌 작업 스레드에서 호출하세요.
pub trait SttEngine: Send {
    /// `audio`는 모노이며 [`EngineCapabilities::input_sample_rate`]에 맞춰져 있어야 합니다.
    fn transcribe(
        &mut self,
        audio: &AudioClip,
        request: &TranscribeRequest,
    ) -> Result<Transcript, SttError>;
}

/// 엔진의 메타데이터를 제공하고 인스턴스를 만듭니다.
pub trait SttEngineFactory: Send + Sync {
    fn descriptor(&self) -> &EngineDescriptor;

    /// `settings`는 스키마 기본값이 채워지고 검증된 값입니다.
    fn create(
        &self,
        ctx: &EngineContext,
        settings: &ResolvedSettings,
    ) -> Result<Box<dyn SttEngine>, SttError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("unknown STT engine '{0}'")]
    UnknownEngine(String),
    #[error("STT engine '{0}' is not implemented yet")]
    NotImplemented(String),
    #[error("invalid settings for STT engine '{engine}': {}", join_issues(issues))]
    InvalidSettings {
        engine: String,
        issues: Vec<SettingIssue>,
    },
    #[error("model file not found: {}", .0.display())]
    ModelNotFound(PathBuf),
    #[error("engine expects {expected} Hz audio but got {actual} Hz")]
    SampleRateMismatch { expected: u32, actual: u32 },
    #[error("language '{0}' is not supported by this engine")]
    UnsupportedLanguage(String),
    #[error("{0}")]
    Backend(String),
}

fn join_issues(issues: &[SettingIssue]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_from_config() {
        assert_eq!(Language::from_config("auto"), Language::Auto);
        assert_eq!(Language::from_config(""), Language::Auto);
        assert_eq!(Language::from_config(" KO "), Language::Code("ko".into()));
    }

    #[test]
    fn language_support() {
        let only_ko = LanguageSupport::Only(vec!["ko".into()]);
        assert!(only_ko.supports(&Language::Code("ko".into())));
        assert!(!only_ko.supports(&Language::Code("en".into())));
        assert!(!only_ko.supports(&Language::Auto));
        assert!(LanguageSupport::Any.supports(&Language::Auto));
    }
}
