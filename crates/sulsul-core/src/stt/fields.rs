//! 여러 엔진이 함께 쓰는 설정 항목: 모델 메모리 유지 시간.
//!
//! 엔진은 이 함수들로 만든 항목을 자기 스키마([`super::EngineDescriptor::settings`])에 넣습니다.
//! 값은 다른 엔진 설정과 마찬가지로 `[stt.engines.<ID>]`에 저장됩니다.

use std::time::Duration;

use crate::settings::{ResolvedSettings, SettingField};

/// 모델을 메모리에 담아 두는 시간(분) 설정 키
pub const KEEP_LOADED_KEY: &str = "keep_loaded_minutes";

/// 모델 메모리 유지 시간의 기본값(분)
pub const DEFAULT_KEEP_LOADED_MINUTES: i64 = 5;

/// 모델 메모리 유지 시간의 최댓값(분, 하루)
pub const MAX_KEEP_LOADED_MINUTES: i64 = 24 * 60;

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

#[cfg(test)]
mod tests {
    use toml::Value;

    use super::*;
    use crate::settings::SettingKind;

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
