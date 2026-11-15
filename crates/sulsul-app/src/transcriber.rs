//! 화면과 단축키 받아쓰기가 함께 쓰는 [`Transcriber`].
//!
//! 받아쓰기 테스트 화면과 단축키 받아쓰기가 같은 모델을 두 번 메모리에 올리지 않도록 엔진 보관소를
//! 하나만 둡니다. 작업 스레드는 [`SharedTranscriber::lock`]으로 잠근 채 준비·인식하므로, 두 작업이
//! 겹치면 나중 작업이 앞 작업이 끝나기를 기다립니다. 화면 스레드는 기다리지 않고
//! [`SharedTranscriber::release_idle`]로 쉬는 엔진만 내려놓습니다.

use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Instant;

use sulsul_core::AppPaths;
use sulsul_core::config::SttConfig;
use sulsul_core::stt::{EngineContext, SttRegistry, Transcriber};

#[derive(Clone)]
pub struct SharedTranscriber(Arc<Mutex<Transcriber>>);

impl SharedTranscriber {
    pub fn new(registry: SttRegistry, paths: AppPaths) -> Self {
        Self(Arc::new(Mutex::new(Transcriber::new(
            registry,
            EngineContext { paths },
        ))))
    }

    /// 엔진 보관소를 잠급니다. 다른 작업이 쓰고 있으면 끝날 때까지 기다리므로 작업 스레드에서만 부르세요.
    pub fn lock(&self) -> MutexGuard<'_, Transcriber> {
        self.0.lock().unwrap_or_else(|poisoned| {
            // 인식 중 패닉이 났습니다. 엔진 상태를 믿을 수 없으니 내려놓고 다음 작업에서 새로 만듭니다.
            tracing::warn!("a transcription job panicked; unloading the engine");
            let mut transcriber = poisoned.into_inner();
            transcriber.unload();
            self.0.clear_poison();
            transcriber
        })
    }

    /// 보관한 엔진을 모델 메모리 유지 시간 동안 쓰지 않았으면 내려놓고, 아직이면 그 시각에 화면을
    /// 다시 그리게 합니다. 작업 중이라 잠겨 있으면 아무것도 하지 않습니다(작업이 끝나면 화면이 다시 그려집니다).
    ///
    /// 시각은 매번 지금 설정으로 다시 계산합니다. 화면에서 설정을 바꾸면 앱이 바로 다시 그리고
    /// 파일을 다시 읽으면 이 함수보다 먼저 반영되므로, 줄여서 당겨진 시각도 다음 프레임에 다시
    /// 걸립니다(egui는 가장 이른 요청을 따름). 늘렸으면 먼저 걸어 둔 시각에 한 번 깨어나 새 시각으로 다시 겁니다.
    pub fn release_idle(&self, stt: &SttConfig, ctx: &egui::Context) {
        let mut transcriber = match self.0.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return,
            Err(TryLockError::Poisoned(poisoned)) => {
                let mut transcriber = poisoned.into_inner();
                transcriber.unload();
                self.0.clear_poison();
                transcriber
            }
        };
        let now = Instant::now();
        if transcriber.unload_if_idle(stt, now) {
            return;
        }
        if let Some(deadline) = transcriber.idle_deadline(stt) {
            ctx.request_repaint_after(deadline.saturating_duration_since(now));
        }
    }
}
