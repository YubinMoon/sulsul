//! 음성 인식(STT) 엔진 추상화.
//!
//! 새 엔진(로컬 모델, 클라우드 API 등)을 추가하는 방법:
//! 1. [`SttEngineFactory`]를 구현해 [`EngineDescriptor`](ID, 번역 키, 설정 스키마)를 제공
//!    - 설정 스키마에는 공용 인식 언어 항목 [`language_field`]를 넣습니다(선택지는
//!      [`OFFERED_LANGUAGES`] 가운데 그 엔진의 [`EngineCapabilities::languages`]가 지원하는 것). 모델을 메모리에 올리는 로컬 엔진은 맨 뒤에 모델 메모리
//!      유지 시간 항목 [`keep_loaded_field`]도 넣습니다. 순서는 "모델·경로 → 언어 → 엔진 고유 항목 →
//!      메모리 유지 시간"입니다.
//!    - 더 이상 쓰지 않는 설정 키는 [`EngineDescriptor::retired_keys`]에 적어 두면 사용자 파일에
//!      남아 있어도 오류나 "모르는 키" 경고 없이 무시됩니다.
//! 2. [`SttEngine`]을 구현해 실제 인식을 수행
//! 3. 모델마다 지원 언어가 다르면(예: 영어 전용 모델) [`SttEngineFactory::languages`]를 구현
//! 4. 앱 시작 시 [`SttRegistry::register`]로 등록
//! 5. `locales/*.yml`의 `engine.<ID>.*` 아래에 이름·설명·설정 항목 문구 추가
//!    (공용 항목의 문구는 `stt.language.*`, `stt.keep_loaded.*`에 이미 있습니다)
//!
//! 엔진별 설정 값은 TOML의 `[stt.engines.<ID>]`에 저장됩니다.
//! 모델 파일을 쓰는 엔진은 [`SttEngineFactory::model`]도 구현해 필요한 파일을 알려 줍니다.
//!
//! 인식을 실행할 때는 엔진을 직접 다루지 말고 [`Transcriber`]를 쓰세요.
//! 엔진 재사용, 언어 확인, 리샘플, 쓰지 않는 모델 내려놓기를 대신 처리합니다.

mod fields;
pub mod mock;
pub mod model;
mod registry;
mod transcriber;

use std::path::PathBuf;
use std::time::Duration;

pub use fields::{
    DEFAULT_KEEP_LOADED_MINUTES, DEFAULT_LANGUAGE, KEEP_LOADED_KEY, LANGUAGE_KEY,
    MAX_KEEP_LOADED_MINUTES, OFFERED_LANGUAGES, WHISPER_LANGUAGES, keep_loaded_field,
    language_field,
};
pub use model::{
    ModelAsset, ModelAxis, ModelFile, ModelRequirement, ModelVariant, VariantChoice, axis_options,
    catalog_axes, delete_variant, find_by_choices, format_bytes,
};
pub use registry::SttRegistry;
pub use transcriber::{
    EngineTestResult, Prepared, TranscribeError, TranscribeJob, Transcriber, Transcription,
    test_engine,
};

use crate::audio::AudioClip;
use crate::cancel::CancelToken;
use crate::config::SttConfig;
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
    /// 더 이상 쓰지 않아 조용히 무시할 설정 키 (예: 예전의 `threads`).
    ///
    /// 사용자 설정 파일에 남아 있어도 검사([`SttRegistry::validate`])와 "모르는 키" 목록
    /// ([`SttRegistry::unknown_keys`])에서 빠집니다. 파일에서 지우지는 않습니다.
    pub retired_keys: Vec<String>,
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
    /// 지정한 언어 코드만 지원 (`auto` 미지원)
    Only(Vec<String>),
    /// 자동 감지(`auto`)와 함께 지정한 언어 코드를 지원
    AutoAnd(Vec<String>),
}

impl LanguageSupport {
    /// 모델이 이 언어(또는 자동 감지)로 인식할 수 있는지. 고를 수 있는지는 [`LanguageSupport::offers`]입니다.
    pub fn supports(&self, language: &Language) -> bool {
        match (self, language) {
            (Self::Any, _) => true,
            (Self::Only(_), Language::Auto) => false,
            (Self::Only(codes), Language::Code(code)) => codes.iter().any(|c| c == code),
            (Self::AutoAnd(_), Language::Auto) => true,
            (Self::AutoAnd(codes), Language::Code(code)) => codes.iter().any(|c| c == code),
        }
    }

