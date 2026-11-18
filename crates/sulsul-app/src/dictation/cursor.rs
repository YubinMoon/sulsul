//! 다른 앱의 입력란(커서 위치)에 받아쓴 글 넣기.
//!
//! 글은 클립보드를 잠깐 빌려([`super::paste::PasteGuard::lend`]) 붙여넣기 키(Ctrl+V)로 넣고, 붙여넣는 앱이
//! 다 읽은 뒤 원래 클립보드 내용으로 되돌립니다. 키 입력으로 한 글자씩 넣지 않는 것은, 그러면 줄바꿈이 Enter가
//! 되어 대화 앱에서 글이 바로 보내지고 편집기의 자동 들여쓰기·괄호 닫기가 끼어들기 때문입니다.
//!
//! OS를 다루는 부분은 [`Cursor`]로 떼어 두어(`crate::cursor`), 규칙을 실제 키 입력 없이 시험합니다.

/// 입력 중인 커서
pub trait Cursor {
    /// 곧 [`Self::is_typing`]을 물을 테니 미리 준비합니다(녹음을 시작할 때). 기다리지 않고 바로 돌아옵니다.
    fn prepare(&mut self) {}
    /// 지금 글을 입력받는 곳(커서가 있는 입력란)이 있고, 그곳에 키를 보낼 수 있는지
    fn is_typing(&mut self) -> bool;
    /// 입력 중인 곳에 붙여넣기 키를 보냅니다.
    fn send_paste(&mut self) -> Result<(), CursorError>;
}

#[derive(Debug, thiserror::Error)]
pub enum CursorError {
    #[error("could not send the paste keys: {0}")]
    Os(#[from] std::io::Error),
    #[error("typing at the cursor is not supported on this platform")]
    /// 지원하지 않는 OS에서만 만듭니다.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
}

#[cfg(test)]
pub mod fake {
    //! 시험용 커서

    use std::sync::{Arc, Mutex};

    use super::{Cursor, CursorError};
    use crate::dictation::paste::fake::FakeClipboard;

    #[derive(Debug, Default)]
    pub struct State {
        /// 입력 중인 곳이 있는지
        pub typing: bool,
        /// 붙여넣기 키를 보내지 못하게 합니다.
        pub fail: bool,
        /// 붙여넣기 키를 보낼 때마다 그때의 클립보드 내용 (= 입력란에 들어간 것)
        pub pasted: Vec<Vec<String>>,
    }

    /// 붙여넣기 키를 받으면 클립보드 내용을 읽어 가는 입력란
    #[derive(Debug, Clone)]
    pub struct FakeCursor {
        pub state: Arc<Mutex<State>>,
        clipboard: FakeClipboard,
    }

    impl FakeCursor {
        pub fn new(clipboard: FakeClipboard, typing: bool) -> Self {
            Self {
                state: Arc::new(Mutex::new(State {
                    typing,
                    ..State::default()
                })),
                clipboard,
            }
        }

        pub fn pasted(&self) -> Vec<Vec<String>> {
            self.state.lock().unwrap().pasted.clone()
        }
    }

    impl Cursor for FakeCursor {
        fn is_typing(&mut self) -> bool {
            self.state.lock().unwrap().typing
        }

        fn send_paste(&mut self) -> Result<(), CursorError> {
            let content = self.clipboard.content();
            let mut state = self.state.lock().unwrap();
            if state.fail {
                return Err(std::io::Error::other("blocked").into());
            }
            state.pasted.push(content);
            Ok(())
        }
    }
}
