//! 작업 스레드에서 도는 일 하나와, 그 결과를 화면 스레드에서 기다리지 않고 확인하는 손잡이.
//!
//! 연결 테스트([`crate::client::ConnectionTest`]), Codex 로그인([`crate::codex::login::CodexLogin`]),
//! Codex 모델 목록 받기가 함께 씁니다. 화면 스레드는 [`Job::poll`]만 부르고, 일이 끝나면
//! `on_finish`가 작업 스레드에서 한 번 불립니다(앱은 egui 다시 그리기 요청을 넘김).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use sulsul_core::CancelToken;

/// 작업 스레드에서 도는 일 하나. 버리면 취소 신호를 보냅니다.
pub struct Job<T> {
    started: Instant,
    cancel: CancelToken,
    /// [`Self::cancel`]을 불렀을 때 결과로 둘 값
    cancelled: T,
    state: Arc<Mutex<Finished<T>>>,
}

struct Finished<T> {
    result: Option<T>,
    /// 끝났을 때까지 걸린 시간
    after: Option<Duration>,
}

impl<T> Finished<T> {
    /// 아직 결과가 없을 때만 결과를 적습니다.
    fn finish(&mut self, result: T, after: Duration) {
        if self.result.is_none() {
            self.result = Some(result);
            self.after = Some(after);
        }
    }
}

impl<T: Clone + Send + 'static> Job<T> {
    /// `work`를 `name` 이름의 작업 스레드에서 시작합니다.
    ///
    /// `work`는 취소 신호를 받아 가끔 확인해야 합니다. `cancelled`는 [`Self::cancel`] 뒤의 결과입니다.
    pub fn start(
        name: &str,
        cancelled: T,
        on_finish: impl Fn() + Send + 'static,
        work: impl FnOnce(&CancelToken) -> T + Send + 'static,
    ) -> std::io::Result<Self> {
        let cancel = CancelToken::new();
        let state = Arc::new(Mutex::new(Finished {
            result: None,
            after: None,
        }));
        let started = Instant::now();

        let worker_cancel = cancel.clone();
        let worker_state = Arc::clone(&state);
        thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let result = work(&worker_cancel);
                worker_state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .finish(result, started.elapsed());
                on_finish();
            })?;

        Ok(Self {
            started,
            cancel,
            cancelled,
            state,
        })
    }

    /// 끝났으면 결과를 돌려줍니다 (부를 때마다 같은 결과).
    /// [`cancel`](Self::cancel) 뒤에는 일이 아직 안 끝났어도 바로 취소 결과입니다.
    pub fn poll(&self) -> Option<T> {
        self.lock().result.clone()
    }

    /// 시작한 뒤 지난 시간. 끝났으면 끝날 때까지 걸린 시간입니다.
    pub fn elapsed(&self) -> Duration {
        self.lock().after.unwrap_or_else(|| self.started.elapsed())
    }

    /// 일을 멈춥니다. 이미 끝났으면 결과를 그대로 둡니다.
    pub fn cancel(&self) {
        self.cancel.cancel();
        let cancelled = self.cancelled.clone();
        self.lock().finish(cancelled, self.started.elapsed());
    }

    fn lock(&self) -> MutexGuard<'_, Finished<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn reports_the_result_once_finished() {
        let (tx, rx) = mpsc::channel();
        let job = Job::start(
            "test-job",
            Err::<u32, &str>("cancelled"),
            move || tx.send(()).unwrap(),
            |_| Ok(7),
        )
        .unwrap();
        rx.recv_timeout(Duration::from_secs(5))
            .expect("끝나면 on_finish가 불려야 합니다");
        assert_eq!(job.poll(), Some(Ok(7)));
        assert_eq!(job.poll(), Some(Ok(7)), "부를 때마다 같은 결과여야 합니다");
        let elapsed = job.elapsed();
        thread::sleep(Duration::from_millis(20));
        assert_eq!(
            job.elapsed(),
            elapsed,
            "끝난 뒤에는 걸린 시간이 늘지 않아야 합니다"
        );
    }

    #[test]
    fn cancel_finishes_immediately_and_signals_the_work() {
        let (tx, rx) = mpsc::channel();
        let job = Job::start(
            "test-job",
            Err::<u32, &str>("cancelled"),
            || {},
            move |cancel| {
                while !cancel.is_cancelled() {
                    thread::sleep(Duration::from_millis(5));
                }
                tx.send(()).unwrap();
                Ok(1)
            },
        )
        .unwrap();
        assert_eq!(job.poll(), None);
        job.cancel();
        assert_eq!(job.poll(), Some(Err("cancelled")));
        rx.recv_timeout(Duration::from_secs(5))
            .expect("일이 취소 신호를 받아야 합니다");
        assert_eq!(
            job.poll(),
            Some(Err("cancelled")),
            "취소 뒤에 끝난 일의 결과로 바뀌면 안 됩니다"
        );
    }

    #[test]
    fn dropping_the_handle_signals_the_work() {
        let (tx, rx) = mpsc::channel();
        let job = Job::start(
            "test-job",
            (),
            || {},
            move |cancel| {
                while !cancel.is_cancelled() {
                    thread::sleep(Duration::from_millis(5));
                }
                tx.send(()).unwrap();
            },
        )
        .unwrap();
        drop(job);
        rx.recv_timeout(Duration::from_secs(5))
            .expect("손잡이를 버리면 일이 취소 신호를 받아야 합니다");
    }
}