    /// 화면과 CLI에서 고를 수 있는 값인지: 자동 감지는 모델이 지원하면, 언어는
    /// [`OFFERED_LANGUAGES`]에 있고 모델이 지원하면 고를 수 있습니다.
    pub fn offers(&self, language: &Language) -> bool {
        self.supports(language)
            && language
                .code()
                .is_none_or(|code| OFFERED_LANGUAGES.contains(&code))
    }

    /// 언어 드롭다운의 선택지. 자동 감지를 지원하면 `"auto"`가 맨 앞이고, 그다음이
    /// [`OFFERED_LANGUAGES`] 가운데 모델이 지원하는 언어입니다(그 순서대로).
    pub fn choices(&self) -> Vec<String> {
        let auto = self.supports(&Language::Auto).then_some("auto");
        let codes = OFFERED_LANGUAGES
            .iter()
            .copied()
            .filter(|&code| self.supports(&Language::Code(code.to_owned())));
        auto.into_iter().chain(codes).map(str::to_owned).collect()
    }

    /// 언어 하나만 지원하면(예: 영어 전용 모델) 그 언어 코드
    pub fn single(&self) -> Option<&str> {
        match self {
            Self::Only(codes) if codes.len() == 1 => Some(&codes[0]),
            _ => None,
        }
    }

    /// `requested`가 고를 수 있는 값([`LanguageSupport::choices`])이면 그대로, 아니면 모델이 언어
    /// 하나만 지원할 때 그 언어를 돌려줍니다. 둘 다 아니면 [`SttError::UnsupportedLanguage`]입니다.
    pub fn resolve(&self, requested: Language) -> Result<Language, SttError> {
        if self.offers(&requested) {
            return Ok(requested);
        }
        match self.single() {
            Some(code) => Ok(Language::Code(code.to_owned())),
            None => Err(SttError::UnsupportedLanguage(
                requested.code().unwrap_or("auto").to_owned(),
            )),
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

    /// 설정에 적는 형식 (`"auto"` 또는 언어 코드)
    pub fn as_config(&self) -> &str {
        self.code().unwrap_or("auto")
    }
}

#[derive(Debug, Clone)]
pub struct TranscribeRequest {
    pub language: Language,
    /// 고유명사·전문용어 힌트
    pub prompt: Option<String>,
    /// 인식을 중간에 멈추는 신호. 멈추면 [`SttError::Cancelled`]를 돌려줍니다.
    pub cancel: CancelToken,
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

/// 엔진이 계산에 쓰는 장치
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ComputeDevice {
    /// 알 수 없음 (클라우드 엔진 등)
    #[default]
    Unknown,
    Cpu,
    /// GPU 가속. 값은 화면에 그대로 보일 이름입니다 (예: `CUDA`, `Vulkan`).
    Gpu(String),
    /// NPU 가속. 값은 화면에 그대로 보일 이름입니다 (예: `DirectML (Intel(R) AI Boost)`).
    Npu(String),
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

    /// 이 인스턴스가 계산에 쓰는 장치.
    /// GPU·NPU를 쓰도록 설정했어도 쓸 수 없는 환경이면 [`ComputeDevice::Cpu`]일 수 있습니다.
    fn device(&self) -> ComputeDevice {
        ComputeDevice::Unknown
    }
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

    /// 전역 `[stt]` 설정을 엔진 실행 설정으로 해석합니다.
    ///
    /// 기본 구현은 `[stt.engines.<ID>]`만 읽으며, 프로필 기반 엔진은 오버라이드할 수 있습니다.
    /// 엔진 표에 언어가 없을 때 예전 전역 언어를 이어받는 일은 [`SttRegistry::resolve_settings`]가
    /// 이 결과 위에 해 주므로, 설정은 레지스트리를 거쳐 읽으세요.
    fn resolve_settings(&self, stt: &SttConfig) -> ResolvedSettings {
        let descriptor = self.descriptor();
        crate::settings::resolve(&descriptor.settings, stt.engines.get(&descriptor.id))
    }

    /// 이 설정(고른 모델)으로 인식할 수 있는 언어.
    ///
    /// 기본 구현은 설명자의 [`EngineCapabilities::languages`]이고, 모델마다 지원 언어가 다른
    /// 엔진(예: Whisper 영어 전용 모델)은 오버라이드합니다.
    fn languages(&self, _settings: &ResolvedSettings) -> LanguageSupport {
        self.descriptor().capabilities.languages.clone()
    }

    /// 이 설정으로 엔진을 만들 때 필요한 모델 파일.
    /// 모델 파일이 필요 없는 엔진(클라우드 등)은 `None`을 돌려줍니다.
    fn model(
        &self,
        _ctx: &EngineContext,
        _settings: &ResolvedSettings,
    ) -> Option<ModelRequirement> {
        None
    }

    /// 엔진이 제공하는 다운로드 가능 모델 크기·변형 카탈로그.
    fn model_variants(&self, _ctx: &EngineContext) -> Vec<ModelVariant> {
        Vec::new()
    }

    /// 카탈로그 모델 ID를 기록할 엔진 설정 키 (예: `"model"`).
    fn model_setting_key(&self) -> Option<&'static str> {
        None
    }
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
    #[error("transcription was cancelled")]
    Cancelled,
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

        let auto_and_ko = LanguageSupport::AutoAnd(vec!["ko".into(), "en".into()]);
        assert!(auto_and_ko.supports(&Language::Auto));
        assert!(auto_and_ko.supports(&Language::Code("ko".into())));
        assert!(!auto_and_ko.supports(&Language::Code("fr".into())));
    }

    fn codes(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|&c| c.to_owned()).collect()
    }

