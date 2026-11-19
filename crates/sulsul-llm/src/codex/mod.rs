//! Codex 로그인: ChatGPT 계정으로 로그인해 Codex가 쓰는 모델로 글을 다듬습니다.
//!
//! Codex CLI와 같은 방법을 씁니다. 브라우저에서 ChatGPT 계정으로 로그인하면(OAuth 인가 코드 + PKCE)
//! 받은 토큰으로 ChatGPT의 Codex 백엔드(`https://chatgpt.com/backend-api/codex`)에 Responses 형식으로
//! 요청합니다. 사용량은 API 요금이 아니라 그 계정의 ChatGPT 요금제에서 차감됩니다.
//!
//! - [`auth`]: 로그인 정보(토큰) 저장, 계정 정보, 만료되기 전에 다시 받기
//! - [`login`]: 브라우저 로그인. 로그인을 마친 브라우저가 돌아오는 주소를 이 PC에서 받습니다.
//! - [`client`]: 다듬기 요청(`/responses`, 스트림)과 모델 목록(`/models`)
//!
//! 로그인 정보는 설정 파일이 아니라 데이터 폴더의 `codex/auth.json`에 둡니다. 토큰은 다시 받을 때마다
//! 바뀌고, 설정 파일은 사용자가 직접 고치고 옮기는 파일이기 때문입니다. Codex CLI의 로그인 파일
//! (`~/.codex/auth.json`)은 읽지도 쓰지도 않습니다. refresh token은 한 번 쓰면 새것으로 바뀌어, 두 프로그램이
//! 같은 로그인을 나눠 쓰면 한쪽이 로그아웃되기 때문입니다.
//!
//! 고른 모델과 추론 단계는 설정의 `[postprocess.processors.llm_codex]`에 `model`·`reasoning_effort`로 둡니다.

pub mod auth;
pub mod client;
pub mod login;

use std::time::Duration;

use sulsul_core::config::{PostProcessConfig, ReasoningEffort};
use sulsul_core::toml::Value;

use crate::CODEX_PROCESSOR_ID;

/// 요청에 넣는 `User-Agent`
const USER_AGENT: &str = concat!("sulsul/", env!("CARGO_PKG_VERSION"));

/// `[postprocess.processors.llm_codex]`에서 고른 모델 이름을 담는 키
pub const MODEL_KEY: &str = "model";

/// `[postprocess.processors.llm_codex]`에서 고른 추론 단계를 담는 키
pub const REASONING_EFFORT_KEY: &str = "reasoning_effort";

/// 모델을 고르지 않았을 때 쓰는 모델 (OpenAI 제공자 프리셋의 기본 모델과 같음)
pub const DEFAULT_MODEL: &str = "gpt-6-luna";

/// 설정에서 고른 Codex 모델. 비어 있으면 [`DEFAULT_MODEL`]입니다.
pub fn chosen_model(config: &PostProcessConfig) -> &str {
    setting(config, MODEL_KEY)
        .filter(|model| !model.is_empty())
        .unwrap_or(DEFAULT_MODEL)
}

/// 설정 파일에 적힌 모델 이름 그대로 (없으면 빈 문자열). 화면의 입력 칸에 씁니다.
pub fn model_setting(config: &PostProcessConfig) -> &str {
    setting(config, MODEL_KEY).unwrap_or_default()
}

/// 설정에서 고른 추론 단계. 없거나 모르는 값이면 모델 기본값입니다.
pub fn chosen_effort(config: &PostProcessConfig) -> ReasoningEffort {
    setting(config, REASONING_EFFORT_KEY)
        .and_then(parse_effort)
        .unwrap_or_default()
}

/// 고른 모델을 설정에 적습니다.
pub fn set_model(config: &mut PostProcessConfig, model: &str) {
    write_setting(config, MODEL_KEY, model.trim());
}

/// 고른 추론 단계를 설정에 적습니다.
///
/// 모델 기본값도 지우지 않고 `"default"`로 적습니다. 키를 지우면 저장할 때 파일에 남은 옛 값이 되살아나기
/// 때문입니다(`sulsul_core::config`의 병합 규칙).
pub fn set_effort(config: &mut PostProcessConfig, effort: ReasoningEffort) {
    write_setting(config, REASONING_EFFORT_KEY, effort_name(effort));
}

/// 요청에 쓰는 ureq 에이전트. 제한 시간을 두고, HTTP 오류 상태도 응답으로 받습니다.
pub(crate) fn http_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into()
}

fn setting<'a>(config: &'a PostProcessConfig, key: &str) -> Option<&'a str> {
    config
        .processors
        .get(CODEX_PROCESSOR_ID)
        .and_then(|table| table.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
}

fn write_setting(config: &mut PostProcessConfig, key: &str, value: &str) {
    config
        .processors
        .entry(CODEX_PROCESSOR_ID.to_owned())
        .or_default()
        .insert(key.to_owned(), Value::String(value.to_owned()));
}

/// 설정 파일에 적는 추론 단계 이름 (`ApiProfile::reasoning_effort`와 같은 이름)
fn effort_name(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Default => "default",
        ReasoningEffort::None => "none",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
    }
}

fn parse_effort(name: &str) -> Option<ReasoningEffort> {
    ReasoningEffort::ALL
        .into_iter()
        .find(|effort| effort_name(*effort).eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_falls_back_to_the_default() {
        let mut config = PostProcessConfig::default();
        assert_eq!(chosen_model(&config), DEFAULT_MODEL);
        set_model(&mut config, " gpt-5.6-terra ");
        assert_eq!(chosen_model(&config), "gpt-5.6-terra");
        set_model(&mut config, "  ");
        assert_eq!(
            chosen_model(&config),
            DEFAULT_MODEL,
            "비운 모델은 기본 모델로 읽어야 합니다"
        );
    }

    #[test]
    fn effort_round_trips_and_matches_the_profile_names() {
        let mut config = PostProcessConfig::default();
        assert_eq!(chosen_effort(&config), ReasoningEffort::Default);
        for effort in ReasoningEffort::ALL {
            set_effort(&mut config, effort);
            assert_eq!(chosen_effort(&config), effort);
            // 외부 제공자 프로필(`reasoning_effort`)과 같은 이름이어야 사용자가 헷갈리지 않습니다.
            let serialized = Value::try_from(effort).unwrap();
            assert_eq!(serialized.as_str(), Some(effort_name(effort)));
        }
        write_setting(&mut config, REASONING_EFFORT_KEY, "turbo");
        assert_eq!(
            chosen_effort(&config),
            ReasoningEffort::Default,
            "모르는 값은 모델 기본값으로 읽어야 합니다"
        );
    }
}
