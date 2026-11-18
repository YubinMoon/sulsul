//! 글자 크기 단계에 맞춘 글 도우미.

use egui::{Label, RichText, Ui};

use crate::theme::{self, Palette, size};

/// 한 줄 미리보기([`preview_line`])의 최대 글자 수
pub const PREVIEW_CHARS: usize = 160;

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

/// 글을 한 줄 미리보기로 줄입니다: 줄바꿈과 이어진 빈칸을 빈칸 하나로 바꾸고 앞부분만 남깁니다.
pub fn preview_line(text: &str) -> String {
    let mut line = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
        if count > 0 {
            line.push(' ');
            count += 1;
        }
        for ch in word.chars() {
            if count >= PREVIEW_CHARS {
                line.push('…');
                return line;
            }
            line.push(ch);
            count += 1;
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_is_a_single_shortened_line() {
        assert_eq!(
            preview_line("  첫 줄\n\n둘째   줄\t끝 "),
            "첫 줄 둘째 줄 끝"
        );
        assert_eq!(preview_line(" \n "), "");

        let long = "가".repeat(PREVIEW_CHARS + 50);
        let preview = preview_line(&long);
        assert_eq!(preview.chars().count(), PREVIEW_CHARS + 1);
        assert!(preview.ends_with('…'));
    }
}
