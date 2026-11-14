//! 글자 크기 단계에 맞춘 글 도우미.

use egui::{Label, RichText, Ui};

use crate::theme::{self, Palette, size};

/// 화면 제목
pub fn title(text: impl Into<String>) -> RichText {
    RichText::new(text).font(theme::semibold(size::TITLE))
}

/// 카드·구역 제목
pub fn section(text: impl Into<String>) -> RichText {
    RichText::new(text).font(theme::semibold(size::SECTION))
}

/// 굵은 본문
pub fn strong(text: impl Into<String>) -> RichText {
    RichText::new(text).font(theme::semibold(size::BODY))
}

/// 흐린 본문 (보조 설명)
pub fn muted(ui: &Ui, text: impl Into<String>) -> RichText {
    RichText::new(text).color(Palette::of(ui).text_muted)
}

/// 작은 흐린 글 (도움말, 부가 정보)
pub fn caption(ui: &Ui, text: impl Into<String>) -> RichText {
    RichText::new(text)
        .size(size::CAPTION)
        .color(Palette::of(ui).text_muted)
}

/// 고정폭 글 (경로 등)
pub fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text).monospace()
}

/// 경로를 보여 주는 글. 길면 줄을 바꿔 끝까지 보여 주고, 끌어서 복사할 수 있습니다.
pub fn path_label(ui: &Ui, path: &std::path::Path) -> Label {
    Label::new(mono(path.display().to_string()).color(Palette::of(ui).text_muted))
        .selectable(true)
        .wrap()
}
