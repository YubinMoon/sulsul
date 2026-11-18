//! 모델 관리 화면의 오래 걸리는 일: 모델 내려받기와 엔진 테스트.
//!
//! 화면을 그리지 않습니다. 작업 스레드를 띄우고, 화면 스레드는 매 프레임 진행 상황만 읽어 갑니다.
//! 두 작업 모두 [`CancelToken`]으로 멈출 수 있습니다.
//!
//! 내려받기([`Downloads`])는 음성 인식과 글 다듬기 갈래가 함께 씁니다. 글 다듬기 제공자의 연결
//! 테스트는 `sulsul_llm::client::ConnectionTest`가 작업 스레드를 맡습니다.

use std::cell::Cell;
use std::fs;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use rust_i18n::t;
use sulsul_core::cancel::CancelToken;
use sulsul_core::config::SttConfig;
use sulsul_core::download::{
    DownloadError, DownloadPhase, DownloadProgress, DownloadRequest, download, part_path,
};
use sulsul_core::stt::{
    self, EngineContext, EngineKind, EngineTestResult, ModelVariant, SttError, TranscribeError,
};

use super::entries::EntryKey;
use crate::accel::AccelRequest;
use crate::error_text;
use crate::session::Shared;

/// 내려받기 진행 상황. 작업 스레드가 쓰고 화면이 읽습니다.
#[derive(Debug, Clone)]
pub struct DownloadSnapshot {
    /// 지금 받는 파일의 순서 (1부터)
    pub file_index: usize,
    pub total_files: usize,
    pub file_name: String,
    /// 앞에서 다 받은 파일들의 크기 합
    pub completed_bytes_before: u64,
    /// 받아야 할 전체 크기
    pub total_bytes: u64,
    pub current_downloaded: u64,
    pub current_file_size: u64,
    pub phase: DownloadPhase,
    finished: Option<DownloadEnd>,
}

/// 내려받기가 끝난 모습
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEnd {
    Done,
    Cancelled,
    /// 화면에 보일 실패 이유
    Failed(String),
}

/// 내려받는 모델. 음성 인식 모델과 글 다듬기 모델이 같은 내려받기를 함께 씁니다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadTarget {
    /// 음성 인식 엔진의 카탈로그 모델
    Stt {
        engine_id: String,
        variant_id: String,
    },
    /// 글 다듬기 로컬 모델. 변형 ID는 모든 계열에서 겹치지 않습니다.
    Llm { variant_id: String },
}

impl DownloadTarget {
    pub fn stt(engine_id: &str, variant_id: &str) -> Self {
        Self::Stt {
            engine_id: engine_id.to_owned(),
            variant_id: variant_id.to_owned(),
        }
    }

    pub fn llm(variant_id: &str) -> Self {
        Self::Llm {
            variant_id: variant_id.to_owned(),
        }
    }

    /// 글 다듬기 모델인지
    pub fn is_llm(&self) -> bool {
        matches!(self, Self::Llm { .. })
    }
}

/// 진행 중인 모델 내려받기. 한 번에 하나만 돌립니다.
pub struct ActiveDownload {
    pub target: DownloadTarget,
    /// 알림에 보일 모델 이름
    pub variant_label: String,
    cancel: CancelToken,
    snapshot: Arc<Mutex<DownloadSnapshot>>,
    worker: JoinHandle<()>,
}

