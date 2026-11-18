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

/// 예전 버전에 있다가 없앤 설정 키 (점으로 구분한 경로).
///
/// 예전 파일에 이 키가 남아 있으면 읽을 때 [`ConfigProblem::UnknownKey`]로 알리지 않고
/// [`LoadedConfig::removed_keys`]에 담으며, 다음 저장 때 붙은 주석과 함께 파일에서 지웁니다.
/// 표 이름을 적으면 그 표 전체가 해당합니다. 설정 키를 없앨 때는 여기에 더합니다.
pub const REMOVED_KEYS: &[&str] = &[
    // 로그인 자동 실행으로 켜질 때만 트레이로 시작하도록 바뀌어 없앴습니다.
    "general.start_minimized",
    // 출력은 "클립보드에 복사하고 붙여넣은 뒤 되돌리기"로 고정되었습니다.
    "output",
];

/// `path`(점으로 구분한 키 경로)가 [`REMOVED_KEYS`]의 키이거나 그 아래 키인지
pub fn is_removed_key(path: &str) -> bool {
    REMOVED_KEYS.iter().any(|removed| {
        path.strip_prefix(removed)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub config_version: u32,
    pub general: GeneralConfig,
    pub hotkey: HotkeyConfig,
    pub audio: AudioConfig,
    pub stt: SttConfig,
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
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            ui_language: "ko".into(),
            theme: Theme::Dark,
            launch_at_startup: false,
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
    /// 녹음이 끝나면 작은 말소리를 알맞은 크기로 키움
    pub auto_gain: bool,
    /// 녹음 앞뒤의 말소리가 없는 부분을 잘라 내고, 말소리가 없으면 받아쓰지 않음 (`sulsul-vad`)
    pub trim_silence: bool,
    pub max_record_seconds: u32,
    pub feedback_sounds: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: String::new(),
            auto_gain: true,
            trim_silence: true,
            max_record_seconds: 120,
            feedback_sounds: true,
        }
    }
}

/// 저장된 원격 API 프로필. STT(`[[stt.api_profiles]]`)와 글 다듬기(`[[postprocess.api_profiles]]`)가 함께 씁니다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ApiProfile {
    /// 프로필 고유 ID
    pub id: String,
    /// 화면에 표시할 이름
    pub name: String,
    /// API 공급자 프리셋 ID (예: `openai`, `groq`, `custom`). 쓸 수 있는 값은 쓰는 쪽(STT·글 다듬기)마다 다릅니다.
    pub provider: String,
    /// 기본 URL 또는 전체 엔드포인트 주소 (STT는 `/audio/transcriptions`, 글 다듬기는 `/chat/completions`)
    pub endpoint: String,
    /// 인증 키
    pub api_key: String,
    /// 사용할 모델 이름 (예: STT `whisper-1`, 글 다듬기 `gpt-6-luna`)
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// 사용할 엔진 ID ([`crate::stt::SttRegistry`]에 등록된 것)
    pub engine: String,
    /// 예전 전역 인식 언어. 엔진 설정(`[stt.engines.<ID>] language`)에 언어가 없을 때만 씁니다.
    ///
    /// 지금은 인식 언어를 엔진마다 저장하므로 새 설정 파일에는 없습니다. 예전 파일의 값을
    /// 이어받으려고 남겨 두었고, 비어 있으면 저장하지 않습니다.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
    /// 용어 힌트
    pub initial_prompt: String,
    /// 선택된 클라우드 API 프로필 ID (`cloud_api` 엔진용)
    pub api_profile: String,
    /// 저장된 클라우드 API 프로필 목록
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub api_profiles: Vec<ApiProfile>,
    /// 엔진별 설정 (`[stt.engines.<ID>]`). 값 검증은 각 엔진의 스키마가 담당합니다.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub engines: BTreeMap<String, toml::Table>,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: "whisper_local".into(),
            language: String::new(),
            initial_prompt: String::new(),
            api_profile: String::new(),
            api_profiles: Vec::new(),
            engines: BTreeMap::new(),
        }
    }
}

impl SttConfig {
    /// 현재 활성화된 클라우드 API 프로필을 돌려줍니다.
    ///
    /// `api_profile` ID가 일치하는 항목을 우선 찾고, ID가 비어 있으면 첫 번째 프로필을 씁니다.
    pub fn active_api_profile(&self) -> Option<&ApiProfile> {
        let selected = self.api_profile.trim();
        if selected.is_empty() {
            return self.api_profiles.first();
        }
        self.api_profiles.iter().find(|p| p.id == selected)
    }
}

