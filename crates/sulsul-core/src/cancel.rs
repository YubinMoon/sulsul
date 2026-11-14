//! 오래 걸리는 작업(인식, 내려받기)을 중간에 멈추는 신호.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 작업을 멈추라는 신호. 복제본끼리 상태를 공유하므로, 하나를 작업에 넘기고
/// 다른 하나로 [`cancel`](Self::cancel)을 부르면 됩니다.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// 멈추라고 알립니다. 작업은 다음 확인 시점에 멈춥니다.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_state() {
        let token = CancelToken::new();
        let other = token.clone();
        assert!(!other.is_cancelled());
        token.cancel();
        assert!(other.is_cancelled());
    }
}
