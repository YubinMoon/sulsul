//! 인식 언어 고르기. 설정 화면, 모델 카드, 테스트 화면의 받아쓰기 테스트가 함께 씁니다.
//!
//! 고를 수 있는 언어는 엔진과 모델마다 다르므로([`LanguageSupport`]) 부르는 쪽에서 넘겨받습니다.
//! 목록에는 자동 감지와 한국어·영어·중국어·일본어 가운데 그 모델이 인식하는 것만 나옵니다
//! ([`LanguageSupport::choices`]). 그 밖의 언어 이름은 자동 감지 결과를 보여 줄 때 씁니다.

use egui::Ui;
use rust_i18n::t;
use sulsul_core::settings::SettingField;
use sulsul_core::stt::LanguageSupport;
use sulsul_core::toml::{Table, Value};

use super::controls::{self, ComboItem};
use super::row::{self, Row};
use super::setting_field::invalid_value_warning;
use crate::i18n::{tr, tr_opt};

/// 자동 감지를 뜻하는 설정 값
const AUTO: &str = "auto";

/// 언어 코드(또는 `auto`)를 화면에 보일 이름으로 바꿉니다. 이름을 모르면 코드를 그대로 돌려줍니다.
pub fn language_name(code: &str) -> String {
    // 언어 이름은 `stt.language.<코드>`에 있습니다. 같은 자리의 다른 문구(`label`, `help` 등)를
    // 언어 이름으로 잘못 읽지 않도록, 언어 코드처럼 생긴 값만 찾습니다.
    let looks_like_code = code == AUTO
        || ((2..=3).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase()));
    looks_like_code
        .then(|| tr_opt(&format!("stt.language.{code}")))
        .flatten()
        .unwrap_or_else(|| code.to_owned())
}

/// 언어 선택 항목. `choices`([`LanguageSupport::choices`])의 순서를 그대로 따릅니다.
///
/// `current`가 `choices`에 없는 값이면 그 값도 맨 위에 고를 수 없는 항목으로 넣습니다.
/// 그러면 닫힌 콤보에서 경고색으로 보이고, 목록에서는 이유가 함께 보입니다.
pub fn language_items(choices: &[String], current: &str) -> Vec<ComboItem<String>> {
    let item = |code: &str| {
        let item = ComboItem::new(code.to_owned(), language_name(code));
        if code == AUTO {
            item
        } else {
            item.detail(code)
        }
    };

    let mut items = Vec::with_capacity(choices.len() + 1);
    if !current.is_empty() && !choices.iter().any(|code| code == current) {
        items.push(item(current).unavailable(t!("stt.language.unsupported")));
    }
    items.extend(choices.iter().map(|code| item(code)));
    items
}

/// 언어 드롭다운. 값이 바뀌면 `true`를 돌려줍니다.
///
/// `items`는 [`language_items`]로 만든 항목이거나, 거기에 "설정 값 사용" 같은 항목을 더한 것입니다.
pub fn language_combo<T: PartialEq + Clone>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    width: f32,
    current: &mut T,
    items: &[ComboItem<T>],
) -> bool {
    controls::combo(ui, id_salt, width, current, items, "")
}

/// 언어 하나만 인식하는 모델(예: 영어 전용)의 고정 표시. 드롭다운 자리에 그 언어 이름만 보여 줍니다.
pub fn fixed_language(ui: &mut Ui, code: &str) {
    controls::badge(ui, &language_name(code));
}

/// 언어 하나만 인식하는 모델에 붙이는 안내 (예: "영어 전용 모델이라 … 영어로 인식합니다.")
pub fn single_language_note(code: &str) -> String {
    t!("stt.language.single_note", language = language_name(code)).into_owned()
}

/// 고른 언어를 이 모델에서 쓸 수 없을 때의 경고
pub fn unsupported_warning(code: &str) -> String {
    t!(
        "stt.language.unsupported_warning",
        language = language_name(code)
    )
    .into_owned()
}

