//! 번역 도우미.
//!
//! 화면 문구는 모두 `locales/*.yml`에 있고, 코드에서는 키로만 참조합니다.

use rust_i18n::t;
use sulsul_core::config::HotkeyMode;

/// 실행 중에 정해지는 키(엔진 설명자의 `name_key` 등)를 번역합니다.
/// 번역이 없으면 키를 그대로 돌려줍니다.
pub fn tr(key: &str) -> String {
    t!(key).into_owned()
}

/// 번역이 있을 때만 돌려줍니다.
pub fn tr_opt(key: &str) -> Option<String> {
    let text = t!(key);
    (text != key).then(|| text.into_owned())
}

/// 번역 파일이 있는 언어 코드 목록
pub fn available_locales() -> Vec<String> {
    let mut locales: Vec<String> = rust_i18n::available_locales!()
        .into_iter()
        .map(|locale| locale.to_string())
        .collect();
    locales.sort_unstable();
    locales
}

/// 언어 코드를 그 언어의 이름으로 바꿉니다 (번역이 없으면 코드 그대로).
pub fn language_name(code: &str) -> String {
    tr_opt(&format!("language.{code}")).unwrap_or_else(|| code.to_owned())
}

/// 단축키 설정 값(`"RightCtrl"`, `"Ctrl+Shift+Space"`)을 화면에 보일 키 이름으로 나눕니다.
///
/// `+`로 나눈 부분마다 `hotkey.keys.<이름>` 번역으로 바꾸고, 번역이 없는 키는 이름 그대로 둡니다.
pub fn hotkey_key_parts(key: &str) -> Vec<String> {
    key.split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| tr_opt(&format!("hotkey.keys.{part}")).unwrap_or_else(|| part.to_owned()))
        .collect()
}

/// 단축키 동작 방식의 이름 (토글, 누르는 동안)
pub fn hotkey_mode_name(mode: HotkeyMode) -> String {
    match mode {
        HotkeyMode::Toggle => t!("hotkey.mode.toggle"),
        HotkeyMode::PushToTalk => t!("hotkey.mode.push_to_talk"),
    }
    .into_owned()
}

/// 설정의 화면 언어를 적용합니다. 번역 파일이 없는 언어면 기본 언어로 표시됩니다.
pub fn apply_locale(code: &str) {
    if *rust_i18n::locale() != *code {
        rust_i18n::set_locale(code);
    }
}

