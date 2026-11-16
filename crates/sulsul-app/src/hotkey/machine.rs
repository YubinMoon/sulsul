//! 키 입력 판단 (OS와 무관).
//!
//! 키보드 후크가 키를 하나 받을 때마다 [`Machine::handle`]에 넘기면, 그 키를 다른 앱에 전달할지,
//! 녹음 단축키가 눌렸는지, 붙여넣기 단축키인지, 단축키 입력받기 결과가 나왔는지를 돌려줍니다.
//!
//! 단축키 규칙
//! - 수정 키 하나(`RightCtrl`): 다른 키 없이 혼자 눌렀다 떼면 토글, 혼자 누르고 있는 동안 녹음.
//!   `RightCtrl+C`처럼 다른 키(마우스 단추 포함)와 함께 누르면 녹음하지 않습니다. 이 키는 다른 앱에도
//!   그대로 전달해서 `Ctrl+C` 같은 조합이 평소처럼 동작합니다.
//! - 수정 키가 아닌 키 하나(`F13`)와 조합(`Ctrl+Shift+Space`): 누르면 바로 동작하고, 그 키는
//!   다른 앱에 전달하지 않습니다.
//! - 한국어 키보드는 오른쪽 Ctrl·Alt가 한자·한/영 키로 들어오기도 합니다. OS 쪽에서 이런 키를
//!   `remapped`로 표시해 오른쪽 Ctrl·Alt로 넘겨주면, 단축키일 때 원래 동작(한자 변환·입력기 전환)이
//!   일어나지 않게 막습니다.

use super::keys::{Key, Modifier};
use super::spec::HotkeySpec;
use super::{Binding, Captured};
use sulsul_core::config::HotkeyMode;

/// 단축키 후크가 알리는 일
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// 토글 모드: 녹음을 시작하거나 끝냅니다.
    Toggle,
    /// 누르는 동안 모드: 단축키를 눌렀습니다 (녹음 시작).
    Press,
    /// 누르는 동안 모드: 단축키를 뗐습니다 (녹음 끝).
    Release,
    /// 누르는 동안 모드: 단축키를 누른 채 다른 키를 눌러 녹음을 버립니다.
    Cancel,
    /// 사용자가 붙여넣기 단축키(`Ctrl+V`, `Shift+Insert`)를 눌렀습니다.
    Paste,
}

/// 후크가 받은 키 하나
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    pub key: Key,
    pub down: bool,
    /// 원래는 다른 키(한자·한/영)로 들어온 오른쪽 Ctrl·Alt. OS는 이 키를 수정 키로 보지 않습니다.
    pub remapped: bool,
    /// OS의 키 코드. 놓친 키 뗌을 가려내는 데 씁니다([`KeyProbe`]).
    pub raw: u32,
}

/// OS에 지금 키가 눌려 있는지 묻습니다. 잠금 화면 등으로 키 뗌을 받지 못해 눌린 채로 남은 키를 지우는 데 씁니다.
pub trait KeyProbe {
    fn is_down(&self, raw: u32) -> bool;
}

/// 메뉴 막대·시작 메뉴가 열리지 않게 가짜 키를 넣는 방법
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mask {
    #[default]
    None,
    /// 지금 가짜 키를 한 번 눌렀다 뗍니다 (조합을 막은 뒤 Alt·Win을 뗄 때를 대비).
    Now,
    /// 이 키 뗌을 막고, 가짜 키를 눌렀다 뗀 뒤 이 키 뗌을 다시 보냅니다 (Alt·Win·Shift 혼자 누르기).
    BeforeRelease,
}

/// 단축키 입력받기 상태 변화
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureUpdate {
    Preview(Option<String>),
    Finish(Captured),
}

/// 키 하나에 대한 판단
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verdict {
    /// 이 키를 다른 앱에 전달하지 않습니다.
    pub swallow: bool,
    pub mask: Mask,
    pub event: Option<HotkeyEvent>,
    /// 붙여넣기 단축키였는지 ([`HotkeyEvent::Paste`])
    pub paste: bool,
    pub capture: Option<CaptureUpdate>,
}

impl Verdict {
    fn swallowed() -> Self {
        Self {
            swallow: true,
            ..Self::default()
        }
    }