impl ActiveDownload {
    /// `variant`에서 아직 없는 파일을 차례로 내려받는 작업 스레드를 띄웁니다.
    /// 받을 파일이 없거나 스레드를 띄우지 못하면 `None`입니다.
    pub fn start(
        egui_ctx: &egui::Context,
        target: DownloadTarget,
        variant: &ModelVariant,
        variant_label: String,
    ) -> Option<Self> {
        let files: Vec<_> = variant.missing_files().into_iter().cloned().collect();
        let first = files.first()?;
        let total_bytes: u64 = files.iter().map(|f| f.asset.size_bytes).sum();
        let cancel = CancelToken::new();
        let snapshot = Arc::new(Mutex::new(DownloadSnapshot {
            file_index: 1,
            total_files: files.len(),
            file_name: first.asset.file_name.clone(),
            completed_bytes_before: 0,
            total_bytes,
            current_downloaded: 0,
            current_file_size: first.asset.size_bytes,
            phase: DownloadPhase::Downloading,
            finished: None,
        }));

        let bg_cancel = cancel.clone();
        let bg_snapshot = snapshot.clone();
        let repaint_ctx = egui_ctx.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("model-download-{}", variant.id))
            .spawn(move || {
                let mut completed_before = 0u64;
                let total_files = files.len();
                let mut end = DownloadEnd::Done;

                for (index, file) in files.iter().enumerate() {
                    if let Ok(mut snap) = bg_snapshot.lock() {
                        snap.file_index = index + 1;
                        snap.total_files = total_files;
                        snap.file_name.clone_from(&file.asset.file_name);
                        snap.completed_bytes_before = completed_before;
                        snap.current_downloaded = 0;
                        snap.current_file_size = file.asset.size_bytes;
                        snap.phase = DownloadPhase::Downloading;
                    }
                    repaint_ctx.request_repaint();

                    let request = DownloadRequest::for_file(file);
                    let mut on_progress = |progress: DownloadProgress| {
                        if let Ok(mut snap) = bg_snapshot.lock() {
                            snap.current_downloaded = progress.downloaded;
                            if let Some(total) = progress.total {
                                snap.current_file_size = total;
                            }
                            snap.phase = progress.phase;
                        }
                        repaint_ctx.request_repaint();
                    };

                    if let Err(error) = download(&request, &bg_cancel, &mut on_progress) {
                        end = match error {
                            DownloadError::Cancelled => DownloadEnd::Cancelled,
                            other => {
                                tracing::warn!("model download failed: {other}");
                                DownloadEnd::Failed(error_text::download(&other))
                            }
                        };
                        break;
                    }
                    completed_before += file.asset.size_bytes;
                }

                if let Ok(mut snap) = bg_snapshot.lock() {
                    snap.finished = Some(end);
                }
                repaint_ctx.request_repaint();
            });
        let worker = match spawned {
            Ok(worker) => worker,
            Err(error) => {
                tracing::error!("could not start the model download thread: {error}");
                return None;
            }
        };

        Some(Self {
            target,
            variant_label,
            cancel,
            snapshot,
            worker,
        })
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// 지금의 진행 상황
    pub fn progress(&self) -> Option<DownloadSnapshot> {
        self.snapshot.lock().ok().map(|snap| snap.clone())
    }

    /// 끝났으면 결과를 돌려줍니다. 작업 스레드가 결과를 남기지 못하고 멈춘 경우도 실패로 봅니다.
    pub fn poll(&self) -> Option<DownloadEnd> {
        let finished = self.progress().and_then(|snap| snap.finished);
        if finished.is_none() && self.worker.is_finished() {
            // 스레드가 끝난 직후에 결과가 들어왔을 수 있으니 한 번 더 봅니다.
            return Some(
                self.progress()
                    .and_then(|snap| snap.finished)
                    .unwrap_or_else(|| {
                        DownloadEnd::Failed(t!("models.download.crashed").into_owned())
                    }),
            );
        }
        finished
    }
}

/// 내려받지 못한 모델과 그 이유
struct DownloadFailure {
    target: DownloadTarget,
    /// 화면에 보일 실패 이유
    message: String,
}

/// 모델 내려받기 상태. 음성 인식과 글 다듬기 갈래가 함께 쓰고, 내려받기는 한 번에 하나만 합니다.
#[derive(Default)]
pub struct Downloads {
    active: Option<ActiveDownload>,
    failure: Option<DownloadFailure>,
    /// 지난번 [`Downloads::take_card_shown`] 뒤로 진행 중인 내려받기의 카드를 화면에 그렸는지
    card_shown: Cell<bool>,
}

