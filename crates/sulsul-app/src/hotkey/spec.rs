//! 설정 파일의 단축키 값.
//!
//! 형식은 두 가지입니다.
//! - 키 하나: `RightCtrl`, `F13`, `ScrollLock` 등. 수정 키는 왼쪽·오른쪽을 가린 이름을 씁니다.
//!   수정 키 하나만 잡으면 다른 키 없이 혼자 눌렀다 뗄 때만 동작합니다.
//! - 조합: `Ctrl+Shift+Space`. 수정 키는 `Ctrl`, `Alt`, `Shift`, `Win` 순서로 쓰고 마지막에 키 하나를 둡니다.
//!   수정 키에 방향을 붙이면(`RightCtrl+Space`) 그쪽 키를 눌러야 합니다.
//!
//! 이름은 대소문자를 가리지 않고 몇 가지 다른 이름(`RCtrl`, `Esc` 등)도 받아들이며,
//! [`HotkeySpec`]을 문자열로 바꾸면 기본 이름과 순서로 씁니다.

use std::fmt;

use super::keys::{Key, Modifier, Side};

/// 조합에서 수정 키를 어느 쪽으로 눌러야 하는지
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideReq {
    Either,
    Only(Side),
}

/// 조합의 수정 키들 ([`Modifier::ALL`] 순서)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods([Option<SideReq>; 4]);

impl Mods {
    fn index(modifier: Modifier) -> usize {
        match modifier {
            Modifier::Ctrl => 0,
            Modifier::Alt => 1,
            Modifier::Shift => 2,
            Modifier::Win => 3,
        }
    }

    pub fn get(&self, modifier: Modifier) -> Option<SideReq> {
        self.0[Self::index(modifier)]
    }

    fn set(&mut self, modifier: Modifier, req: SideReq) -> bool {
        let slot = &mut self.0[Self::index(modifier)];
        if slot.is_some() {
            return false;
        }
        *slot = Some(req);
        true
    }

    /// `held(수정 키, 방향)`이 지금 눌린 수정 키를 알려 줄 때 이 조합의 수정 키와 정확히 맞는지.
    /// 조합에 없는 수정 키가 눌려 있으면 맞지 않습니다.
    pub fn matches(&self, held: impl Fn(Modifier, Side) -> bool) -> bool {
        Modifier::ALL.into_iter().all(|modifier| {
            let left = held(modifier, Side::Left);
            let right = held(modifier, Side::Right);
            match self.get(modifier) {
                None => !left && !right,
                Some(SideReq::Either) => left || right,
                Some(SideReq::Only(Side::Left)) => left,
                Some(SideReq::Only(Side::Right)) => right,
            }
        })
    }

    pub fn contains(&self, modifier: Modifier) -> bool {
        self.get(modifier).is_some()
    }

    /// 수정 키가 Shift 하나뿐인지
    fn only_shift(&self) -> bool {
        Modifier::ALL
            .into_iter()
            .all(|modifier| self.contains(modifier) == (modifier == Modifier::Shift))
    }
}

/// 녹음 단축키
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeySpec {
    /// 키 하나. 수정 키면 혼자 눌렀다 뗄 때(누르는 동안 모드는 혼자 누르고 있는 동안) 동작합니다.
    Single(Key),
    /// 수정 키를 누른 채 누르는 키
    Combo { mods: Mods, key: Key },
}

/// 단축키 값을 읽지 못한 이유 (로그용)
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    #[error("the hotkey is empty")]
    Empty,
    #[error("unknown key '{0}'")]
    UnknownKey(String),
    #[error("modifier '{0}' appears more than once")]
    DuplicateModifier(String),
    #[error("a hotkey can have only one non-modifier key")]
    MultipleKeys,
    #[error("a combination needs a non-modifier key")]
    NoKey,
    #[error("a single modifier key must name its side (e.g. RightCtrl), not '{0}'")]
    ModifierWithoutSide(String),
    #[error("'{0}' is used for typing and cannot be a hotkey on its own")]
    TypingKey(String),
}

impl HotkeySpec {
    pub fn parse(text: &str) -> Result<Self, SpecError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(SpecError::Empty);
        }
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        if parts.iter().any(|part| part.is_empty()) {
            return Err(SpecError::UnknownKey(text.to_owned()));
        }

        if let [part] = parts.as_slice() {
            if Modifier::from_name(part).is_some() {
                return Err(SpecError::ModifierWithoutSide((*part).to_owned()));
            }
            let key = Key::from_name(part).ok_or_else(|| SpecError::UnknownKey((*part).into()))?;
            if !key.usable_alone() {
                return Err(SpecError::TypingKey((*part).to_owned()));
            }
            return Ok(Self::Single(key));
        }

        let mut mods = Mods::default();
        let mut main = None;
        for part in parts {
            let (modifier, req) = if let Some(modifier) = Modifier::from_name(part) {
                (modifier, SideReq::Either)
            } else {
                let key = Key::from_name(part).ok_or_else(|| SpecError::UnknownKey(part.into()))?;
                match key.modifier() {
                    Some((modifier, side)) => (modifier, SideReq::Only(side)),
                    None => {
                        if main.replace(key).is_some() {
                            return Err(SpecError::MultipleKeys);
                        }
                        continue;
                    }
                }
            };
            if !mods.set(modifier, req) {
                return Err(SpecError::DuplicateModifier(part.to_owned()));
            }
        }
        let key = main.ok_or(SpecError::NoKey)?;
        if mods.only_shift() && key.is_typing() {
            return Err(SpecError::TypingKey(text.to_owned()));
        }
        Ok(Self::Combo { mods, key })
    }
}