    fn event(event: Option<HotkeyEvent>) -> Self {
        Self {
            event,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Held {
    key: Key,
    raw: u32,
    remapped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Trigger {
    #[default]
    Idle,
    /// 수정 키 하나로 된 단축키를 누르고 있음
    Tap {
        /// 누른 뒤로 다른 입력이 없었는지
        clean: bool,
        /// 누르는 동안 모드에서 [`HotkeyEvent::Press`]를 보냈는지
        pressed: bool,
    },
    /// 키 하나(수정 키 아님)나 조합의 키를 누르고 있음 (누름을 막음)
    Hold { key: Key, pressed: bool },
}

/// 단축키 입력받기 중 지금까지 누른 키
#[derive(Debug, Default)]
struct Gesture {
    /// 누른 키를 모두 뗄 때 확정할 조합
    pending: Option<String>,
    /// 모두 뗄 때까지 누른 수정 키
    modifiers: Vec<Key>,
    /// 입력받기 중 눌러서 아직 누르고 있는 수정 키
    down: Vec<Key>,
    /// 수정 키가 아닌 키를 눌렀는지 (그러면 수정 키 하나만 뗄 때 결과로 삼지 않음)
    other: bool,
}

#[derive(Debug, Default)]
pub struct Machine {
    binding: Option<Binding>,
    held: Vec<Held>,
    /// 누름을 막은 키. 뗌도 막습니다.
    swallowed: Vec<Key>,
    trigger: Trigger,
    /// 입력받는 중이면 `Some`
    capture: Option<Gesture>,
}

impl Machine {
    pub fn set_binding(&mut self, binding: Option<Binding>) {
        if self.binding != binding {
            self.binding = binding;
            self.trigger = Trigger::Idle;
        }
    }

    /// 키 하나를 판단합니다. `capturing`은 설정 화면이 단축키를 입력받는 중인지입니다.
    pub fn handle(&mut self, input: KeyInput, capturing: bool, probe: &dyn KeyProbe) -> Verdict {
        self.sync_capture(capturing);

        let key = input.key;
        let fresh = if input.down {
            self.press(input, probe)
        } else {
            self.held.retain(|held| held.key != key);
            false
        };
        let swallow_up = !input.down && remove(&mut self.swallowed, key);

        if self.capture.is_some() {
            let mut verdict = self.capture_key(input, fresh);
            if matches!(verdict.capture, Some(CaptureUpdate::Finish(_))) {
                self.capture = None;
            }
            verdict.swallow |= swallow_up;
            return verdict;
        }

        let mut verdict = match self.binding {
            Some(binding) => self.trigger_key(&binding, input, fresh),
            None => Verdict::default(),
        };
        verdict.swallow |= swallow_up;
        if input.down && !verdict.swallow {
            verdict.paste = self.is_paste(key);
        }
        verdict
    }

    /// 마우스 단추·휠 입력. 수정 키 하나로 된 단축키를 누르고 있었다면 "다른 키와 함께 누름"으로 봅니다.
    pub fn other_input(&mut self) -> Option<HotkeyEvent> {
        if self.capture.is_some() {
            return None;
        }
        let Trigger::Tap { clean, pressed } = &mut self.trigger else {
            return None;
        };
        *clean = false;
        std::mem::take(pressed).then_some(HotkeyEvent::Cancel)
    }

    fn sync_capture(&mut self, capturing: bool) {
        if capturing == self.capture.is_some() {
            return;
        }
        self.trigger = Trigger::Idle;
        self.capture = capturing.then(Gesture::default);
    }

    /// 누른 키를 기록하고, 새로 누른 것이면(자동 반복이 아니면) `true`.
    fn press(&mut self, input: KeyInput, probe: &dyn KeyProbe) -> bool {
        let swallowed = &self.swallowed;
        // 막은 키는 OS가 눌렸다고 알지 못하므로 OS에 묻지 않습니다.
        let still_down = |held: &Held| swallowed.contains(&held.key) || probe.is_down(held.raw);
        let fresh = match self.held.iter().find(|held| held.key == input.key) {
            None => true,
            // 이미 눌려 있다고 기록된 키를 OS는 떼어져 있다고 보면, 지난번 뗌을 놓친 것입니다.
            // (OS의 키 상태는 이 키의 누름을 반영하기 전입니다.)
            Some(held) => !still_down(held),
        };
        if fresh {
            self.held
                .retain(|held| held.key != input.key && still_down(held));
            self.held.push(Held {
                key: input.key,
                raw: input.raw,
                remapped: input.remapped,
            });
        }
        fresh
    }

    fn is_held(&self, key: Key) -> bool {
        self.held.iter().any(|held| held.key == key)
    }

    /// `key` 말고 눌려 있는 키가 있는지
    fn others_held(&self, key: Key) -> bool {
        self.held.iter().any(|held| held.key != key)
    }

    /// OS가 수정 키로 보는 `modifier`가 눌려 있는지 (한자·한/영으로 들어온 키는 빼고)
    fn os_modifier_held(&self, modifier: Modifier) -> bool {
        self.held
            .iter()
            .any(|held| !held.remapped && held.key.modifier().is_some_and(|(m, _)| m == modifier))
    }

    fn is_paste(&self, key: Key) -> bool {
        let ctrl = self.os_modifier_held(Modifier::Ctrl);
        let alt = self.os_modifier_held(Modifier::Alt);
        let shift = self.os_modifier_held(Modifier::Shift);
        let win = self.os_modifier_held(Modifier::Win);
        match key {
            Key::Letter(b'V') => ctrl && !alt && !win,
            Key::Insert => shift && !ctrl && !alt && !win,
            _ => false,
        }
    }

    fn trigger_key(&mut self, binding: &Binding, input: KeyInput, fresh: bool) -> Verdict {
        match binding.spec {
            HotkeySpec::Single(key) if key.is_modifier() => {
                self.modifier_tap(key, binding.mode, input, fresh)
            }
            HotkeySpec::Single(key) => {
                // 수정 키가 아닌 키는 다른 키를 누르고 있지 않을 때만 단축키로 받습니다.
                let armed = !self.others_held(key);
                self.held_key(key, armed, false, binding.mode, input, fresh)
            }
            HotkeySpec::Combo { mods, key } => {
                let mods_ok = mods.matches(|modifier, side| self.is_held(modifier.key(side)));
                let others = self
                    .held
                    .iter()
                    .any(|held| held.key != key && !held.key.is_modifier());
                // 단축키를 막으면 Alt·Win만 눌렀다 뗀 것처럼 보여 메뉴·시작 메뉴가 열리므로 가짜 키를 넣습니다.
                let mask =
                    self.os_modifier_held(Modifier::Alt) || self.os_modifier_held(Modifier::Win);
                self.held_key(key, mods_ok && !others, mask, binding.mode, input, fresh)
            }
        }
    }

    /// 수정 키 하나로 된 단축키
    fn modifier_tap(
        &mut self,
        key: Key,
        mode: HotkeyMode,
        input: KeyInput,
        fresh: bool,
    ) -> Verdict {
        if input.key != key {
            if input.down && fresh {
                return Verdict::event(self.other_input());
            }
            return Verdict::default();
        }

        // 한자·한/영으로 들어온 키는 원래 동작(한자 변환·입력기 전환)을 막습니다.
        let swallow = input.remapped;
        if input.down {
            if fresh {
                let clean = !self.others_held(key);
                let pressed = mode == HotkeyMode::PushToTalk && clean;
                self.trigger = Trigger::Tap { clean, pressed };
                if swallow {
                    self.swallowed.push(key);
                }
                return Verdict {
                    swallow,
                    event: pressed.then_some(HotkeyEvent::Press),
                    ..Verdict::default()
                };
            }
            return Verdict {
                swallow,
                ..Verdict::default()
            };
        }

        let event = match std::mem::take(&mut self.trigger) {
            Trigger::Tap { clean, pressed } => match mode {
                HotkeyMode::Toggle => clean.then_some(HotkeyEvent::Toggle),
                HotkeyMode::PushToTalk => pressed.then_some(HotkeyEvent::Release),
            },
            _ => None,
        };
        let masked = key
            .modifier()
            .is_some_and(|(modifier, _)| modifier != Modifier::Ctrl);
        Verdict {
            mask: if event.is_some() && masked && !input.remapped {
                Mask::BeforeRelease
            } else {
                Mask::None
            },
            event,
            ..Verdict::default()
        }
    }

    /// 누름을 막는 단축키 (수정 키가 아닌 키 하나, 조합)
    fn held_key(
        &mut self,
        key: Key,
        armed: bool,
        mask: bool,
        mode: HotkeyMode,
        input: KeyInput,
        fresh: bool,
    ) -> Verdict {
        if input.key != key {
            return Verdict::default();
        }
        if !input.down {
            let event = match std::mem::take(&mut self.trigger) {
                Trigger::Hold { key: held, pressed } if held == key && pressed => {
                    Some(HotkeyEvent::Release)
                }
                _ => None,
            };
            return Verdict::event(event);
        }
        if !fresh {
            // 자동 반복: 누름을 막았던 키면 반복도 막습니다.
            return Verdict {
                swallow: self.swallowed.contains(&key),
                ..Verdict::default()
            };
        }
        if !armed {
            return Verdict::default();
        }
        self.swallowed.push(key);
        let event = match mode {
            HotkeyMode::Toggle => HotkeyEvent::Toggle,
            HotkeyMode::PushToTalk => HotkeyEvent::Press,
        };
        self.trigger = Trigger::Hold {
            key,
            pressed: mode == HotkeyMode::PushToTalk,
        };
        Verdict {
            swallow: true,
            mask: if mask { Mask::Now } else { Mask::None },
            event: Some(event),
            ..Verdict::default()
        }
    }

    /// 단축키 입력받기 중: 모든 키를 막고, 누른 키를 모두 떼면 결과를 냅니다.
    fn capture_key(&mut self, input: KeyInput, fresh: bool) -> Verdict {
        let key = input.key;
        if input.down {
            if !self.swallowed.contains(&key) {
                self.swallowed.push(key);
            }
            if !fresh {
                return Verdict::swallowed();
            }
            if key == Key::Escape {
                return capture_verdict(CaptureUpdate::Finish(Captured::Cancelled));
            }
            let held_modifiers: Vec<Key> = self
                .held
                .iter()
                .map(|held| held.key)
                .filter(|held| held.is_modifier())
                .collect();
            let gesture = self.capture.get_or_insert_with(Gesture::default);
            if key.is_modifier() {
                push_unique(&mut gesture.modifiers, key);
                push_unique(&mut gesture.down, key);
                return capture_verdict(CaptureUpdate::Preview(Some(modifier_preview(
                    &held_modifiers,
                ))));
            }
            if !gesture.down.is_empty() {
                gesture.other = true;
            }
            return match captured_spec(&held_modifiers, key) {
                Some(spec) => {
                    gesture.pending = Some(spec.clone());
                    capture_verdict(CaptureUpdate::Preview(Some(spec)))
                }
                // 쓸 수 없는 키(수정 키 없는 글자 등)는 무시하고 계속 입력받습니다.
                None => Verdict::swallowed(),
            };
        }

        let Some(gesture) = self.capture.as_mut() else {
            return Verdict::default();
        };
        if gesture.pending.is_some() {
            remove(&mut gesture.down, key);
            if self.held.is_empty() {
                return capture_verdict(CaptureUpdate::Finish(Captured::Key(
                    gesture.pending.take().unwrap_or_default(),
                )));
            }
            return Verdict::swallowed();
        }
        if !key.is_modifier() || !remove(&mut gesture.down, key) {
            return Verdict::default();
        }
        if !gesture.down.is_empty() {
            let held_modifiers: Vec<Key> = self
                .held
                .iter()
                .map(|held| held.key)
                .filter(|held| held.is_modifier())
                .collect();
            return capture_verdict(CaptureUpdate::Preview(Some(modifier_preview(
                &held_modifiers,
            ))));
        }
        // 입력받기 중 누른 수정 키를 모두 뗐습니다. 수정 키 하나만 눌렀다 뗐으면 그 키가 결과입니다.
        let single = match (gesture.modifiers.as_slice(), gesture.other) {
            ([only], false) => Some(*only),
            _ => None,
        };
        gesture.modifiers.clear();
        gesture.other = false;
        match single.and_then(Key::name) {
            Some(name) => capture_verdict(CaptureUpdate::Finish(Captured::Key(name.into_owned()))),
            None => capture_verdict(CaptureUpdate::Preview(None)),
        }
    }
}

fn capture_verdict(update: CaptureUpdate) -> Verdict {
    Verdict {
        swallow: true,
        capture: Some(update),
        ..Verdict::default()
    }
}

fn remove(keys: &mut Vec<Key>, key: Key) -> bool {
    let before = keys.len();
    keys.retain(|k| *k != key);
    keys.len() != before
}

fn push_unique(keys: &mut Vec<Key>, key: Key) {
    if !keys.contains(&key) {
        keys.push(key);
    }
}

/// 누르고 있는 수정 키 미리보기. 하나면 방향을 붙인 이름(`RightCtrl`), 여럿이면 종류만(`Ctrl+Shift`).
fn modifier_preview(modifiers: &[Key]) -> String {
    match modifiers {
        [only] => only.name().unwrap_or_default().into_owned(),
        _ => modifier_kinds(modifiers),
    }
}

/// 수정 키를 누른 채 `key`를 눌렀을 때의 단축키 값. 단축키로 쓸 수 없으면 `None`.
fn captured_spec(modifiers: &[Key], key: Key) -> Option<String> {
    let name = key.name()?;
    let text = if modifiers.is_empty() {
        name.into_owned()
    } else {
        format!("{}+{name}", modifier_kinds(modifiers))
    };
    // 형식 검사(혼자 쓸 수 없는 키, Shift+글자 등)는 설정 값을 읽을 때와 같은 규칙을 씁니다.
    HotkeySpec::parse(&text).ok().map(|spec| spec.to_string())
}

/// 수정 키 종류만 (`Ctrl+Shift`). 하나여도 방향을 붙이지 않습니다.
fn modifier_kinds(modifiers: &[Key]) -> String {
    Modifier::ALL
        .into_iter()
        .filter(|modifier| {
            modifiers
                .iter()
                .any(|key| key.modifier().is_some_and(|(m, _)| m == *modifier))
        })
        .map(Modifier::name)
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 기록된 키가 모두 눌려 있다고 답합니다 (놓친 뗌 없음).
    struct AllDown;
    impl KeyProbe for AllDown {
        fn is_down(&self, _raw: u32) -> bool {
            true
        }
    }

    /// 모든 키가 떼어져 있다고 답합니다.
    struct AllUp;
    impl KeyProbe for AllUp {
        fn is_down(&self, _raw: u32) -> bool {
            false
        }
    }

    fn machine(spec: &str, mode: HotkeyMode) -> Machine {
        let mut machine = Machine::default();
        machine.set_binding(Some(Binding {
            spec: HotkeySpec::parse(spec).unwrap(),
            mode,
        }));
        machine
    }

    fn input(key: Key, down: bool) -> KeyInput {
        KeyInput {
            key,
            down,
            remapped: false,
            raw: 0,
        }
    }

    fn down(machine: &mut Machine, key: Key) -> Verdict {
        machine.handle(input(key, true), false, &AllDown)
    }

    fn up(machine: &mut Machine, key: Key) -> Verdict {
        machine.handle(input(key, false), false, &AllDown)
    }

    fn capture_down(machine: &mut Machine, key: Key) -> Verdict {
        machine.handle(input(key, true), true, &AllDown)
    }

    fn capture_up(machine: &mut Machine, key: Key) -> Verdict {
        machine.handle(input(key, false), true, &AllDown)
    }

    const C: Key = Key::Letter(b'C');
    const V: Key = Key::Letter(b'V');

    #[test]
    fn modifier_tap_toggles_and_passes_the_key_through() {
        let mut m = machine("RightCtrl", HotkeyMode::Toggle);
        let pressed = down(&mut m, Key::RightCtrl);
        assert_eq!(
            pressed,
            Verdict::default(),
            "누를 때는 아무 일도 없고 키는 전달됨"
        );
        // 자동 반복
        assert_eq!(down(&mut m, Key::RightCtrl), Verdict::default());
        let released = up(&mut m, Key::RightCtrl);
        assert_eq!(released.event, Some(HotkeyEvent::Toggle));
        assert!(!released.swallow);
        assert_eq!(released.mask, Mask::None, "Ctrl은 가짜 키가 필요 없음");
    }

    #[test]
    fn modifier_with_other_key_does_not_toggle() {
        let mut m = machine("RightCtrl", HotkeyMode::Toggle);
        down(&mut m, Key::RightCtrl);
        down(&mut m, C);
        up(&mut m, C);
        assert_eq!(
            up(&mut m, Key::RightCtrl).event,
            None,
            "오른쪽 Ctrl+C는 녹음하지 않음"
        );

        // 다른 키를 누른 채 단축키를 눌러도 녹음하지 않습니다.
        down(&mut m, Key::LeftShift);
        down(&mut m, Key::RightCtrl);
        up(&mut m, Key::RightCtrl);
        up(&mut m, Key::LeftShift);
        assert_eq!(m.trigger, Trigger::Idle);

        // 마우스 단추도 다른 입력입니다.
        down(&mut m, Key::RightCtrl);
        assert_eq!(m.other_input(), None);
        assert_eq!(up(&mut m, Key::RightCtrl).event, None);

        // 그 뒤 혼자 누르면 다시 동작합니다.
        down(&mut m, Key::RightCtrl);
        assert_eq!(up(&mut m, Key::RightCtrl).event, Some(HotkeyEvent::Toggle));
    }

    #[test]
    fn push_to_talk_with_modifier_presses_releases_and_cancels() {
        let mut m = machine("RightCtrl", HotkeyMode::PushToTalk);
        assert_eq!(down(&mut m, Key::RightCtrl).event, Some(HotkeyEvent::Press));
        assert_eq!(down(&mut m, Key::RightCtrl).event, None, "자동 반복");
        assert_eq!(up(&mut m, Key::RightCtrl).event, Some(HotkeyEvent::Release));

        assert_eq!(down(&mut m, Key::RightCtrl).event, Some(HotkeyEvent::Press));
        assert_eq!(down(&mut m, C).event, Some(HotkeyEvent::Cancel));
        up(&mut m, C);
        assert_eq!(
            up(&mut m, Key::RightCtrl).event,
            None,
            "버린 뒤 떼면 아무 일 없음"
        );

        assert_eq!(down(&mut m, Key::RightCtrl).event, Some(HotkeyEvent::Press));
        assert_eq!(m.other_input(), Some(HotkeyEvent::Cancel), "마우스 단추");
        assert_eq!(up(&mut m, Key::RightCtrl).event, None);
    }

    #[test]
    fn alt_win_shift_taps_are_masked() {
        for (spec, key) in [
            ("RightAlt", Key::RightAlt),
            ("LeftWin", Key::LeftWin),
            ("RightShift", Key::RightShift),
        ] {
            let mut m = machine(spec, HotkeyMode::Toggle);
            down(&mut m, key);
            let released = up(&mut m, key);
            assert_eq!(released.event, Some(HotkeyEvent::Toggle), "{spec}");
            assert_eq!(released.mask, Mask::BeforeRelease, "{spec}");
        }
        // 동작하지 않은 뗌에는 가짜 키가 필요 없습니다.
        let mut m = machine("RightAlt", HotkeyMode::Toggle);
        down(&mut m, Key::RightAlt);
        down(&mut m, Key::Tab);
        up(&mut m, Key::Tab);
        assert_eq!(up(&mut m, Key::RightAlt).mask, Mask::None);
    }

    #[test]
    fn remapped_right_ctrl_works_and_is_swallowed() {
        // 한국어 키보드: 오른쪽 Ctrl이 한자 키로 들어옵니다.
        let mut m = machine("RightCtrl", HotkeyMode::Toggle);
        let hanja = KeyInput {
            key: Key::RightCtrl,
            down: true,
            remapped: true,
            raw: 0x19,
        };
        let pressed = m.handle(hanja, false, &AllDown);
        assert!(pressed.swallow, "한자 변환이 일어나지 않게 막음");
        let released = m.handle(
            KeyInput {
                down: false,
                ..hanja
            },
            false,
            &AllDown,
        );
        assert!(released.swallow);
        assert_eq!(released.event, Some(HotkeyEvent::Toggle));
        assert_eq!(released.mask, Mask::None);
    }

    #[test]
    fn single_key_is_swallowed_and_triggers_on_press() {
        let mut m = machine("F13", HotkeyMode::Toggle);
        let pressed = down(&mut m, Key::Function(13));
        assert!(pressed.swallow);
        assert_eq!(pressed.event, Some(HotkeyEvent::Toggle));
        let repeat = down(&mut m, Key::Function(13));
        assert!(repeat.swallow);
        assert_eq!(repeat.event, None, "자동 반복으로 다시 토글하지 않음");
        let released = up(&mut m, Key::Function(13));
        assert!(released.swallow);
        assert_eq!(released.event, None);

        // 다른 키와 함께 누르면 전달만 합니다.
        down(&mut m, Key::LeftCtrl);
        let combo = down(&mut m, Key::Function(13));
        assert_eq!(combo, Verdict::default());
        assert!(!up(&mut m, Key::Function(13)).swallow);
    }

    #[test]
    fn single_key_push_to_talk() {
        let mut m = machine("F13", HotkeyMode::PushToTalk);
        assert_eq!(
            down(&mut m, Key::Function(13)).event,
            Some(HotkeyEvent::Press)
        );
        assert_eq!(
            up(&mut m, Key::Function(13)).event,
            Some(HotkeyEvent::Release)
        );
    }

    #[test]
    fn combination_requires_exact_modifiers() {
        let mut m = machine("Ctrl+Shift+Space", HotkeyMode::Toggle);
        down(&mut m, Key::LeftCtrl);
        down(&mut m, Key::RightShift);
        let pressed = down(&mut m, Key::Space);
        assert!(pressed.swallow);
        assert_eq!(pressed.event, Some(HotkeyEvent::Toggle));
        assert_eq!(pressed.mask, Mask::None);
        assert!(up(&mut m, Key::Space).swallow, "막은 누름의 뗌도 막음");
        up(&mut m, Key::RightShift);

        // Shift가 빠지면 평소처럼 전달합니다.
        let plain = down(&mut m, Key::Space);
        assert_eq!(plain, Verdict::default());
        up(&mut m, Key::Space);

        // 조합에 없는 Alt가 함께 눌려 있어도 안 됩니다.
        down(&mut m, Key::LeftShift);
        down(&mut m, Key::LeftAlt);
        assert_eq!(down(&mut m, Key::Space).event, None);
    }

    #[test]
    fn combination_with_alt_or_win_is_masked() {
        let mut m = machine("Alt+K", HotkeyMode::PushToTalk);
        down(&mut m, Key::LeftAlt);
        let pressed = down(&mut m, Key::Letter(b'K'));
        assert_eq!(pressed.event, Some(HotkeyEvent::Press));
        assert_eq!(pressed.mask, Mask::Now);
        let released = up(&mut m, Key::Letter(b'K'));
        assert_eq!(released.event, Some(HotkeyEvent::Release));
        assert!(released.swallow);
    }

    #[test]
    fn paste_shortcuts_are_detected() {
        let mut m = machine("RightCtrl", HotkeyMode::Toggle);
        down(&mut m, Key::LeftCtrl);
        let paste = down(&mut m, V);
        assert!(paste.paste);
        assert!(!paste.swallow, "붙여넣기는 막지 않음");
        assert!(down(&mut m, V).paste, "자동 반복도 붙여넣기");
        up(&mut m, V);
        down(&mut m, Key::LeftShift);
        assert!(down(&mut m, V).paste, "Ctrl+Shift+V");
        up(&mut m, V);
        up(&mut m, Key::LeftShift);
        down(&mut m, Key::LeftAlt);
        assert!(!down(&mut m, V).paste, "Ctrl+Alt+V는 아님");
        up(&mut m, V);
        up(&mut m, Key::LeftAlt);
        up(&mut m, Key::LeftCtrl);

        assert!(!down(&mut m, V).paste, "V만 누름");
        up(&mut m, V);
        down(&mut m, Key::RightShift);
        assert!(down(&mut m, Key::Insert).paste, "Shift+Insert");
        up(&mut m, Key::Insert);
        up(&mut m, Key::RightShift);

        // 단축키(오른쪽 Ctrl)로 Ctrl+V를 해도 붙여넣기이고 녹음은 하지 않습니다.
        down(&mut m, Key::RightCtrl);
        assert!(down(&mut m, V).paste);
        up(&mut m, V);
        assert_eq!(up(&mut m, Key::RightCtrl).event, None);

        // 한자 키로 들어온 오른쪽 Ctrl은 OS에는 Ctrl이 아니라서 붙여넣기가 아닙니다.
        let hanja = KeyInput {
            key: Key::RightCtrl,
            down: true,
            remapped: true,
            raw: 0x19,
        };
        let mut plain = Machine::default();
        plain.handle(hanja, false, &AllDown);
        assert!(!plain.handle(input(V, true), false, &AllDown).paste);
    }

    #[test]
    fn missed_releases_are_forgotten() {
        let mut m = machine("RightCtrl", HotkeyMode::Toggle);
        // 잠금 화면(Win+L)으로 Win 뗌을 놓쳤습니다.
        down(&mut m, Key::LeftWin);
        // OS는 Win이 떼어져 있다고 알려 줍니다.
        m.handle(input(Key::RightCtrl, true), false, &AllUp);
        assert_eq!(
            up(&mut m, Key::RightCtrl).event,
            Some(HotkeyEvent::Toggle),
            "눌린 채 남은 Win 때문에 단축키가 막히면 안 됨"
        );
    }

    #[test]
    fn no_binding_only_detects_paste() {
        let mut m = Machine::default();
        assert_eq!(down(&mut m, Key::RightCtrl), Verdict::default());
        let paste = down(&mut m, V);
        assert!(paste.paste);
        assert_eq!(paste.event, None);
        up(&mut m, V);
        assert_eq!(up(&mut m, Key::RightCtrl), Verdict::default());
    }

    #[test]
    fn capture_single_modifier_on_release() {
        let mut m = machine("F13", HotkeyMode::Toggle);
        let pressed = capture_down(&mut m, Key::RightCtrl);
        assert!(pressed.swallow);
        assert_eq!(
            pressed.capture,
            Some(CaptureUpdate::Preview(Some("RightCtrl".into())))
        );
        let released = capture_up(&mut m, Key::RightCtrl);
        assert!(released.swallow);
        assert_eq!(
            released.capture,
            Some(CaptureUpdate::Finish(Captured::Key("RightCtrl".into())))
        );
    }

    #[test]
    fn capture_combination_after_all_keys_are_released() {
        let mut m = Machine::default();
        capture_down(&mut m, Key::LeftCtrl);
        let preview = capture_down(&mut m, Key::RightShift);
        assert_eq!(
            preview.capture,
            Some(CaptureUpdate::Preview(Some("Ctrl+Shift".into())))
        );
        let pressed = capture_down(&mut m, Key::Space);
        assert!(pressed.swallow);
        assert_eq!(
            pressed.capture,
            Some(CaptureUpdate::Preview(Some("Ctrl+Shift+Space".into())))
        );
        for key in [Key::Space, Key::RightShift] {
            let released = capture_up(&mut m, key);
            assert!(released.swallow);
            assert!(!matches!(released.capture, Some(CaptureUpdate::Finish(_))));
        }
        let done = capture_up(&mut m, Key::LeftCtrl);
        assert!(done.swallow);
        assert_eq!(
            done.capture,
            Some(CaptureUpdate::Finish(Captured::Key(
                "Ctrl+Shift+Space".into()
            )))
        );
        assert!(!up(&mut m, Key::LeftCtrl).swallow, "한 번만");

        // 다음 입력받기에 지난 조합의 수정 키가 남지 않습니다.
        capture_down(&mut m, Key::RightCtrl);
        assert_eq!(
            capture_up(&mut m, Key::RightCtrl).capture,
            Some(CaptureUpdate::Finish(Captured::Key("RightCtrl".into())))
        );
    }

    #[test]
    fn capture_rejects_typing_keys_and_cancels_on_escape() {
        let mut m = Machine::default();
        let typed = capture_down(&mut m, Key::Letter(b'A'));
        assert!(typed.swallow);
        assert_eq!(typed.capture, None, "글자 하나는 단축키가 아님");
        capture_up(&mut m, Key::Letter(b'A'));

        capture_down(&mut m, Key::LeftShift);
        assert_eq!(
            capture_down(&mut m, Key::Letter(b'A')).capture,
            None,
            "Shift+A도 아님"
        );
        capture_up(&mut m, Key::Letter(b'A'));
        // 글자를 함께 눌렀으므로 Shift만 뗐다고 Shift가 결과가 되지는 않습니다.
        assert_eq!(
            capture_up(&mut m, Key::LeftShift).capture,
            Some(CaptureUpdate::Preview(None))
        );

        assert_eq!(
            capture_down(&mut m, Key::Function(13)).capture,
            Some(CaptureUpdate::Preview(Some("F13".into())))
        );
        assert_eq!(
            capture_up(&mut m, Key::Function(13)).capture,
            Some(CaptureUpdate::Finish(Captured::Key("F13".into())))
        );
        let mut m = Machine::default();
        assert_eq!(
            capture_down(&mut m, Key::Escape).capture,
            Some(CaptureUpdate::Finish(Captured::Cancelled))
        );
    }

    #[test]
    fn capture_of_two_modifiers_without_key_gives_nothing() {
        let mut m = Machine::default();
        capture_down(&mut m, Key::LeftCtrl);
        capture_down(&mut m, Key::LeftAlt);
        assert_eq!(
            capture_up(&mut m, Key::LeftAlt).capture,
            Some(CaptureUpdate::Preview(Some("LeftCtrl".into())))
        );
        assert_eq!(
            capture_up(&mut m, Key::LeftCtrl).capture,
            Some(CaptureUpdate::Preview(None))
        );
    }

    /// 입력란에 포커스를 두고 Space·Enter로 입력받기를 시작하면, 그 키를 떼는 것이 입력받기가 시작된 뒤에 옵니다.
    /// 시작 전에 누른 키의 뗌은 결과가 되지 않고 그대로 전달돼야 합니다.
    #[test]
    fn capture_ignores_the_release_of_keys_pressed_before_it_started() {
        let mut m = machine("RightCtrl", HotkeyMode::Toggle);
        down(&mut m, Key::Space);
        let released = capture_up(&mut m, Key::Space);
        assert_eq!(
            released.capture, None,
            "시작 전에 누른 키의 뗌은 결과가 아님"
        );
        assert!(!released.swallow, "누름을 전달했으니 뗌도 전달해야 함");

        // 수정 키도 마찬가지입니다. 입력받기 중에 누른 키만 결과가 됩니다.
        let mut m = machine("RightCtrl", HotkeyMode::Toggle);
        down(&mut m, Key::LeftShift);
        let released = capture_up(&mut m, Key::LeftShift);
        assert_eq!(released.capture, None);
        assert!(!released.swallow);
        capture_down(&mut m, Key::RightAlt);
        assert_eq!(
            capture_up(&mut m, Key::RightAlt).capture,
            Some(CaptureUpdate::Finish(Captured::Key("RightAlt".into())))
        );
    }

    #[test]
    fn capture_suspends_the_hotkey() {
        let mut m = machine("RightCtrl", HotkeyMode::PushToTalk);
        assert_eq!(capture_down(&mut m, Key::Function(1)).event, None);
        // 입력받기가 끝나면 다시 단축키로 동작합니다.
        up(&mut m, Key::Function(1));
        assert_eq!(down(&mut m, Key::RightCtrl).event, Some(HotkeyEvent::Press));
    }
}
