//! 단축키에 쓰는 키와 그 이름 (OS와 무관).
//!
//! 이름은 설정 파일에 쓰는 형식입니다. 화면에는 `locales/*.yml`의 `hotkey.keys.<이름>`으로 바꿔 보여 줍니다.

use std::borrow::Cow;

/// 물리 키. 수정 키는 왼쪽·오른쪽을 가립니다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    LeftCtrl,
    RightCtrl,
    LeftAlt,
    RightAlt,
    LeftShift,
    RightShift,
    LeftWin,
    RightWin,
    /// `A`~`Z` (대문자 ASCII)
    Letter(u8),
    /// 숫자 줄의 `0`~`9`
    Digit(u8),
    /// `F1`~`F24`
    Function(u8),
    /// 숫자 패드 `0`~`9`
    Numpad(u8),
    Space,
    Enter,
    Tab,
    Backspace,
    Escape,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    CapsLock,
    ScrollLock,
    NumLock,
    Pause,
    PrintScreen,
    ContextMenu,
    /// 한/영 키
    HangulMode,
    /// 한자 키
    Hanja,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadDecimal,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    Backquote,
    /// 이름이 없는 키 (OS의 키 코드). 단축키로는 쓸 수 없지만 "다른 키를 함께 눌렀는지"를 가리는 데 씁니다.
    Other(u32),
}

/// 수정 키 종류
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Win,
}

impl Modifier {
    /// 설정 파일에 쓰는 순서
    pub const ALL: [Self; 4] = [Self::Ctrl, Self::Alt, Self::Shift, Self::Win];

    pub fn name(self) -> &'static str {
        match self {
            Self::Ctrl => "Ctrl",
            Self::Alt => "Alt",
            Self::Shift => "Shift",
            Self::Win => "Win",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        Some(match lower.as_str() {
            "ctrl" | "control" => Self::Ctrl,
            "alt" => Self::Alt,
            "shift" => Self::Shift,
            "win" | "windows" | "super" | "meta" => Self::Win,
            _ => return None,
        })
    }

