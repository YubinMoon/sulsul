//! 받아쓴 글을 클립보드에 넣고, 붙여넣은 뒤 원래 내용으로 되돌리기.
//!
//! 클립보드에 복사할 때([`PasteGuard::copy`]) 되돌리기를 켰으면
//! 1. 받아쓴 글을 넣기 전에 지금 클립보드 내용을 통째로 떠 둡니다([`Clipboard::snapshot`]).
//! 2. 붙여넣기 단축키를 감지하면([`PasteGuard::pasted`]) 붙여넣는 앱이 클립보드를 다 읽을 때까지
//!    잠깐([`RESTORE_DELAY`]) 기다린 뒤 떠 둔 내용으로 되돌립니다.
//! 3. 그 사이 사용자가 다른 것을 복사했으면(클립보드 번호가 바뀜) 되돌리지 않습니다.
//! 4. 앱을 끝낼 때 아직 붙여넣지 않았으면 원래 내용으로 되돌립니다([`PasteGuard::restore_now`]).
//!
//! 되돌리기를 껐으면 글만 넣고 원래 내용은 떠 두지 않습니다.
//!
//! 커서 위치에 넣을 때는 클립보드를 잠깐 빌립니다([`PasteGuard::lend`]). 원래 내용을 떠 두고 글을 넣은 뒤,
//! 부르는 쪽이 붙여넣기 키를 보내고 [`PasteGuard::pasted`]를 부르면 위와 같이 되돌립니다. 빌려 넣은 글은
//! 클립보드 기록(Win+V)에 남기지 않습니다.
//!
//! 클립보드를 직접 다루는 부분은 [`Clipboard`]로 떼어 두어, 규칙을 실제 클립보드 없이 시험합니다.

use std::time::{Duration, Instant};

/// 붙여넣기 단축키를 누른 뒤 원래 내용으로 되돌리기까지 기다리는 시간.
/// 붙여넣는 앱(원격 데스크톱 등 느린 앱 포함)이 클립보드를 다 읽을 때까지 기다립니다.
pub const RESTORE_DELAY: Duration = Duration::from_millis(700);

/// 클립보드 접근
pub trait Clipboard {
    /// 떠 둔 클립보드 내용
    type Snapshot;

    /// 클립보드 내용이 바뀔 때마다 달라지는 번호
    fn sequence(&self) -> u32;
    /// 지금 내용을 통째로 떠 둡니다 (글뿐 아니라 그림·파일 등 읽을 수 있는 형식 모두).
    fn snapshot(&mut self) -> Result<Self::Snapshot, ClipboardError>;
    /// 떠 둔 내용으로 바꿉니다.
    fn restore(&mut self, snapshot: &Self::Snapshot) -> Result<(), ClipboardError>;
    /// 글로 바꿉니다. `keep_in_history`가 아니면 클립보드 기록·다른 기기 동기화에 남기지 않습니다.
    fn set_text(&mut self, text: &str, keep_in_history: bool) -> Result<(), ClipboardError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("the clipboard is being used by another app")]
    Busy,
    #[error("clipboard error: {0}")]
    Os(#[from] std::io::Error),
    #[error("the clipboard is not supported on this platform")]
    /// 지원하지 않는 OS에서만 만듭니다.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
}

pub struct PasteGuard<C: Clipboard> {
    clipboard: C,
    pending: Option<Pending<C::Snapshot>>,
}

/// 붙여넣기를 기다리는 받아쓰기 글
struct Pending<S> {
    /// 받아쓰기 전 클립보드 내용. 읽지 못했으면 `None`이고 되돌리지 않습니다.
    original: Option<S>,
    /// 받아쓴 글을 넣은 직후의 클립보드 번호
    ours: u32,
    /// 붙여넣기를 감지해 되돌릴 시각
    restore_at: Option<Instant>,
}