/// 마이크 권한 설정 위치 안내 (OS별)
pub fn microphone_settings_hint() -> String {
    match std::env::consts::OS {
        "windows" => t!("hint.microphone_settings.windows"),
        "macos" => t!("hint.microphone_settings.macos"),
        _ => t!("hint.microphone_settings.other"),
    }
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// 코드에서 `t!` 매크로에 글자 그대로 넘긴 키를 모읍니다.
    /// (`format!`처럼 이름이 `t!`로 끝나는 다른 매크로는 건너뜁니다)
    fn literal_keys(source: &str) -> Vec<String> {
        const MACRO: &str = "t!(";
        let is_key_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.';
        let mut keys = Vec::new();
        for (at, _) in source.match_indices(MACRO) {
            let part_of_longer_name = source[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            if part_of_longer_name {
                continue;
            }
            let inner = source[at + MACRO.len()..].trim_start();
            if let Some(quoted) = inner.strip_prefix('"')
                && let Some(end) = quoted.find('"')
                && !quoted[..end].is_empty()
                && quoted[..end].chars().all(is_key_char)
            {
                keys.push(quoted[..end].to_owned());
            }
        }
        keys
    }

    #[test]
    fn literal_keys_finds_only_translation_macros() {
        // 이 파일도 검사 대상이라, 예시가 진짜 키로 잡히지 않도록 매크로 이름을 나눠 적습니다.
        let source = [
            r#"M("a.b"); format!("{x}"); M( "c_d", n = 1);"#,
            r#"print!("no"); M(key); M("not a key");"#,
        ]
        .concat()
        .replace("M(", "t!(");
        assert_eq!(literal_keys(&source), ["a.b", "c_d"]);
    }

    /// 번역이 빠진 키는 화면에 키 이름이 그대로 보이므로 미리 잡아냅니다.
    #[test]
    fn every_key_used_in_code_has_a_translation() {
        rust_i18n::set_locale("ko");
        let mut files = Vec::new();
        rust_files(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        assert!(!files.is_empty());

        let mut missing = Vec::new();
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            for key in literal_keys(&source) {
                if tr_opt(&key).is_none() {
                    missing.push(format!("{key} ({})", file.display()));
                }
            }
        }
        assert!(missing.is_empty(), "번역이 없는 키: {missing:#?}");
    }

    /// 모델 카탈로그가 가리키는 번역 키(모델 이름·설명, 단계별 선택지, 단계 이름)가 모두 있어야 합니다.
    /// 빠지면 모델 관리 화면에 키 이름이나 모델 ID가 그대로 보입니다.
    #[test]
    fn model_catalogs_have_translations() {
        use sulsul_core::AppPaths;
        use sulsul_core::stt::EngineContext;

        rust_i18n::set_locale("ko");
        let dir = tempfile::tempdir().unwrap();
        let ctx = EngineContext {
            paths: AppPaths::under(dir.path()),
        };
        let registry = crate::engines::registry();

        let mut keys = Vec::new();
        for descriptor in registry.descriptors() {
            let catalog = registry
                .model_variants(&ctx, &descriptor.id)
                .unwrap_or_default();
            for variant in catalog {
                keys.push(variant.name_key);
                // 설명은 없어도 되지만, 키를 적었다면 번역이 있어야 합니다.
                if !variant.description_key.is_empty() {
                    keys.push(variant.description_key);
                }
                for choice in variant.choices {
                    keys.push(crate::pages::models::axis_name_key(choice.axis));
                    keys.push(choice.label_key);
                }
            }
        }
        assert!(!keys.is_empty(), "검사할 모델 카탈로그가 없습니다");
        keys.sort_unstable();
        keys.dedup();

        let missing: Vec<_> = keys
            .into_iter()
            .filter(|key| tr_opt(key).is_none())
            .collect();
        assert!(missing.is_empty(), "번역이 없는 키: {missing:#?}");
    }

    /// 글 다듬기 로컬 모델 카탈로그와 제공자 프리셋이 가리키는 번역 키도 모두 있어야 합니다.
    /// 개발 빌드에만 나오는 가짜 제공자도 함께 봅니다.
    #[test]
    fn text_polishing_catalog_and_providers_have_translations() {
        use sulsul_core::AppPaths;

        rust_i18n::set_locale("ko");
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());

        let mut keys = Vec::new();
        for family in sulsul_llm::catalog::families(&paths) {
            keys.push(family.name_key);
            keys.push(family.description_key);
            for variant in family.variants {
                keys.push(variant.name_key);
                keys.push(variant.description_key);
                for choice in variant.choices {
                    keys.push(crate::pages::models::axis_name_key(choice.axis));
                    keys.push(choice.label_key);
                }
            }
        }
        assert!(!keys.is_empty(), "검사할 모델 카탈로그가 없습니다");

        let presets = sulsul_llm::presets::presets(true);
        assert!(
            presets
                .iter()
                .any(|preset| preset.id == sulsul_llm::presets::MOCK_PROVIDER_ID),
            "가짜 제공자가 검사 대상에 없습니다"
        );
        keys.extend(presets.iter().map(|preset| preset.name_key.to_owned()));
        keys.sort_unstable();
        keys.dedup();

        let missing: Vec<_> = keys
            .into_iter()
            .filter(|key| tr_opt(key).is_none())
            .collect();
        assert!(missing.is_empty(), "번역이 없는 키: {missing:#?}");
    }

    #[test]
    fn hotkey_names_are_translated_part_by_part() {
        rust_i18n::set_locale("ko");
        assert_eq!(hotkey_key_parts("RightCtrl"), ["오른쪽 Ctrl"]);
        assert_eq!(
            hotkey_key_parts("Ctrl+Shift+Space"),
            ["Ctrl", "Shift", "스페이스"]
        );
        // 번역이 없는 키는 이름 그대로, 빈 부분은 건너뜁니다.
        assert_eq!(hotkey_key_parts(" Alt + F13 +"), ["Alt", "F13"]);
        assert!(hotkey_key_parts("").is_empty());
        assert_eq!(
            hotkey_key_parts("Ctrl+Alt+HangulMode"),
            ["Ctrl", "Alt", "한/영"]
        );
        // 기본 단축키는 설정 값 그대로가 아니라 화면 이름으로 보여야 합니다.
        let default_key = sulsul_core::config::Config::default().hotkey.key;
        assert_ne!(
            hotkey_key_parts(&default_key),
            [default_key.as_str()],
            "기본 단축키의 화면 이름이 없습니다"
        );
    }

    #[test]
    fn default_locale_is_available() {
        assert!(available_locales().iter().any(|locale| locale == "ko"));
        assert_eq!(language_name("ko"), "한국어");
        assert_eq!(language_name("xx"), "xx");
    }
}