    pub fn key(self, side: Side) -> Key {
        match (self, side) {
            (Self::Ctrl, Side::Left) => Key::LeftCtrl,
            (Self::Ctrl, Side::Right) => Key::RightCtrl,
            (Self::Alt, Side::Left) => Key::LeftAlt,
            (Self::Alt, Side::Right) => Key::RightAlt,
            (Self::Shift, Side::Left) => Key::LeftShift,
            (Self::Shift, Side::Right) => Key::RightShift,
            (Self::Win, Side::Left) => Key::LeftWin,
            (Self::Win, Side::Right) => Key::RightWin,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
}

/// 이름이 고정된 키 (설정 파일 이름, 키). 앞의 이름이 기본 이름이고 뒤는 같은 키의 다른 이름입니다.
const NAMED: &[(&str, Key)] = &[
    ("LeftCtrl", Key::LeftCtrl),
    ("RightCtrl", Key::RightCtrl),
    ("LeftAlt", Key::LeftAlt),
    ("RightAlt", Key::RightAlt),
    ("LeftShift", Key::LeftShift),
    ("RightShift", Key::RightShift),
    ("LeftWin", Key::LeftWin),
    ("RightWin", Key::RightWin),
    ("Space", Key::Space),
    ("Enter", Key::Enter),
    ("Tab", Key::Tab),
    ("Backspace", Key::Backspace),
    ("Escape", Key::Escape),
    ("Insert", Key::Insert),
    ("Delete", Key::Delete),
    ("Home", Key::Home),
    ("End", Key::End),
    ("PageUp", Key::PageUp),
    ("PageDown", Key::PageDown),
    ("Up", Key::Up),
    ("Down", Key::Down),
    ("Left", Key::Left),
    ("Right", Key::Right),
    ("CapsLock", Key::CapsLock),
    ("ScrollLock", Key::ScrollLock),
    ("NumLock", Key::NumLock),
    ("Pause", Key::Pause),
    ("PrintScreen", Key::PrintScreen),
    ("ContextMenu", Key::ContextMenu),
    ("HangulMode", Key::HangulMode),
    ("Hanja", Key::Hanja),
    ("NumpadAdd", Key::NumpadAdd),
    ("NumpadSubtract", Key::NumpadSubtract),
    ("NumpadMultiply", Key::NumpadMultiply),
    ("NumpadDivide", Key::NumpadDivide),
    ("NumpadDecimal", Key::NumpadDecimal),
    ("Minus", Key::Minus),
    ("Equal", Key::Equal),
    ("BracketLeft", Key::BracketLeft),
    ("BracketRight", Key::BracketRight),
    ("Backslash", Key::Backslash),
    ("Semicolon", Key::Semicolon),
    ("Quote", Key::Quote),
    ("Comma", Key::Comma),
    ("Period", Key::Period),
    ("Slash", Key::Slash),
    ("Backquote", Key::Backquote),
    // 다른 이름 (읽을 때만 받아들이고, 쓸 때는 위의 이름을 씁니다)
    ("LCtrl", Key::LeftCtrl),
    ("RCtrl", Key::RightCtrl),
    ("LeftControl", Key::LeftCtrl),
    ("RightControl", Key::RightCtrl),
    ("LAlt", Key::LeftAlt),
    ("RAlt", Key::RightAlt),
    ("AltGr", Key::RightAlt),
    ("LShift", Key::LeftShift),
    ("RShift", Key::RightShift),
    ("LWin", Key::LeftWin),
    ("RWin", Key::RightWin),
    ("Spacebar", Key::Space),
    ("Return", Key::Enter),
    ("Esc", Key::Escape),
    ("Ins", Key::Insert),
    ("Del", Key::Delete),
    ("PgUp", Key::PageUp),
    ("PgDn", Key::PageDown),
    ("Menu", Key::ContextMenu),
    ("Apps", Key::ContextMenu),
    ("Hangul", Key::HangulMode),
];

impl Key {
    /// 이 키가 수정 키면 그 종류와 방향
    pub fn modifier(self) -> Option<(Modifier, Side)> {
        Some(match self {
            Self::LeftCtrl => (Modifier::Ctrl, Side::Left),
            Self::RightCtrl => (Modifier::Ctrl, Side::Right),
            Self::LeftAlt => (Modifier::Alt, Side::Left),
            Self::RightAlt => (Modifier::Alt, Side::Right),
            Self::LeftShift => (Modifier::Shift, Side::Left),
            Self::RightShift => (Modifier::Shift, Side::Right),
            Self::LeftWin => (Modifier::Win, Side::Left),
            Self::RightWin => (Modifier::Win, Side::Right),
            _ => return None,
        })
    }

    pub fn is_modifier(self) -> bool {
        self.modifier().is_some()
    }

    /// 수정 키 없이 혼자 단축키로 쓸 수 있는지.
    ///
    /// 글자·숫자·기호·방향키처럼 평소 입력에 쓰는 키를 혼자 단축키로 잡으면 그 키를 입력할 수 없게 되므로 막습니다.
    pub fn usable_alone(self) -> bool {
        self.is_modifier()
            || matches!(
                self,
                Self::Function(_)
                    | Self::CapsLock
                    | Self::ScrollLock
                    | Self::NumLock
                    | Self::Pause
                    | Self::PrintScreen
                    | Self::Insert
                    | Self::ContextMenu
                    | Self::HangulMode
                    | Self::Hanja
            )
    }

    /// 글을 입력하는 키인지 (Shift만 함께 누르면 대문자·기호 입력이라 단축키로 쓸 수 없음)
    pub fn is_typing(self) -> bool {
        matches!(
            self,
            Self::Letter(_)
                | Self::Digit(_)
                | Self::Space
                | Self::Enter
                | Self::Tab
                | Self::Backspace
                | Self::Minus
                | Self::Equal
                | Self::BracketLeft
                | Self::BracketRight
                | Self::Backslash
                | Self::Semicolon
                | Self::Quote
                | Self::Comma
                | Self::Period
                | Self::Slash
                | Self::Backquote
        )
    }

    /// 설정 파일에 쓰는 이름. 이름이 없는 키는 `None`입니다.
    pub fn name(self) -> Option<Cow<'static, str>> {
        match self {
            Self::Letter(c) => Some(Cow::Owned(char::from(c).to_string())),
            Self::Digit(d) => Some(Cow::Owned(d.to_string())),
            Self::Function(n) => Some(Cow::Owned(format!("F{n}"))),
            Self::Numpad(d) => Some(Cow::Owned(format!("Numpad{d}"))),
            Self::Other(_) => None,
            key => NAMED
                .iter()
                .find(|(_, named)| *named == key)
                .map(|(name, _)| Cow::Borrowed(*name)),
        }
    }

    /// 설정 파일의 이름을 읽습니다. 대소문자는 가리지 않습니다.
    pub fn from_name(name: &str) -> Option<Self> {
        if let Some((_, key)) = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            return Some(*key);
        }
        let upper = name.to_ascii_uppercase();
        let bytes = upper.as_bytes();
        match bytes {
            [c @ b'A'..=b'Z'] => return Some(Self::Letter(*c)),
            [d @ b'0'..=b'9'] => return Some(Self::Digit(d - b'0')),
            _ => {}
        }
        if let Some(n) = upper.strip_prefix('F').and_then(parse_small)
            && (1..=24).contains(&n)
        {
            return Some(Self::Function(n));
        }
        if let Some(d) = upper.strip_prefix("NUMPAD").and_then(parse_small)
            && d <= 9
        {
            return Some(Self::Numpad(d));
        }
        None
    }
}