impl fmt::Display for HotkeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key_name = |key: Key| key.name().unwrap_or_default();
        match self {
            Self::Single(key) => f.write_str(&key_name(*key)),
            Self::Combo { mods, key } => {
                for modifier in Modifier::ALL {
                    match mods.get(modifier) {
                        None => continue,
                        Some(SideReq::Either) => f.write_str(modifier.name())?,
                        Some(SideReq::Only(side)) => f.write_str(&key_name(modifier.key(side)))?,
                    }
                    f.write_str("+")?;
                }
                f.write_str(&key_name(*key))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(text: &str) -> String {
        HotkeySpec::parse(text)
            .unwrap_or_else(|error| panic!("'{text}'을(를) 읽지 못했습니다: {error}"))
            .to_string()
    }

    #[test]
    fn single_keys_and_combinations_are_parsed() {
        assert_eq!(
            HotkeySpec::parse("RightCtrl"),
            Ok(HotkeySpec::Single(Key::RightCtrl))
        );
        assert_eq!(
            HotkeySpec::parse("F13"),
            Ok(HotkeySpec::Single(Key::Function(13)))
        );
        assert_eq!(canonical("F13"), "F13");
        assert_eq!(canonical("Ctrl+Shift+Space"), "Ctrl+Shift+Space");
        assert_eq!(canonical("HangulMode"), "HangulMode");
    }

    #[test]
    fn output_uses_canonical_names_and_order() {
        assert_eq!(canonical(" shift + ctrl + space "), "Ctrl+Shift+Space");
        assert_eq!(canonical("Win+Alt+K"), "Alt+Win+K");
        assert_eq!(canonical("rctrl"), "RightCtrl");
        assert_eq!(canonical("RightCtrl+Space"), "RightCtrl+Space");
        assert_eq!(canonical("Control+Esc"), "Ctrl+Escape");
        assert_eq!(canonical("Shift+F5"), "Shift+F5");
    }

    #[test]
    fn invalid_values_are_rejected() {
        let cases = [
            ("", SpecError::Empty),
            ("  ", SpecError::Empty),
            ("Ctrl", SpecError::ModifierWithoutSide("Ctrl".into())),
            ("A", SpecError::TypingKey("A".into())),
            ("Space", SpecError::TypingKey("Space".into())),
            ("Shift+A", SpecError::TypingKey("Shift+A".into())),
            ("Ctrl+Shift", SpecError::NoKey),
            ("Ctrl+A+B", SpecError::MultipleKeys),
            ("Ctrl+Ctrl+A", SpecError::DuplicateModifier("Ctrl".into())),
            (
                "Ctrl+RightCtrl+A",
                SpecError::DuplicateModifier("RightCtrl".into()),
            ),
            ("Hyper", SpecError::UnknownKey("Hyper".into())),
            ("Ctrl++", SpecError::UnknownKey("Ctrl++".into())),
        ];
        for (text, expected) in cases {
            assert_eq!(HotkeySpec::parse(text), Err(expected), "'{text}'");
        }
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let HotkeySpec::Combo { mods, .. } = HotkeySpec::parse("Ctrl+Shift+Space").unwrap() else {
            panic!("조합이어야 합니다");
        };
        let held = |keys: &'static [Key]| {
            move |modifier: Modifier, side: Side| keys.contains(&modifier.key(side))
        };
        assert!(mods.matches(held(&[Key::LeftCtrl, Key::RightShift])));
        assert!(mods.matches(held(&[Key::RightCtrl, Key::LeftShift])));
        assert!(!mods.matches(held(&[Key::LeftCtrl])), "Shift가 빠짐");
        assert!(
            !mods.matches(held(&[Key::LeftCtrl, Key::LeftShift, Key::LeftAlt])),
            "조합에 없는 Alt가 눌림"
        );

        let HotkeySpec::Combo { mods, .. } = HotkeySpec::parse("RightCtrl+Space").unwrap() else {
            panic!("조합이어야 합니다");
        };
        assert!(mods.matches(held(&[Key::RightCtrl])));
        assert!(
            !mods.matches(held(&[Key::LeftCtrl])),
            "왼쪽 Ctrl은 맞지 않음"
        );
    }
}