impl<C: Clipboard> PasteGuard<C> {
    pub fn new(clipboard: C) -> Self {
        Self {
            clipboard,
            pending: None,
        }
    }

    /// 받아쓴 글을 클립보드에 복사합니다. `restore`면 사용자가 붙여넣은 뒤 원래 내용으로 되돌립니다.
    pub fn copy(&mut self, text: &str, restore: bool) -> Result<(), ClipboardError> {
        if restore {
            return self.put(text, true);
        }
        // 되돌리지 않으므로, 앞서 붙여넣기를 기다리던 글이 있어도 더는 기다리지 않습니다.
        self.pending = None;
        self.clipboard.set_text(text, true)
    }

    /// 커서 위치에 붙여넣으려고 클립보드를 빌려 글을 넣습니다. 붙여넣기 키를 보낸 뒤 [`Self::pasted`]를,
    /// 보내지 못했으면 [`Self::restore_now`]를 불러 원래 내용으로 되돌립니다.
    pub fn lend(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.put(text, false)
    }

    /// 원래 내용을 떠 두고 글을 넣은 뒤, 되돌릴 때까지 기다립니다.
    fn put(&mut self, text: &str, keep_in_history: bool) -> Result<(), ClipboardError> {
        // 아직 붙여넣지 않은 받아쓰기 글이 그대로 있으면, 되돌릴 내용은 그보다 앞의 원래 내용입니다.
        let sequence = self.clipboard.sequence();
        let kept = self
            .pending
            .take()
            .filter(|pending| pending.ours == sequence);
        let was_kept = kept.is_some();
        let original = match kept {
            Some(pending) => pending.original,
            None => match self.clipboard.snapshot() {
                Ok(snapshot) => Some(snapshot),
                Err(error) => {
                    // 받아쓴 글은 그래도 넣습니다. 원래 내용으로는 되돌리지 못합니다.
                    tracing::warn!("could not save the clipboard before dictation: {error}");
                    None
                }
            },
        };

        let before = self.clipboard.sequence();
        if let Err(error) = self.clipboard.set_text(text, keep_in_history) {
            if self.clipboard.sequence() != before {
                // 비우기만 하고 넣지 못했습니다. 원래 내용으로 돌려 둡니다.
                if let Some(original) = &original
                    && let Err(error) = self.clipboard.restore(original)
                {
                    tracing::warn!("could not restore the clipboard: {error}");
                }
            } else if was_kept {
                // 앞의 받아쓰기 글이 그대로 남아 있으므로 계속 기다립니다.
                self.pending = Some(Pending {
                    original,
                    ours: before,
                    restore_at: None,
                });
            }
            return Err(error);
        }
        self.pending = Some(Pending {
            original,
            ours: self.clipboard.sequence(),
            restore_at: None,
        });
        Ok(())
    }

    /// 붙여넣기 단축키가 눌렸습니다(사용자가 눌렀거나, 커서 위치에 넣으려고 앱이 보냄).
    /// 조금 뒤 원래 내용으로 되돌립니다.
    pub fn pasted(&mut self, now: Instant) {
        let sequence = self.clipboard.sequence();
        let Some(pending) = &mut self.pending else {
            return;
        };
        if pending.ours == sequence {
            // 자동 반복으로 여러 번 붙여넣으면 마지막 붙여넣기부터 다시 기다립니다.
            pending.restore_at = Some(now + RESTORE_DELAY);
        } else {
            self.pending = None;
        }
    }

    /// 되돌릴 시각 (붙여넣기를 감지했을 때)
    pub fn restore_at(&self) -> Option<Instant> {
        self.pending.as_ref().and_then(|pending| pending.restore_at)
    }

    /// 되돌릴 시각이 지났으면 되돌리고, 사용자가 다른 것을 복사했으면 기다리기를 그만둡니다.
    pub fn tick(&mut self, now: Instant) {
        let Some(pending) = &self.pending else {
            return;
        };
        if pending.ours != self.clipboard.sequence() {
            tracing::info!("the clipboard changed after dictation; not restoring it");
            self.pending = None;
            return;
        }
        if pending.restore_at.is_some_and(|at| now >= at)
            && let Some(pending) = self.pending.take()
        {
            self.restore(pending);
        }
    }