/// 앞에 0이 붙지 않은 작은 십진수 (`"13"` → 13)
fn parse_small(text: &str) -> Option<u8> {
    if text.is_empty() || text.len() > 2 || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let mut keys: Vec<Key> = NAMED.iter().map(|(_, key)| *key).collect();
        keys.extend((b'A'..=b'Z').map(Key::Letter));
        keys.extend((0..=9).map(Key::Digit));
        keys.extend((1..=24).map(Key::Function));
        keys.extend((0..=9).map(Key::Numpad));
        for key in keys {
            let name = key.name().expect("이름이 있어야 합니다");
            assert_eq!(Key::from_name(&name), Some(key), "{name}");
        }
    }

    #[test]
    fn names_are_case_insensitive_and_have_aliases() {
        assert_eq!(Key::from_name("rightctrl"), Some(Key::RightCtrl));
        assert_eq!(Key::from_name("RCtrl"), Some(Key::RightCtrl));
        assert_eq!(Key::from_name("esc"), Some(Key::Escape));
        assert_eq!(Key::from_name("f13"), Some(Key::Function(13)));
        assert_eq!(Key::from_name("a"), Some(Key::Letter(b'A')));
        assert_eq!(Key::RightCtrl.name().as_deref(), Some("RightCtrl"));
        assert_eq!(Key::Escape.name().as_deref(), Some("Escape"));
    }

    #[test]
    fn unknown_names_are_rejected() {
        for name in [
            "", "F0", "F25", "F013", "Numpad10", "Ctrl", "Hyper", "AB", "Other",
        ] {
            assert_eq!(Key::from_name(name), None, "{name}");
        }
        assert_eq!(Key::Other(0xFF).name(), None);
    }

    /// 화면 이름 번역(`locales/ko.yml`의 `hotkey.keys.*`)은 이 모듈이 설정 파일에 쓰는 이름과 같아야 합니다.
    /// 다른 이름(별칭)이나 없는 키로 적으면 그 번역은 쓰이지 않고 설정 값이 그대로 화면에 보입니다.
    #[test]
    fn translated_key_names_are_the_names_written_to_the_config() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales/ko.yml");
        let text = std::fs::read_to_string(path).unwrap();
        let mut in_keys = false;
        let mut names = Vec::new();
        for line in text.lines() {
            if line == "  keys:" {
                in_keys = true;
                continue;
            }
            // `hotkey.keys` 아래(들여쓰기 4칸)에서 다음 항목이 나올 때까지
            if in_keys {
                match line.strip_prefix("    ") {
                    Some(entry) if !entry.starts_with([' ', '#']) => {
                        let (name, _) = entry.split_once(':').expect("이름: 문구 꼴");
                        names.push(name.to_owned());
                    }
                    _ if line.trim_start().starts_with('#') => {}
                    _ => break,
                }
            }
        }
        assert!(names.len() > 10, "번역 목록을 읽지 못했습니다: {names:?}");
        for name in names {
            let written = Key::from_name(&name).and_then(Key::name);
            assert_eq!(
                written.as_deref(),
                Some(name.as_str()),
                "'{name}'은(는) 설정 파일에 쓰는 키 이름이 아닙니다"
            );
        }
    }

    #[test]
    fn typing_keys_are_not_usable_alone() {
        assert!(Key::RightCtrl.usable_alone());
        assert!(Key::Function(13).usable_alone());
        assert!(Key::HangulMode.usable_alone());
        assert!(!Key::Letter(b'A').usable_alone());
        assert!(!Key::Space.usable_alone());
        assert!(!Key::Escape.usable_alone());
        assert!(Key::Letter(b'A').is_typing());
        assert!(!Key::Function(1).is_typing());
    }
}