/// 엔진 설정의 인식 언어 줄. 값이 바뀌면 `table`에 쓰고 `true`를 돌려줍니다.
///
/// - `field`: 엔진 스키마의 언어 항목 (이름, 기본값, 값 검사에 씁니다)
/// - `help`: 줄 이름 아래에 보일 도움말
/// - `support`: 이 줄이 가리키는 모델이 인식할 수 있는 언어. 엔진 스키마의 선택지보다 좁을 수 있습니다.
/// - `table`: 그 엔진의 설정 표(`[stt.engines.<엔진 ID>]`)
/// - `resolved`: 표에 값이 없거나 잘못됐을 때 실제로 쓰이는 값까지 반영한 언어
///   (`SttRegistry::resolve_settings`의 `language`)
///
/// 언어 하나만 인식하는 모델이면 드롭다운 대신 그 언어를 고정으로 보여 주고 값은 바꾸지 않습니다.
/// 엔진 설정은 그 엔진의 다른 모델도 함께 쓰기 때문입니다.
pub fn language_setting_row(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    field: &SettingField,
    help: &str,
    support: &LanguageSupport,
    table: &mut Table,
    resolved: &str,
) -> bool {
    let label = tr(&field.label_key);
    if let Some(only) = support.single() {
        let note = single_language_note(only);
        Row::new(&label)
            .help(&note)
            .show(ui, |ui| fixed_language(ui, only));
        return false;
    }

    let choices = support.choices();
    // 저장된 값이 엔진 스키마와 맞지 않으면 다른 항목처럼 기본값을 보여 주고 그 사실을 알립니다.
    // 스키마에는 맞지만 이 모델이 인식하지 못하는 언어면 그 값을 경고색으로 보여 줍니다.
    let warning = invalid_value_warning(field, table).or_else(|| {
        (!choices.iter().any(|code| code == resolved)).then(|| unsupported_warning(resolved))
    });
    let items = language_items(&choices, resolved);
    let mut value = resolved.to_owned();
    let changed = Row::new(&label)
        .help(help)
        .warning_opt(warning.as_deref())
        .show(ui, |ui| {
            // 좁은 창에서는 조작 영역이 기본 너비보다 좁아집니다.
            let width = row::CONTROL_WIDTH.min(ui.available_width());
            language_combo(ui, id_salt, width, &mut value, &items)
        });
    if changed {
        table.insert(field.key.clone(), Value::String(value));
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::stt::{LANGUAGE_KEY, WHISPER_LANGUAGES, language_field};

    fn codes(items: &[ComboItem<String>]) -> Vec<&str> {
        items.iter().map(|item| item.value.as_str()).collect()
    }

    #[test]
    fn every_whisper_language_has_a_name() {
        rust_i18n::set_locale("ko");
        assert_eq!(language_name("auto"), "자동 감지");
        assert_eq!(language_name("ko"), "한국어");
        for code in WHISPER_LANGUAGES {
            assert_ne!(language_name(code), *code, "{code}: 언어 이름이 없습니다");
        }
    }

    #[test]
    fn other_texts_are_not_read_as_language_names() {
        rust_i18n::set_locale("ko");
        // `stt.language.label`처럼 같은 자리에 있는 문구를 언어 이름으로 보여 주면 안 됩니다.
        for value in ["label", "help", "unsupported", "xx", "KO", ""] {
            assert_eq!(language_name(value), value);
        }
    }

    #[test]
    fn languages_keep_the_order_they_are_given_in() {
        rust_i18n::set_locale("ko");
        // 이름순으로 다시 놓지 않고 받은 순서 그대로 보여 줍니다.
        let choices = ["auto", "ja", "en", "ko"].map(str::to_owned);
        let items = language_items(&choices, "ko");

        assert_eq!(codes(&items), ["auto", "ja", "en", "ko"]);
        // 자동 감지에는 코드를 붙이지 않고, 언어에는 코드를 함께 보여 줍니다.
        assert_eq!(items[0].detail, None);
        assert_eq!(items[1].label, "일본어");
        assert_eq!(items[1].detail.as_deref(), Some("ja"));
        assert!(items.iter().all(|item| item.unavailable.is_none()));
    }

    #[test]
    fn only_offered_languages_the_model_supports_are_listed() {
        rust_i18n::set_locale("ko");
        let offered = ["auto", "ko", "en", "zh", "ja"];

        // 언어를 가리지 않는 엔진과 Whisper 다국어 모델: 자동 감지와 네 언어
        let any = LanguageSupport::Any.choices();
        assert_eq!(codes(&language_items(&any, "ko")), offered);
        let whisper =
            LanguageSupport::AutoAnd(WHISPER_LANGUAGES.iter().map(|&c| c.into()).collect());
        assert_eq!(codes(&language_items(&whisper.choices(), "ko")), offered);

        // SenseVoice는 광둥어도 인식하지만 목록에는 넣지 않습니다.
        let sensevoice =
            LanguageSupport::AutoAnd(["ko", "en", "zh", "ja", "yue"].map(str::to_owned).to_vec());
        let items = language_items(&sensevoice.choices(), "auto");
        assert_eq!(codes(&items), offered);
        assert!(
            !codes(&items).contains(&"yue"),
            "광둥어는 목록에 나오면 안 됩니다"
        );
        assert!(items.iter().all(|item| item.unavailable.is_none()));

        // 자동 감지를 못 하는 모델이면 자동 감지 항목이 없고, 인식하지 못하는 언어도 없습니다.
        let only = LanguageSupport::Only(vec!["en".into(), "ko".into()]);
        assert_eq!(codes(&language_items(&only.choices(), "ko")), ["ko", "en"]);
    }

    #[test]
    fn unsupported_current_value_is_shown_as_unavailable() {
        rust_i18n::set_locale("ko");
        let support = LanguageSupport::AutoAnd(vec!["ko".into(), "en".into()]);
        let items = language_items(&support.choices(), "fr");
        assert_eq!(codes(&items), ["fr", "auto", "ko", "en"]);
        assert_eq!(items[0].label, "프랑스어");
        assert!(items[0].unavailable.is_some());
        assert!(items[1..].iter().all(|item| item.unavailable.is_none()));

        // 값이 비어 있으면(모르는 엔진 등) 빈 항목을 만들지 않습니다.
        assert_eq!(
            codes(&language_items(&support.choices(), "")),
            ["auto", "ko", "en"]
        );
    }

    #[test]
    fn schema_language_field_uses_the_names_shown_here() {
        // 엔진 스키마의 언어 항목과 이 위젯이 같은 번역 키를 봅니다.
        let field = language_field(&LanguageSupport::Any, "ko");
        assert_eq!(field.key, LANGUAGE_KEY);
        assert_eq!(field.label_key, "stt.language.label");
    }
}
