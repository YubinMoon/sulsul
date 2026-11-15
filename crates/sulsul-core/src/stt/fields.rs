//! 여러 엔진이 함께 쓰는 설정 항목: 인식 언어, 모델 메모리 유지 시간.
//!
//! 엔진은 이 함수들로 만든 항목을 자기 스키마([`super::EngineDescriptor::settings`])에 넣습니다.
//! 값은 다른 엔진 설정과 마찬가지로 `[stt.engines.<ID>]`에 저장됩니다.

use std::time::Duration;

use toml::Value;

use super::{EngineDescriptor, Language, LanguageSupport, SttEngineFactory, SttError};
use crate::config::SttConfig;
use crate::settings::{ChoiceOption, ResolvedSettings, SettingField};

/// whisper.cpp가 아는 언어 코드 전부. whisper.cpp의 언어 ID 순서(large-v3 기준 100개)와 같습니다.
///
/// `jw`(자바어)처럼 ISO 639-1과 다른 whisper 고유 코드도 그대로 둡니다. 모델이 아는 언어(모델 사실)라서
/// 고를 수 있는 언어는 아닙니다([`OFFERED_LANGUAGES`]). 자동 감지가 돌려준 언어를 이름으로 보여 줄 때도 씁니다.
pub const WHISPER_LANGUAGES: &[&str] = &[
    "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it",
    "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur",
    "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn",
    "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si",
    "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo",
    "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln",
    "ha", "ba", "jw", "su", "yue",
];

/// 화면과 CLI에서 고를 수 있는 인식 언어(이 순서로 보임). 모델이 지원하는 것만 실제 선택지가 됩니다.
pub const OFFERED_LANGUAGES: &[&str] = &["ko", "en", "zh", "ja"];

/// 인식 언어 설정 키 (`ko`, `en`, `auto` 등)
pub const LANGUAGE_KEY: &str = "language";

/// 엔진 설정에 언어가 없고 이어받을 예전 값도 없을 때 쓰는 인식 언어
pub const DEFAULT_LANGUAGE: &str = "ko";

/// 모델을 메모리에 담아 두는 시간(분) 설정 키
pub const KEEP_LOADED_KEY: &str = "keep_loaded_minutes";

/// 모델 메모리 유지 시간의 기본값(분)
pub const DEFAULT_KEEP_LOADED_MINUTES: i64 = 5;

/// 모델 메모리 유지 시간의 최댓값(분, 하루)
pub const MAX_KEEP_LOADED_MINUTES: i64 = 24 * 60;

/// 인식 언어 항목. 선택지는 [`LanguageSupport::choices`]입니다: 자동 감지를 지원하면 `auto`가 맨 앞이고,
/// 그다음이 [`OFFERED_LANGUAGES`] 가운데 `support`가 지원하는 언어입니다. 선택지에 없는 값(예: 설정 파일의
/// `language = "fr"`)은 다른 잘못된 값처럼 기본값으로 읽히고 설정 화면에 경고가 뜹니다.
///
/// 각 선택지의 이름은 `stt.language.<코드>` 번역 키에 있습니다.
pub fn language_field(support: &LanguageSupport, default: &str) -> SettingField {
    let options = support
        .choices()
        .into_iter()
        .map(|code| {
            let option = ChoiceOption::new(code.clone()).with_label(format!("stt.language.{code}"));
            if code == "auto" {
                option
            } else {
                option.with_detail(code)
            }
        })
        .collect();
    SettingField::choice(LANGUAGE_KEY, "stt.language.label", default, options)
        .with_help("stt.language.help")
}

/// 모델 메모리 유지 시간 항목(분). 받아쓰기가 끝난 뒤 이 시간 동안 쓰지 않으면
/// [`super::Transcriber`]가 준비해 둔 엔진을 내려놓습니다. 0이면 끝나자마자 내려놓습니다.
pub fn keep_loaded_field() -> SettingField {
    SettingField::integer(
        KEEP_LOADED_KEY,
        "stt.keep_loaded.label",
        DEFAULT_KEEP_LOADED_MINUTES,
        0,
        MAX_KEEP_LOADED_MINUTES,
    )
    .with_help("stt.keep_loaded.help")
}

/// 설정의 모델 메모리 유지 시간. 이 항목이 없는 엔진(클라우드 등)이면 `None`입니다.
pub(crate) fn keep_loaded(settings: &ResolvedSettings) -> Option<Duration> {
    let minutes = settings
        .get(KEEP_LOADED_KEY)?
        .as_integer()?
        .clamp(0, MAX_KEEP_LOADED_MINUTES);
    Some(Duration::from_secs(minutes.unsigned_abs() * 60))
}

/// 엔진 설정의 인식 언어. 언어 항목이 없는 엔진이면 예전 전역 값(`[stt] language`)을 씁니다.
pub(crate) fn configured_language(settings: &ResolvedSettings, stt: &SttConfig) -> Language {
    let value = settings
        .get(LANGUAGE_KEY)
        .and_then(Value::as_str)
        .unwrap_or(&stt.language);
    Language::from_config(value)
}

