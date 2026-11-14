//! 화면(페이지).

pub mod about;
pub mod record;
pub mod settings;
pub mod transcribe;

use egui::Ui;
use rust_i18n::t;

use crate::i18n::{tr, tr_opt};
use crate::icons::Icon;
use crate::widgets::controls::ComboItem;
use crate::widgets::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Record,
    Transcribe,
    Settings,
    About,
}

impl Page {
    pub const ALL: [Self; 4] = [Self::Record, Self::Transcribe, Self::Settings, Self::About];

    pub fn title(self) -> String {
        match self {
            Self::Record => t!("nav.record"),
            Self::Transcribe => t!("nav.transcribe"),
            Self::Settings => t!("nav.settings"),
            Self::About => t!("nav.about"),
        }
        .into_owned()
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::Record => Icon::Mic,
            Self::Transcribe => Icon::AudioLines,
            Self::Settings => Icon::Settings,
            Self::About => Icon::Info,
        }
    }
}

/// 화면 맨 위의 제목과 한 줄 설명
pub fn header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(text::title(title));
        ui.label(text::muted(ui, subtitle));
    });
    ui.add_space(SECTION_GAP);
}

/// 카드 사이 간격
pub const SECTION_GAP: f32 = 14.0;

/// 인식 언어로 고를 수 있는 값
const LANGUAGES: [&str; 3] = ["ko", "en", "auto"];

/// 인식 언어 선택 항목. `current`가 목록에 없는 값이면 그 값도 넣습니다.
pub fn language_items(current: &str) -> Vec<ComboItem<String>> {
    let mut items: Vec<_> = LANGUAGES
        .iter()
        .map(|code| {
            ComboItem::new((*code).to_owned(), tr(&format!("stt.language.{code}"))).detail(*code)
        })
        .collect();
    if !LANGUAGES.contains(&current) {
        let label =
            tr_opt(&format!("stt.language.{current}")).unwrap_or_else(|| current.to_owned());
        items.push(ComboItem::new(current.to_owned(), label));
    }
    items
}