    /// 받아쓴 글이 클립보드에 있고 붙여넣기를 기다리는 중인지
    pub fn is_waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// 기다리지 않고 지금 되돌립니다. 앱을 끝낼 때와 빌려 넣은 글을 붙여넣지 못했을 때 부릅니다.
    /// 받아쓴 글이 그대로 있을 때만 되돌립니다.
    pub fn restore_now(&mut self) {
        if let Some(pending) = self.pending.take()
            && pending.ours == self.clipboard.sequence()
        {
            self.restore(pending);
        }
    }

    fn restore(&mut self, pending: Pending<C::Snapshot>) {
        let Some(original) = pending.original else {
            return;
        };
        match self.clipboard.restore(&original) {
            Ok(()) => tracing::info!("restored the clipboard"),
            Err(error) => tracing::warn!("could not restore the clipboard: {error}"),
        }
    }
}

#[cfg(test)]
pub mod fake {
    //! 시험용 클립보드

    use std::sync::{Arc, Mutex};

    use super::{Clipboard, ClipboardError};

    #[derive(Debug, Default)]
    pub struct State {
        pub content: Vec<String>,
        pub sequence: u32,
        /// 다음 `set_text`가 비우기만 하고 실패하게 합니다.
        pub fail_set_after_clear: bool,
        /// 다음 `snapshot`이 실패하게 합니다.
        pub fail_snapshot: bool,
        /// 클립보드 기록(Win+V)에 남은 글 (`keep_in_history`로 넣은 것만)
        pub history: Vec<String>,
    }

    /// 상태를 함께 보는 시험용 클립보드
    #[derive(Debug, Clone, Default)]
    pub struct FakeClipboard(pub Arc<Mutex<State>>);

    impl FakeClipboard {
        pub fn with(content: &[&str]) -> Self {
            let clipboard = Self::default();
            clipboard.user_copies(content);
            clipboard
        }

        /// 사용자가 무언가를 복사합니다.
        pub fn user_copies(&self, content: &[&str]) {
            let mut state = self.0.lock().unwrap();
            state.content = content.iter().map(|s| (*s).to_owned()).collect();
            state.sequence += 1;
        }

        pub fn content(&self) -> Vec<String> {
            self.0.lock().unwrap().content.clone()
        }

        pub fn history(&self) -> Vec<String> {
            self.0.lock().unwrap().history.clone()
        }
    }

    impl Clipboard for FakeClipboard {
        type Snapshot = Vec<String>;

        fn sequence(&self) -> u32 {
            self.0.lock().unwrap().sequence
        }

        fn snapshot(&mut self) -> Result<Vec<String>, ClipboardError> {
            let mut state = self.0.lock().unwrap();
            if std::mem::take(&mut state.fail_snapshot) {
                return Err(ClipboardError::Busy);
            }
            Ok(state.content.clone())
        }

        fn restore(&mut self, snapshot: &Vec<String>) -> Result<(), ClipboardError> {
            let mut state = self.0.lock().unwrap();
            state.content.clone_from(snapshot);
            state.sequence += 1;
            Ok(())
        }