/// 실제로 인식에 쓸 언어 ([`super::SttRegistry::resolve_language`] 참고).
pub(crate) fn effective_language(
    factory: &dyn SttEngineFactory,
    settings: &ResolvedSettings,
    stt: &SttConfig,
    requested: Option<&str>,
) -> Result<Language, SttError> {
    let requested = match requested {
        Some(language) => Language::from_config(language),
        None => configured_language(settings, stt),
    };
    factory.languages(settings).resolve(requested)
}

/// 엔진 표에 언어가 없을 때 이어받을 예전 전역 인식 언어(`[stt] language`).
///
/// 엔진 표에 `language`가 이미 있거나, 엔진에 언어 항목이 없거나, 예전 값이 비었거나 그 엔진에서
/// 고를 수 없는 값이면 `None`입니다.
pub(crate) fn legacy_language(descriptor: &EngineDescriptor, stt: &SttConfig) -> Option<Value> {
    let has_own = stt
        .engines
        .get(&descriptor.id)
        .is_some_and(|table| table.contains_key(LANGUAGE_KEY));
    if has_own {
        return None;
    }
    let field = descriptor
        .settings
        .iter()
        .find(|field| field.key == LANGUAGE_KEY)?;
    let legacy = stt.language.trim().to_ascii_lowercase();
    if legacy.is_empty() {
        return None;
    }
    let value = Value::String(legacy);
    field.check(&value).is_ok().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingKind;

    fn options(field: &SettingField) -> Vec<&str> {
        match &field.kind {
            SettingKind::Choice { options } => options.iter().map(|o| o.value.as_str()).collect(),
            other => panic!("선택 항목이 아닙니다: {other:?}"),
        }
    }

    #[test]
    fn whisper_languages_are_unique() {
        assert_eq!(WHISPER_LANGUAGES.len(), 100);
        let mut seen = std::collections::HashSet::new();
        for code in WHISPER_LANGUAGES {
            assert!(seen.insert(code), "{code}: 언어 코드가 두 번 들어 있습니다");
        }
        assert!(WHISPER_LANGUAGES.contains(&DEFAULT_LANGUAGE));
        assert!(OFFERED_LANGUAGES.contains(&DEFAULT_LANGUAGE));
    }

    #[test]
    fn language_field_lists_auto_first() {
        let support = LanguageSupport::AutoAnd(vec!["ko".into(), "en".into()]);
        let field = language_field(&support, "ko");
        assert_eq!(field.key, LANGUAGE_KEY);
        assert_eq!(field.label_key, "stt.language.label");
        assert_eq!(field.help_key.as_deref(), Some("stt.language.help"));
        assert_eq!(field.default, Value::String("ko".into()));
        assert_eq!(options(&field), ["auto", "ko", "en"]);
        let SettingKind::Choice { options: choices } = &field.kind else {
            unreachable!();
        };
        assert_eq!(choices[0].label_key.as_deref(), Some("stt.language.auto"));
        assert_eq!(choices[1].label_key.as_deref(), Some("stt.language.ko"));
        assert_eq!(choices[1].detail.as_deref(), Some("ko"));

        let any = language_field(&LanguageSupport::Any, "ko");
        assert_eq!(options(&any), ["auto", "ko", "en", "zh", "ja"]);
    }

    /// 모델이 아는 언어라도 고를 수 있는 언어가 아니면 잘못된 값입니다.
    #[test]
    fn language_field_rejects_languages_that_are_not_offered() {
        let whisper =
            LanguageSupport::AutoAnd(WHISPER_LANGUAGES.iter().map(|&c| c.to_owned()).collect());
        let field = language_field(&whisper, DEFAULT_LANGUAGE);
        assert!(field.check(&Value::String("ja".into())).is_ok());
        assert!(field.check(&Value::String("fr".into())).is_err());

        let mut table = toml::Table::new();
        table.insert(LANGUAGE_KEY.into(), Value::String("fr".into()));
        let resolved = crate::settings::resolve(std::slice::from_ref(&field), Some(&table));
        assert_eq!(resolved.str(LANGUAGE_KEY), DEFAULT_LANGUAGE);
    }

    #[test]
    fn keep_loaded_field_defaults_to_five_minutes() {
        let field = keep_loaded_field();
        assert_eq!(field.key, KEEP_LOADED_KEY);
        assert_eq!(field.default, Value::Integer(5));
        assert_eq!(field.kind, SettingKind::Integer { min: 0, max: 1440 });

        let resolved = crate::settings::resolve(&[field], None);
        assert_eq!(keep_loaded(&resolved), Some(Duration::from_secs(300)));
        assert_eq!(keep_loaded(&ResolvedSettings::default()), None);
    }
}