/// 글 다듬기 프롬프트 (`[[postprocess.prompts]]`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Prompt {
    /// 프롬프트 고유 ID
    pub id: String,
    /// 화면에 보일 이름
    pub name: String,
    /// 언어 모델에 보낼 지시문. 받아쓴 글은 따로 붙여 보냅니다.
    pub text: String,
}

/// 처음부터 넣어 두는 프롬프트의 ID
pub const DEFAULT_PROMPT_ID: &str = "default";

/// 처음부터 넣어 두는 프롬프트 목록 (`Config::default()`와 `default.toml`에 같은 내용)
pub fn default_prompts() -> Vec<Prompt> {
    vec![Prompt {
        id: DEFAULT_PROMPT_ID.into(),
        name: "기본 다듬기".into(),
        text: "음성 인식으로 받아쓴 글을 다듬어 주세요.\n\
               - 맞춤법, 띄어쓰기, 문장 부호만 바로잡습니다.\n\
               - 뜻과 말투는 바꾸지 않고, 내용을 더하거나 빼지 않습니다.\n\
               - 설명 없이 다듬은 글만 돌려줍니다."
            .into(),
    }]
}

/// 글 다듬기(LLM 후처리) 설정 (`[postprocess]`)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PostProcessConfig {
    pub enabled: bool,
    /// 사용할 후처리기 ID: `""`(안 고름), `"llm_local"`, `"llm_cloud"`
    pub processor: String,
    /// `llm_cloud`일 때 고른 프로필 ID. 비어 있으면 첫 프로필을 씁니다
    /// ([`SttConfig::active_api_profile`]과 같은 규칙).
    pub api_profile: String,
    /// 지금 쓰는 프롬프트 ID. 비어 있으면 첫 프롬프트를 씁니다.
    pub prompt: String,
    /// 저장된 외부 제공자 프로필 목록 (`[[postprocess.api_profiles]]`)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub api_profiles: Vec<ApiProfile>,
    /// 후처리기별 설정 (`[postprocess.processors.<ID>]`). 로컬 모델 선택은
    /// `[postprocess.processors.llm_local] model = "<변형 ID>"`에 둡니다.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub processors: BTreeMap<String, toml::Table>,
    /// 저장된 프롬프트 목록 (`[[postprocess.prompts]]`)
    ///
    /// 파일에 이 항목이 아예 없으면 [`default_prompts`]로 읽습니다. 사용자가 모두 지운 상태를
    /// 지키려고 비어 있어도 저장합니다(`prompts = []`).
    pub prompts: Vec<Prompt>,
}

impl Default for PostProcessConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            processor: String::new(),
            api_profile: String::new(),
            prompt: DEFAULT_PROMPT_ID.into(),
            api_profiles: Vec::new(),
            processors: BTreeMap::new(),
            prompts: default_prompts(),
        }
    }
}

impl PostProcessConfig {
    /// 지금 고른 외부 제공자 프로필을 돌려줍니다.
    ///
    /// `api_profile` ID가 일치하는 항목을 찾고, ID가 비어 있으면 첫 번째 프로필을 씁니다.
    pub fn active_api_profile(&self) -> Option<&ApiProfile> {
        let selected = self.api_profile.trim();
        if selected.is_empty() {
            return self.api_profiles.first();
        }
        self.api_profiles.iter().find(|p| p.id == selected)
    }

    /// 지금 쓰는 프롬프트. `prompt` ID와 맞는 것, ID가 비어 있으면 첫 프롬프트, 없는 ID면 `None`
    /// ([`Self::active_api_profile`]과 같은 규칙)
    pub fn active_prompt(&self) -> Option<&Prompt> {
        let selected = self.prompt.trim();
        if selected.is_empty() {
            return self.prompts.first();
        }
        self.prompts.iter().find(|p| p.id == selected)
    }

    /// 새 프롬프트에 쓸, 목록에 아직 없는 ID (`prompt-1`, `prompt-2`, …)
    pub fn new_prompt_id(&self) -> String {
        let mut n = 1_usize;
        loop {
            let id = format!("prompt-{n}");
            if !self.prompts.iter().any(|p| p.id == id) {
                return id;
            }
            n += 1;
        }
    }
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

    #[test]
    fn postprocess_active_api_profile_follows_stt_rule() {
        let profile = |id: &str| ApiProfile {
            id: id.into(),
            ..ApiProfile::default()
        };
        let mut post = PostProcessConfig::default();
        assert_eq!(
            post.active_api_profile(),
            None,
            "프로필이 없으면 None이어야 합니다"
        );

        post.api_profiles = vec![profile("a"), profile("b")];
        assert_eq!(
            post.active_api_profile().map(|p| p.id.as_str()),
            Some("a"),
            "ID가 비어 있으면 첫 프로필이어야 합니다"
        );
        post.api_profile = " b ".into();
        assert_eq!(post.active_api_profile().map(|p| p.id.as_str()), Some("b"));
        post.api_profile = "missing".into();
        assert_eq!(
            post.active_api_profile(),
            None,
            "없는 ID를 고르면 None이어야 합니다"
        );
    }

