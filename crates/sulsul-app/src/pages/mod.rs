//! 화면(페이지).

pub mod about;
pub mod history;
pub mod models;
pub mod prompts;
pub mod settings;
pub mod test;

use egui::Ui;
use rust_i18n::t;

use crate::icons::Icon;
use crate::widgets::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Test,
    History,
    Models,
    Prompts,
    Settings,
    About,
}

impl Page {
    pub const ALL: [Self; 6] = [
        Self::Test,
        Self::History,
        Self::Models,
        Self::Prompts,
        Self::Settings,
        Self::About,
    ];

    pub fn title(self) -> String {
        match self {
            Self::Test => t!("nav.test"),
            Self::History => t!("nav.history"),
            Self::Models => t!("nav.models"),
            Self::Prompts => t!("nav.prompts"),
            Self::Settings => t!("nav.settings"),
            Self::About => t!("nav.about"),
        }
        .into_owned()
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::Test => Icon::FlaskConical,
            Self::History => Icon::History,
            Self::Models => Icon::Boxes,
            Self::Prompts => Icon::MessageSquareText,
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
