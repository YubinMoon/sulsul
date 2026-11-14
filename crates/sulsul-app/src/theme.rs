//! 화면 테마: 색상표, 글자 크기 단계, egui 스타일.
//!
//! 배경은 사이드바 → 본문 → 카드 순으로 한 단계씩 밝아지고(라이트는 반대),
//! 강조에는 포인트 색 하나만 씁니다. 경고·오류·성공 색은 상태 표시에만 씁니다.

use egui::style::{HandleShape, ScrollStyle, Selection, WidgetVisuals, Widgets};
use egui::{
    Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, Style, TextStyle,
    ThemePreference, Ui, Visuals, vec2,
};
use sulsul_core::config::Theme;

/// 굵은 글꼴 패밀리 이름 ([`crate::fonts`]에서 등록)
pub const SEMIBOLD: &str = "semibold";

/// 글자 크기 단계
pub mod size {
    pub const CAPTION: f32 = 12.0;
    pub const BODY: f32 = 14.0;
    pub const SECTION: f32 = 15.0;
    pub const SUBTITLE: f32 = 17.0;
    pub const TITLE: f32 = 23.0;
    pub const DISPLAY: f32 = 38.0;
}

/// 모서리 반지름 단계
pub mod radius {
    pub const CONTROL: u8 = 8;
    pub const CARD: u8 = 12;
    pub const PILL: u8 = 255;
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub sidebar: Color32,
    pub background: Color32,
    pub card: Color32,
    pub card_border: Color32,
    /// 입력 칸 배경
    pub field: Color32,
    /// 버튼 배경
    pub control: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    /// 포인트 색 (채움용)
    pub accent: Color32,
    pub accent_hover: Color32,
    /// 포인트 색 (글자용, 배경과 대비 확보)
    pub accent_text: Color32,
    /// 선택된 항목 배경
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub danger_hover: Color32,
    pub danger_soft: Color32,
    pub danger_text: Color32,
    pub warning: Color32,
    pub warning_soft: Color32,
    pub warning_text: Color32,
    pub success: Color32,
    pub success_soft: Color32,
    pub success_text: Color32,
    pub shadow: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Palette {
    pub const DARK: Self = Self {
        sidebar: rgb(0x14161B),
        background: rgb(0x1A1C22),
        card: rgb(0x21242C),
        card_border: rgb(0x2C303A),
        field: rgb(0x16181D),
        control: rgb(0x2B2F39),
        control_hover: rgb(0x353A46),
        control_active: rgb(0x3F4553),
        border: rgb(0x383D49),
        border_strong: rgb(0x4D5464),
        text: rgb(0xE7E9EE),
        text_muted: rgb(0xA3AAB7),
        text_faint: rgb(0x727A89),
        accent: rgb(0x5577F2),
        accent_hover: rgb(0x6887F5),
        accent_text: rgb(0x9DB2FF),
        accent_soft: rgb(0x283150),
        on_accent: rgb(0xFFFFFF),
        danger: rgb(0xE5484D),
        danger_hover: rgb(0xEC5D61),
        danger_soft: rgb(0x3A2024),
        danger_text: rgb(0xFF9A9D),
        warning: rgb(0xF5A524),
        warning_soft: rgb(0x3A2E18),
        warning_text: rgb(0xFFCB6B),
        success: rgb(0x3DD68C),
        success_soft: rgb(0x1B3327),
        success_text: rgb(0x6FE3A9),
        shadow: Color32::from_black_alpha(110),
    };

    pub const LIGHT: Self = Self {
        sidebar: rgb(0xECEEF2),
        background: rgb(0xF5F6F8),
        card: rgb(0xFFFFFF),
        card_border: rgb(0xE0E3E9),
        field: rgb(0xFFFFFF),
        control: rgb(0xEDEFF3),
        control_hover: rgb(0xE2E5EB),
        control_active: rgb(0xD6DAE2),
        border: rgb(0xD2D6DE),
        border_strong: rgb(0xB3B9C5),
        text: rgb(0x1B1E26),
        text_muted: rgb(0x596170),
        text_faint: rgb(0x8A91A0),
        accent: rgb(0x3D5FE0),
        accent_hover: rgb(0x3252CB),
        accent_text: rgb(0x3050CC),
        accent_soft: rgb(0xE2E8FF),
        on_accent: rgb(0xFFFFFF),
        danger: rgb(0xD93036),
        danger_hover: rgb(0xC3262C),
        danger_soft: rgb(0xFDE7E8),
        danger_text: rgb(0xB4232A),
        warning: rgb(0xD9870B),
        warning_soft: rgb(0xFFF1D2),
        warning_text: rgb(0x8A5300),
        success: rgb(0x1E9E63),
        success_soft: rgb(0xDCF3E5),
        success_text: rgb(0x17734A),
        shadow: Color32::from_black_alpha(40),
    };

    /// 지금 화면에 적용된 테마의 색상표
    pub fn of(ui: &Ui) -> Self {
        Self::for_dark_mode(ui.visuals().dark_mode)
    }

    pub fn for_dark_mode(dark: bool) -> Self {
        if dark { Self::DARK } else { Self::LIGHT }
    }
}

/// 다크·라이트 스타일을 등록합니다. 시작할 때 한 번 부릅니다.
pub fn install(ctx: &egui::Context) {
    ctx.set_style_of(egui::Theme::Dark, style(&Palette::DARK, true));
    ctx.set_style_of(egui::Theme::Light, style(&Palette::LIGHT, false));
}

/// 설정의 테마 값을 적용합니다. `System`은 OS 설정을 따릅니다.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    ctx.set_theme(match theme {
        Theme::Dark => ThemePreference::Dark,
        Theme::Light => ThemePreference::Light,
        Theme::System => ThemePreference::System,
    });
}