        fn set_text(&mut self, text: &str, keep_in_history: bool) -> Result<(), ClipboardError> {
            let mut state = self.0.lock().unwrap();
            if std::mem::take(&mut state.fail_set_after_clear) {
                state.content.clear();
                state.sequence += 1;
                return Err(ClipboardError::Busy);
            }
            state.content = vec![format!("text:{text}")];
            state.sequence += 1;
            if keep_in_history {
                state.history.push(text.to_owned());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeClipboard;
    use super::*;

    fn guard(content: &[&str]) -> (PasteGuard<FakeClipboard>, FakeClipboard) {
        let clipboard = FakeClipboard::with(content);
        (PasteGuard::new(clipboard.clone()), clipboard)
    }

    #[test]
    fn paste_restores_the_original_after_a_delay() {
        let (mut guard, clipboard) = guard(&["image:png", "files:a.txt"]);
        guard.copy("안녕하세요", true).unwrap();
        assert_eq!(clipboard.content(), ["text:안녕하세요"]);
        assert!(guard.is_waiting());

        let now = Instant::now();
        guard.tick(now);
        assert_eq!(
            clipboard.content(),
            ["text:안녕하세요"],
            "붙여넣기 전에는 그대로"
        );

        guard.pasted(now);
        assert_eq!(guard.restore_at(), Some(now + RESTORE_DELAY));
        guard.tick(now + RESTORE_DELAY / 2);
        assert_eq!(
            clipboard.content(),
            ["text:안녕하세요"],
            "붙여넣는 앱이 읽을 때까지 기다림"
        );
        guard.tick(now + RESTORE_DELAY);
        assert_eq!(clipboard.content(), ["image:png", "files:a.txt"]);
        assert!(!guard.is_waiting());
    }

    #[test]
    fn repeated_paste_extends_the_wait() {
        let (mut guard, clipboard) = guard(&["원래"]);
        guard.copy("글", true).unwrap();
        let now = Instant::now();
        guard.pasted(now);
        guard.pasted(now + RESTORE_DELAY / 2);
        guard.tick(now + RESTORE_DELAY);
        assert_eq!(clipboard.content(), ["text:글"]);
        guard.tick(now + RESTORE_DELAY * 2);
        assert_eq!(clipboard.content(), ["원래"]);
    }

    #[test]
    fn copying_something_else_cancels_the_restore() {
        let (mut guard, clipboard) = guard(&["원래"]);
        guard.copy("글", true).unwrap();
        let now = Instant::now();
        guard.pasted(now);
        clipboard.user_copies(&["새로 복사"]);
        guard.tick(now + RESTORE_DELAY);
        assert_eq!(clipboard.content(), ["새로 복사"]);
        assert!(!guard.is_waiting());

        // 붙여넣기 전에 다른 것을 복사해도 마찬가지입니다.
        guard.copy("두 번째", true).unwrap();
        clipboard.user_copies(&["또 복사"]);
        guard.tick(now);
        assert!(!guard.is_waiting(), "다른 것을 복사하면 기다리기를 그만둠");
        guard.restore_now();
        assert_eq!(clipboard.content(), ["또 복사"]);
    }

    #[test]
    fn second_dictation_keeps_the_first_original() {
        let (mut guard, clipboard) = guard(&["원래"]);
        guard.copy("첫째", true).unwrap();
        guard.copy("둘째", true).unwrap();
        assert_eq!(clipboard.content(), ["text:둘째"]);
        let now = Instant::now();
        guard.pasted(now);
        guard.tick(now + RESTORE_DELAY);
        assert_eq!(
            clipboard.content(),
            ["원래"],
            "받아쓰기 글이 아니라 그 전 내용으로 되돌림"
        );
    }

    #[test]
    fn exit_restores_when_not_pasted() {
        let (mut guard, clipboard) = guard(&["원래"]);
        guard.copy("글", true).unwrap();
        guard.restore_now();
        assert_eq!(clipboard.content(), ["원래"]);
        assert!(!guard.is_waiting());
    }

    #[test]
    fn empty_original_is_restored_as_empty() {
        let (mut guard, clipboard) = guard(&[]);
        guard.copy("글", true).unwrap();
        guard.restore_now();
        assert!(clipboard.content().is_empty());
    }

    #[test]
    fn failed_snapshot_still_delivers_but_does_not_restore() {
        let (mut guard, clipboard) = guard(&["원래"]);
        clipboard.0.lock().unwrap().fail_snapshot = true;
        guard.copy("글", true).unwrap();
        assert_eq!(clipboard.content(), ["text:글"]);
        guard.restore_now();
        assert_eq!(
            clipboard.content(),
            ["text:글"],
            "원래 내용을 모르면 그대로 둠"
        );
    }

    #[test]
    fn failed_write_puts_the_original_back() {
        let (mut guard, clipboard) = guard(&["원래"]);
        clipboard.0.lock().unwrap().fail_set_after_clear = true;
        assert!(guard.copy("글", true).is_err());
        assert_eq!(clipboard.content(), ["원래"]);
        assert!(!guard.is_waiting());
    }

    #[test]
    fn copy_without_restore_leaves_the_text() {
        let (mut guard, clipboard) = guard(&["원래"]);
        guard.copy("글", false).unwrap();
        assert_eq!(clipboard.content(), ["text:글"]);
        assert!(!guard.is_waiting(), "되돌리지 않으면 기다리지 않음");
        assert_eq!(
            clipboard.history(),
            ["글"],
            "복사한 글은 클립보드 기록에 남김"
        );

        let now = Instant::now();
        guard.pasted(now);
        guard.tick(now + RESTORE_DELAY);
        guard.restore_now();
        assert_eq!(
            clipboard.content(),
            ["text:글"],
            "붙여넣어도, 앱을 끝내도 그대로"
        );
    }

    #[test]
    fn turning_restore_off_forgets_the_waiting_original() {
        let (mut guard, clipboard) = guard(&["원래"]);
        guard.copy("첫째", true).unwrap();
        guard.copy("둘째", false).unwrap();
        assert!(!guard.is_waiting());
        guard.restore_now();
        assert_eq!(clipboard.content(), ["text:둘째"]);
    }

    #[test]
    fn lent_text_is_restored_after_the_paste_and_kept_out_of_history() {
        let (mut guard, clipboard) = guard(&["image:png"]);
        guard.lend("커서에 넣을 글").unwrap();
        assert_eq!(clipboard.content(), ["text:커서에 넣을 글"]);
        assert!(
            clipboard.history().is_empty(),
            "빌려 넣은 글은 클립보드 기록에 남기지 않음"
        );

        let now = Instant::now();
        guard.pasted(now);
        guard.tick(now + RESTORE_DELAY / 2);
        assert_eq!(
            clipboard.content(),
            ["text:커서에 넣을 글"],
            "붙여넣는 앱이 읽을 때까지 기다림"
        );
        guard.tick(now + RESTORE_DELAY);
        assert_eq!(clipboard.content(), ["image:png"]);
        assert!(!guard.is_waiting());
    }

    #[test]
    fn lent_text_is_returned_at_once_when_the_paste_was_not_sent() {
        let (mut guard, clipboard) = guard(&["원래"]);
        guard.lend("글").unwrap();
        guard.restore_now();
        assert_eq!(clipboard.content(), ["원래"]);
        assert!(!guard.is_waiting());
    }

    #[test]
    fn lending_after_an_unpasted_copy_keeps_the_first_original() {
        let (mut guard, clipboard) = guard(&["원래"]);
        guard.copy("복사한 글", true).unwrap();
        guard.lend("커서 글").unwrap();
        let now = Instant::now();
        guard.pasted(now);
        guard.tick(now + RESTORE_DELAY);
        assert_eq!(
            clipboard.content(),
            ["원래"],
            "받아쓰기 글이 아니라 그 전 내용으로 되돌림"
        );
    }

    #[test]
    fn paste_without_pending_text_does_nothing() {
        let (mut guard, clipboard) = guard(&["원래"]);
        let now = Instant::now();
        guard.pasted(now);
        guard.tick(now + RESTORE_DELAY);
        assert_eq!(clipboard.content(), ["원래"]);
        assert_eq!(guard.restore_at(), None);
    }
}