    /// 선택지는 자동 감지(지원하면) + 한국어·영어·중국어·일본어 가운데 모델이 지원하는 것입니다.
    #[test]
    fn language_choices() {
        let offered = ["auto", "ko", "en", "zh", "ja"];
        assert_eq!(LanguageSupport::Any.choices(), offered);
        let whisper = LanguageSupport::AutoAnd(codes(WHISPER_LANGUAGES));
        assert_eq!(whisper.choices(), offered);
        // SenseVoice처럼 광둥어(yue)를 알아도 고를 수 있는 언어에는 넣지 않습니다.
        let sense_voice = LanguageSupport::AutoAnd(codes(&["zh", "en", "ja", "ko", "yue"]));
        assert_eq!(sense_voice.choices(), offered);
        assert_eq!(LanguageSupport::Only(codes(&["en"])).choices(), ["en"]);
        // 모델이 지원하지 않는 언어는 빠지고, 순서는 모델 목록이 아니라 고를 수 있는 언어 순서입니다.
        assert_eq!(
            LanguageSupport::AutoAnd(codes(&["en", "fr", "ko"])).choices(),
            ["auto", "ko", "en"]
        );
        assert!(
            OFFERED_LANGUAGES
                .iter()
                .all(|code| WHISPER_LANGUAGES.contains(code))
        );
    }

    /// 모델이 아는 언어(`supports`)와 고를 수 있는 언어(`offers`)는 다릅니다.
    #[test]
    fn offered_languages_are_a_subset_of_supported_ones() {
        let whisper = LanguageSupport::AutoAnd(codes(WHISPER_LANGUAGES));
        let french = Language::Code("fr".into());
        assert!(whisper.supports(&french));
        assert!(!whisper.offers(&french));
        assert!(whisper.offers(&Language::Auto));
        assert!(whisper.offers(&Language::Code("ja".into())));
        assert!(!LanguageSupport::Only(codes(&["en"])).offers(&Language::Auto));
    }

    /// 모델이 아는 언어라도 고를 수 있는 언어가 아니면 받지 않습니다.
    #[test]
    fn resolve_accepts_only_offered_languages() {
        let whisper = LanguageSupport::AutoAnd(codes(WHISPER_LANGUAGES));
        assert!(matches!(
            whisper.resolve(Language::Code("fr".into())),
            Err(SttError::UnsupportedLanguage(code)) if code == "fr"
        ));
        assert_eq!(
            whisper.resolve(Language::Code("ja".into())).unwrap(),
            Language::Code("ja".into())
        );
        assert_eq!(whisper.resolve(Language::Auto).unwrap(), Language::Auto);
        let sense_voice = LanguageSupport::AutoAnd(codes(&["zh", "en", "ja", "ko", "yue"]));
        assert!(matches!(
            sense_voice.resolve(Language::Code("yue".into())),
            Err(SttError::UnsupportedLanguage(code)) if code == "yue"
        ));
    }

    #[test]
    fn single_language_model_falls_back_to_its_language() {
        let english = LanguageSupport::Only(vec!["en".into()]);
        assert_eq!(english.single(), Some("en"));
        // 영어 전용 모델은 한국어나 자동 감지를 골라 두어도 영어로 인식합니다.
        for requested in [Language::Code("ko".into()), Language::Auto] {
            assert_eq!(
                english.resolve(requested).unwrap(),
                Language::Code("en".into())
            );
        }

        let two = LanguageSupport::Only(vec!["ko".into(), "en".into()]);
        assert_eq!(two.single(), None);
        assert!(matches!(
            two.resolve(Language::Auto),
            Err(SttError::UnsupportedLanguage(code)) if code == "auto"
        ));
        assert_eq!(
            two.resolve(Language::Code("en".into())).unwrap(),
            Language::Code("en".into())
        );
        assert_eq!(LanguageSupport::Any.single(), None);
    }
}