    #[test]
    fn removed_key_matches_key_and_children_only() {
        assert!(is_removed_key("general.start_minimized"));
        assert!(is_removed_key("output"));
        assert!(is_removed_key("output.restore_delay_ms"));
        assert!(
            !is_removed_key("outputs"),
            "이름이 비슷한 다른 키는 아닙니다"
        );
        assert!(!is_removed_key("general"));
        assert!(!is_removed_key("general.start_minimized_x"));
    }

    #[test]
    fn default_prompt_is_selected() {
        let post = PostProcessConfig::default();
        assert_eq!(post.prompts, default_prompts());
        assert_eq!(post.prompt, DEFAULT_PROMPT_ID);
        assert_eq!(
            post.active_prompt().map(|p| p.id.as_str()),
            Some(DEFAULT_PROMPT_ID)
        );
        assert!(
            post.active_prompt().unwrap().text.contains('\n'),
            "기본 프롬프트는 여러 줄이어야 합니다"
        );
    }

    #[test]
    fn active_prompt_follows_api_profile_rule() {
        let prompt = |id: &str| Prompt {
            id: id.into(),
            ..Prompt::default()
        };
        let mut post = PostProcessConfig {
            prompt: String::new(),
            prompts: Vec::new(),
            ..PostProcessConfig::default()
        };
        assert_eq!(
            post.active_prompt(),
            None,
            "프롬프트가 없으면 None이어야 합니다"
        );

        post.prompts = vec![prompt("a"), prompt("b")];
        assert_eq!(
            post.active_prompt().map(|p| p.id.as_str()),
            Some("a"),
            "ID가 비어 있으면 첫 프롬프트여야 합니다"
        );
        post.prompt = " b ".into();
        assert_eq!(post.active_prompt().map(|p| p.id.as_str()), Some("b"));
        post.prompt = "missing".into();
        assert_eq!(
            post.active_prompt(),
            None,
            "없는 ID를 고르면 None이어야 합니다"
        );
    }

    #[test]
    fn new_prompt_id_skips_used_ids() {
        let mut post = PostProcessConfig::default();
        assert_eq!(post.new_prompt_id(), "prompt-1");
        for id in ["prompt-1", "prompt-3"] {
            post.prompts.push(Prompt {
                id: id.into(),
                ..Prompt::default()
            });
        }
        assert_eq!(post.new_prompt_id(), "prompt-2");
        post.prompts.push(Prompt {
            id: "prompt-2".into(),
            ..Prompt::default()
        });
        assert_eq!(post.new_prompt_id(), "prompt-4");
    }

    /// 프롬프트 항목이 없는 예전 파일은 기본 프롬프트로 읽고, 비워 둔 목록은 비운 채로 읽어야 합니다.
    #[test]
    fn missing_prompts_use_defaults_but_empty_list_stays_empty() {
        let parsed: Config = toml::from_str("[postprocess]\nenabled = true\n").unwrap();
        assert_eq!(parsed.postprocess.prompts, default_prompts());
        assert_eq!(parsed.postprocess.prompt, DEFAULT_PROMPT_ID);

        let parsed: Config =
            toml::from_str("[postprocess]\nprompt = \"\"\nprompts = []\n").unwrap();
        assert!(
            parsed.postprocess.prompts.is_empty(),
            "모두 지운 목록에 기본 프롬프트가 되살아나면 안 됩니다"
        );
        assert_eq!(parsed.postprocess.active_prompt(), None);
    }

    #[test]
    fn postprocess_profiles_and_processor_tables_parse() {
        let parsed: Config = toml::from_str(
            "[postprocess]\nprocessor = \"llm_cloud\"\napi_profile = \"p1\"\n\
             [[postprocess.api_profiles]]\nid = \"p1\"\nprovider = \"openai\"\nmodel = \"m\"\n\
             [postprocess.processors.llm_local]\nmodel = \"v1\"\n",
        )
        .unwrap();
        let post = &parsed.postprocess;
        assert_eq!(post.api_profiles.len(), 1);
        assert_eq!(post.active_api_profile().unwrap().model, "m");
        assert_eq!(
            post.processors["llm_local"].get("model"),
            Some(&toml::Value::String("v1".into()))
        );
    }
}