impl Downloads {
    /// 진행 중인 내려받기의 카드를 그렸다고 적어 둡니다. 카드를 보고 있으면 실패 원인이 카드에 바로 뜹니다.
    pub fn mark_card_shown(&self) {
        self.card_shown.set(true);
    }

    /// 지난번에 부른 뒤로 진행 중인 내려받기의 카드를 그렸는지 돌려주고 지웁니다. 매 프레임 부르세요.
    pub fn take_card_shown(&self) -> bool {
        self.card_shown.replace(false)
    }

    /// 지금 진행 중인 내려받기
    pub fn active(&self) -> Option<&ActiveDownload> {
        self.active.as_ref()
    }

    /// `target`을 내려받는 중이면 그 내려받기
    pub fn of(&self, target: &DownloadTarget) -> Option<&ActiveDownload> {
        self.active
            .as_ref()
            .filter(|download| &download.target == target)
    }

    /// `target`을 마지막으로 내려받다 실패했으면 그 이유
    pub fn failure_of(&self, target: &DownloadTarget) -> Option<&str> {
        self.failure
            .as_ref()
            .filter(|failure| &failure.target == target)
            .map(|failure| failure.message.as_str())
    }

    /// `variant`를 내려받기 시작합니다. 이미 다른 것을 받는 중이거나 받을 파일이 없으면 아무것도 하지 않습니다.
    pub fn start(
        &mut self,
        egui_ctx: &egui::Context,
        target: DownloadTarget,
        variant: &ModelVariant,
        variant_label: String,
    ) {
        if self.active.is_some() || variant.missing_files().is_empty() {
            return;
        }
        self.failure = None;
        self.active = ActiveDownload::start(egui_ctx, target.clone(), variant, variant_label);
        if self.active.is_none() {
            // 작업 스레드를 띄우지 못했습니다.
            self.failure = Some(DownloadFailure {
                target,
                message: t!("models.download.crashed").into_owned(),
            });
        }
    }

    pub fn cancel(&self) {
        if let Some(download) = &self.active {
            download.cancel();
        }
    }

    /// 내려받기가 끝났으면 내려놓고, 끝난 내려받기와 그 결과를 돌려줍니다.
    /// 실패한 이유는 다음에 같은 모델을 다시 받기 시작할 때까지 기억합니다.
    pub fn take_finished(&mut self) -> Option<(ActiveDownload, DownloadEnd)> {
        let end = self.active.as_ref().and_then(ActiveDownload::poll)?;
        let download = self.active.take()?;
        match &end {
            DownloadEnd::Done => self.failure = None,
            DownloadEnd::Cancelled => {}
            DownloadEnd::Failed(message) => {
                self.failure = Some(DownloadFailure {
                    target: download.target.clone(),
                    message: message.clone(),
                });
            }
        }
        Some((download, end))
    }
}

