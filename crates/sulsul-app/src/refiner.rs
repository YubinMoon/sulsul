//! 단축키 받아쓰기가 쓰는 글 다듬기 [`Refiner`] 보관소.
//!
//! 로컬 모델 실행기를 띄워 둔 채 여러 받아쓰기에서 다시 쓰려고 하나만 둡니다. 작업 스레드는
//! [`SharedRefiner::lock`]으로 잠근 채 미리 올리거나 다듬고, 화면 스레드는 기다리지 않고
//! [`SharedRefiner::release_idle`]로 오래 쉰 실행기만 끝냅니다 ([`crate::transcriber::SharedTranscriber`]와 같은 방식).

use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Instant;

use sulsul_core::AppPaths;
use sulsul_llm::refine::Refiner;
use sulsul_llm::runner::default_runner_path;

#[derive(Clone)]
pub struct SharedRefiner {
    refiner: Arc<Mutex<Refiner>>,
    paths: AppPaths,
}

impl SharedRefiner {
    /// 앱 실행 파일 옆의 로컬 모델 실행기(`sulsul-llm-runner`)를 쓰는 보관소
    pub fn new(paths: AppPaths) -> Self {
        Self {
            refiner: Arc::new(Mutex::new(Refiner::new(
                paths.clone(),
                default_runner_path(),
            ))),
            paths,
        }
    }

    /// 모델 파일을 찾는 앱 경로
    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    /// 보관소를 잠급니다. 다른 작업이 쓰고 있으면 끝날 때까지 기다리므로 작업 스레드에서만 부르세요.
    pub fn lock(&self) -> MutexGuard<'_, Refiner> {
        self.refiner.lock().unwrap_or_else(|poisoned| {
            // 다듬는 중 패닉이 났습니다. 실행기 상태를 믿을 수 없으니 끝내고 다음 작업에서 새로 띄웁니다.
            tracing::warn!("a refinement job panicked; stopping the local model runner");
            let mut refiner = poisoned.into_inner();
            refiner.unload();
            self.refiner.clear_poison();
            refiner
        })
    }

    /// 띄워 둔 실행기를 오래 쓰지 않았으면 끝내고, 아직이면 그 시각에 화면을 다시 그리게 합니다.
    /// 작업 중이라 잠겨 있으면 아무것도 하지 않습니다(작업이 끝나면 화면이 다시 그려집니다).
    pub fn release_idle(&self, ctx: &egui::Context) {
        let mut refiner = match self.refiner.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return,
            Err(TryLockError::Poisoned(poisoned)) => {
                let mut refiner = poisoned.into_inner();
                refiner.unload();
                self.refiner.clear_poison();
                refiner
            }
        };
        let now = Instant::now();
        if refiner.unload_if_idle(now) {
            return;
        }
        if let Some(deadline) = refiner.idle_deadline() {
            ctx.request_repaint_after(deadline.saturating_duration_since(now));
        }
    }

    /// 띄워 둔 실행기를 끝냅니다 (앱을 끝낼 때). 작업 중이면 끝날 때까지 기다리지 않고, 보관소를
    /// 마지막으로 버릴 때 함께 끝납니다.
    pub fn shutdown(&self) {
        if let Ok(mut refiner) = self.refiner.try_lock() {
            refiner.unload();
        }
    }
}