fn style(p: &Palette, dark: bool) -> Style {
    let mut style = Style {
        text_styles: [
            (TextStyle::Small, regular(size::CAPTION)),
            (TextStyle::Body, regular(size::BODY)),
            (TextStyle::Button, regular(size::BODY)),
            (TextStyle::Heading, semibold(size::TITLE)),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
        ]
        .into(),
        visuals: visuals(p, dark),
        ..Style::default()
    };

    let spacing = &mut style.spacing;
    spacing.item_spacing = vec2(10.0, 8.0);
    spacing.button_padding = vec2(12.0, 6.0);
    spacing.interact_size = vec2(40.0, 32.0);
    spacing.window_margin = Margin::same(20);
    spacing.menu_margin = Margin::same(6);
    spacing.icon_width = 18.0;
    spacing.icon_width_inner = 10.0;
    spacing.icon_spacing = 8.0;
    spacing.slider_width = 150.0;
    spacing.slider_rail_height = 5.0;
    spacing.combo_width = 120.0;
    spacing.combo_height = 320.0;
    spacing.text_edit_width = 260.0;
    spacing.tooltip_width = 360.0;
    spacing.scroll = ScrollStyle {
        bar_width: 8.0,
        floating_allocated_width: 0.0,
        ..ScrollStyle::floating()
    };

    style.interaction.selectable_labels = false;
    style
}

fn visuals(p: &Palette, dark: bool) -> Visuals {
    let base = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let control_radius = CornerRadius::same(radius::CONTROL);

    let widget = |fill: Color32, border: Color32, text: Color32| WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: Stroke::new(1.0, border),
        corner_radius: control_radius,
        fg_stroke: Stroke::new(1.5, text),
        expansion: 0.0,
    };

    Visuals {
        dark_mode: dark,
        override_text_color: None,
        weak_text_color: Some(p.text_muted),
        widgets: Widgets {
            noninteractive: WidgetVisuals {
                bg_stroke: Stroke::new(1.0, p.card_border),
                fg_stroke: Stroke::new(1.0, p.text),
                ..widget(p.card, p.card_border, p.text)
            },
            inactive: widget(p.control, p.border, p.text),
            hovered: widget(p.control_hover, p.border_strong, p.text),
            active: widget(p.control_active, p.accent, p.text),
            open: widget(p.control_hover, p.border_strong, p.text),
        },
        selection: Selection {
            bg_fill: p.accent_soft,
            stroke: Stroke::new(1.0, p.accent_text),
        },
        hyperlink_color: p.accent_text,
        faint_bg_color: p.control,
        extreme_bg_color: p.field,
        text_edit_bg_color: Some(p.field),
        code_bg_color: p.control,
        warn_fg_color: p.warning_text,
        error_fg_color: p.danger_text,
        window_corner_radius: CornerRadius::same(radius::CARD),
        window_shadow: Shadow {
            offset: [0, 12],
            blur: 32,
            spread: 0,
            color: p.shadow,
        },
        window_fill: p.card,
        window_stroke: Stroke::new(1.0, p.border),
        menu_corner_radius: CornerRadius::same(10),
        panel_fill: p.background,
        popup_shadow: Shadow {
            offset: [0, 8],
            blur: 20,
            spread: 0,
            color: p.shadow,
        },
        slider_trailing_fill: true,
        handle_shape: HandleShape::Circle,
        indent_has_left_vline: false,
        ..base
    }
}

/// 두 색을 섞습니다 (`t` = 0이면 `a`, 1이면 `b`).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}