/// 내려받다 만 조각(`.part`) 파일의 크기 합. 0보다 크면 이어받을 수 있습니다.
pub fn partial_bytes(variant: &ModelVariant) -> u64 {
    variant
        .missing_files()
        .into_iter()
        .map(|f| {
            fs::metadata(part_path(&f.path))
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .sum()
}

/// 테스트하는 대상
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestTarget {
    /// "사용 가능 모델" 목록의 항목
    Entry(EntryKey),
    /// 아직 등록하지 않은 새 제공자 (초안)
    Draft,
}

/// 테스트가 실패한 모습
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestFailure {
    Cancelled,
    /// 화면에 보일 실패 이유
    Failed(String),
}

pub type TestResult = Result<EngineTestResult, TestFailure>;

/// 진행 중인 엔진 테스트. 한 번에 하나만 돌립니다.
pub struct ActiveTest {
    pub target: TestTarget,
    pub started: Instant,
    /// 테스트한 설정이 요청한 가속. 가속 설정이 없는 엔진(클라우드 등)이면 `None`입니다.
    pub accel: Option<AccelRequest>,
    /// 테스트하는 엔진의 종류 (로컬·클라우드)
    pub kind: EngineKind,
    cancel: CancelToken,
    rx: Receiver<TestResult>,
}

impl ActiveTest {
    /// `stt` 설정으로 `engine_id` 엔진을 새로 만들어 짧은 오디오를 인식해 보는 작업 스레드를 띄웁니다.
    /// 스레드를 띄우지 못하면 `None`입니다.
    pub fn start(
        shared: &Shared,
        egui_ctx: &egui::Context,
        target: TestTarget,
        stt: SttConfig,
        engine_id: String,
    ) -> Option<Self> {
        let cancel = CancelToken::new();
        let (tx, rx) = mpsc::channel();
        let registry = shared.registry.clone();
        let ctx = EngineContext {
            paths: shared.paths.clone(),
        };
        // 테스트에 쓰는 설정 그대로에서 읽어, 결과와 견줄 때 화면의 값이 바뀌어도 어긋나지 않게 합니다.
        let accel = AccelRequest::from_settings(&registry, &stt, &engine_id);
        // 등록되지 않은 엔진이면 테스트가 실패하므로 종류는 쓰이지 않습니다.
        let kind = registry
            .descriptor(&engine_id)
            .map_or(EngineKind::Local, |descriptor| descriptor.kind);
        let bg_cancel = cancel.clone();
        let repaint_ctx = egui_ctx.clone();

        let spawned = std::thread::Builder::new()
            .name(format!("stt-test-{engine_id}"))
            .spawn(move || {
                let outcome = stt::test_engine(&registry, &ctx, &stt, &engine_id, bg_cancel)
                    .map_err(|error| match error {
                        TranscribeError::Stt(SttError::Cancelled) => TestFailure::Cancelled,
                        TranscribeError::Stt(e) => {
                            tracing::warn!("engine test failed: {e}");
                            TestFailure::Failed(error_text::stt(&e, &registry))
                        }
                        TranscribeError::Audio(e) => {
                            tracing::warn!("engine test failed: {e}");
                            TestFailure::Failed(error_text::audio(&e))
                        }
                    });
                let _ = tx.send(outcome);
                repaint_ctx.request_repaint();
            });
        if let Err(error) = spawned {
            tracing::error!("could not start the engine test thread: {error}");
            return None;
        }

        Some(Self {
            target,
            started: Instant::now(),
            accel,
            kind,
            cancel,
            rx,
        })
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// 끝났으면 결과를 돌려줍니다. 작업 스레드가 결과를 보내지 못하고 멈춘 경우도 실패로 봅니다.
    pub fn poll(&self) -> Option<TestResult> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(TestFailure::Failed(
                t!("models.test.crashed").into_owned(),
            ))),
        }
    }
}

/// 끝난 테스트의 결과
pub struct TestOutcome {
    pub target: TestTarget,
    pub result: TestResult,
    pub accel: Option<AccelRequest>,
    /// 테스트한 엔진의 종류. 엔진이 장치를 알려 주지 않았을 때 어떻게 보여 줄지 정합니다.
    pub kind: EngineKind,
}

#[cfg(test)]
mod tests {
    use sulsul_core::stt::{ModelAsset, ModelFile};

    use super::*;

    #[test]
    fn partial_bytes_detects_resumable_part_files() {
        let dir = tempfile::tempdir().unwrap();
        let model_path = dir.path().join("ggml-tiny.bin");
        fs::write(part_path(&model_path), b"1234567").unwrap();
        let variant = ModelVariant {
            id: "tiny".into(),
            name_key: "engine.whisper_local.models.tiny.name".into(),
            description_key: "engine.whisper_local.models.tiny.description".into(),
            recommended: false,
            files: vec![ModelFile {
                path: model_path,
                asset: ModelAsset {
                    id: "tiny".into(),
                    file_name: "ggml-tiny.bin".into(),
                    url: "https://example.com".into(),
                    size_bytes: 100,
                    sha256: "a".repeat(64),
                },
            }],
            choices: Vec::new(),
        };
        assert_eq!(partial_bytes(&variant), 7);
    }
}
