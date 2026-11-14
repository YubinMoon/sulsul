//! 글꼴 등록.
//!
//! egui 기본 글꼴에는 한글이 없어서 Pretendard(OFL-1.1)를 실행 파일에 넣어 둡니다.
//! 라이선스는 `assets/fonts/Pretendard-LICENSE.txt`에 있습니다.

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily, FontTweak};

use crate::icons;
use crate::theme::SEMIBOLD;

const REGULAR_NAME: &str = "Pretendard-Regular";
const SEMIBOLD_NAME: &str = "Pretendard-SemiBold";
const ICONS_NAME: &str = "lucide";

/// 아이콘을 아래로 내리는 정도 (글꼴 크기에 대한 비율)
const ICON_Y_OFFSET: f32 = 0.07;

const REGULAR_BYTES: &[u8] = include_bytes!("../assets/fonts/Pretendard-Regular.otf");
const SEMIBOLD_BYTES: &[u8] = include_bytes!("../assets/fonts/Pretendard-SemiBold.otf");

pub fn install(ctx: &egui::Context) {
    // egui 기본 글꼴(영문·기호·고정폭)은 뒤쪽 대체 글꼴로 남겨 둡니다.
    let mut fonts = FontDefinitions::default();

    let mut add = |name: &str, data: FontData| {
        fonts.font_data.insert(name.to_owned(), Arc::new(data));
    };
    add(REGULAR_NAME, FontData::from_static(REGULAR_BYTES));
    add(SEMIBOLD_NAME, FontData::from_static(SEMIBOLD_BYTES));
    add(
        ICONS_NAME,
        FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES).tweak(FontTweak {
            // 문구 옆에 놓았을 때 글자 가운데와 맞도록 살짝 내립니다.
            y_offset_factor: ICON_Y_OFFSET,
            ..FontTweak::default()
        }),
    );

    let fallbacks = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    let with_fallbacks = |first: &[&str]| -> Vec<String> {
        first
            .iter()
            .map(|name| (*name).to_owned())
            .chain(fallbacks.iter().cloned())
            .collect()
    };

    fonts
        .families
        .insert(FontFamily::Proportional, with_fallbacks(&[REGULAR_NAME]));
    fonts.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        with_fallbacks(&[SEMIBOLD_NAME, REGULAR_NAME]),
    );
    // 아이콘은 전용 글꼴 모음으로만 그립니다 (이유는 `icons` 모듈 설명 참고).
    fonts.families.insert(
        FontFamily::Name(icons::FAMILY.into()),
        vec![ICONS_NAME.to_owned(), REGULAR_NAME.to_owned()],
    );
    // 고정폭 글꼴에는 한글이 없으므로 바로 뒤에 Pretendard를 둡니다.
    let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
    let at = monospace.len().min(1);
    monospace.insert(at, REGULAR_NAME.to_owned());

    ctx.set_fonts(fonts);
}
