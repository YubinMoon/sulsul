//! 아이콘 (Lucide 아이콘 글꼴).
//!
//! 아이콘 글꼴은 사용자 영역 문자(U+E000~)를 쓰는데, Pretendard에도 같은 자리에
//! 다른 글리프가 있습니다. 그래서 일반 글꼴의 대체 글꼴로 섞지 않고, 아이콘 전용
//! 글꼴 모음([`FAMILY`])으로 따로 등록해 아이콘을 그릴 때만 씁니다.

use egui::text::LayoutJob;
use egui::{Color32, FontFamily, FontId, RichText, TextFormat, TextStyle, Ui};

pub use lucide_icons::Icon;

/// 아이콘 전용 글꼴 모음 이름 ([`crate::fonts`]에서 등록)
pub const FAMILY: &str = "icons";

/// 문구 옆 아이콘과 문구 사이 간격
const TEXT_GAP: f32 = 7.0;

/// 아이콘 글꼴
pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY.into()))
}

/// 아이콘 하나 (크기·색은 돌려받은 값에 이어서 지정)
pub fn text(icon: Icon) -> RichText {
    RichText::new(icon.unicode().to_string()).family(FontFamily::Name(FAMILY.into()))
}

/// 아이콘과 문구를 나란히 놓은 글 (버튼 등에 사용). 색은 위젯의 글자색을 따릅니다.
pub fn with_text(ui: &Ui, icon: Icon, text: impl AsRef<str>) -> LayoutJob {
    with_text_in(icon, text, TextStyle::Button.resolve(ui.style()))
}

/// [`with_text`]와 같지만 문구의 글꼴을 직접 정합니다.
pub fn with_text_in(icon: Icon, text: impl AsRef<str>, text_font: FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        &icon.unicode().to_string(),
        0.0,
        TextFormat::simple(font(text_font.size + 1.0), Color32::PLACEHOLDER),
    );
    job.append(
        text.as_ref(),
        TEXT_GAP,
        TextFormat::simple(text_font, Color32::PLACEHOLDER),
    );
    job
}
