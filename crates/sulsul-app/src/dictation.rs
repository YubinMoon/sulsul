//! 단축키 받아쓰기: 녹음 → 인식 → 글 다듬기(켰을 때) → 클립보드.
//!
//! 전용 스레드(`sulsul-dictation`)에서 돌아 창이 트레이에 숨겨져 있어도 동작합니다.
//! 키보드 후크가 보낸 [`HotkeyEvent`]로 녹음을 시작·끝내고, 인식은 작업 스레드에서
//! 테스트 화면의 받아쓰기 테스트와 함께 쓰는 [`SharedTranscriber`]로 합니다. 글 다듬기를 켰으면 같은 작업
//! 스레드에서 이어서 [`SharedRefiner`]로 다듬습니다. 받아쓴(다듬은) 글은 [`PasteGuard`]가
//! 클립보드에 넣고, 사용자가 붙여넣으면 원래 내용으로 되돌립니다.
//!
//! 글 다듬기에 실패하면 받아쓴 원문 대신 실패 원인을 알리는 글을 클립보드에 넣습니다. 그 글은
//! 다듬은 결과로 착각하지 않게 `[술술 오류]`로 시작합니다(`dictation.refine_failed_clipboard`).
//!
//! 녹음을 끝내면 작은 말소리를 알맞은 크기로 키우고([`AudioClip::auto_gain`]) 앞뒤의 말소리가 없는 부분을
//! 잘라 냅니다([`sulsul_vad`]). 끌 이유가 없어 설정 없이 늘 합니다. 말소리가 없으면 인식하지 않고 끝냅니다.
//! 인식을 시작하지 않으니 터지는 소리([`Cue::Pop`]) 없이 실패음만 내고, 클립보드는 그대로 둡니다.
//! 말소리를 판정하지 못하면(모델을 불러오지 못하는 등) 자르지 않고 받아씁니다.
//!
//! 글을 만들어 낼 때마다(다듬기에 실패해도) 받아쓴 원문과 다듬은 글을 기록([`Entry`])으로 남깁니다.
//! 인식된 말이 없거나 녹음·인식에 실패하면 남기지 않습니다.
//!
//! 화면은 매 프레임 [`Dictation::state`], [`Dictation::take_notices`], [`Dictation::take_entries`]로
//! 상태와 알림, 새 기록을 가져갑니다.
//!
//! 누르는 동안 모드에서는 단축키를 누르자마자 녹음을 시작하지만(앞 소리를 놓치지 않게),
//! [`PUSH_TO_TALK_CONFIRM`]만큼 누르고 있어야 시작음을 내고 녹음 중으로 보입니다. 그 전에 떼거나
//! 다른 키를 누르면(`RightCtrl+C` 등) 아무 일도 없었던 것처럼 버립니다.

pub mod paste;

use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use rust_i18n::t;
use sulsul_core::CancelToken;
use sulsul_core::audio::{AudioClip, AudioError, Recording, RecordingOptions};
use sulsul_core::config::{Config, PostProcessConfig, SttConfig};
use sulsul_core::stt::{SttError, TranscribeError, TranscribeJob};
use sulsul_llm::refine::RefineError;

use self::paste::{Clipboard, ClipboardError, PasteGuard};
use crate::clipboard::SystemClipboard;
use crate::error_text;
use crate::history::{Entry, Refinement};
use crate::hotkey::HotkeyEvent;
use crate::refiner::SharedRefiner;
use crate::session::DictationState;
use crate::sound::{self, Cue};
use crate::transcriber::SharedTranscriber;

/// 누르는 동안 모드에서 이만큼 누르고 있어야 녹음으로 칩니다.
pub const PUSH_TO_TALK_CONFIRM: Duration = Duration::from_millis(250);
/// 녹음 중 최대 길이·장치 오류를 확인하는 간격
const RECORDING_POLL: Duration = Duration::from_millis(50);
/// 붙여넣기를 기다리는 동안 다른 복사가 있었는지 확인하는 간격
const WAITING_POLL: Duration = Duration::from_millis(500);

/// 받아쓰기에 쓰는 설정
#[derive(Debug, Clone, PartialEq)]
pub struct DictationSettings {
    pub input_device: String,
    pub max_record_seconds: u32,
    pub feedback_sounds: bool,
    pub stt: SttConfig,
    /// 글 다듬기 설정 (`enabled`가 꺼져 있으면 다듬지 않음)
    pub postprocess: PostProcessConfig,
}

impl DictationSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            input_device: config.audio.input_device.clone(),
            max_record_seconds: config.audio.max_record_seconds,
            feedback_sounds: config.audio.feedback_sounds,
            stt: config.stt.clone(),
            postprocess: config.postprocess.clone(),
        }
    }
}

/// 화면에 알릴 일
#[derive(Debug)]
pub enum Notice {
    /// 받아쓴 글을 클립보드에 넣음 (`refined`: 글 다듬기를 거쳤는지)
    Copied {
        refined: bool,
    },
    /// 글 다듬기에 실패해 받아쓴 원문 대신 실패 원인을 클립보드에 넣음
    RefineFailed(RefineError),
    /// 인식된 말이 없음
    Empty,
    /// 녹음에 말소리가 없어 받아쓰지 않음
    NoSpeech,
    /// 앞의 받아쓰기를 아직 처리하는 중
    Busy,
    RecordFailed(AudioError),
    Failed(DictationError),
    ClipboardFailed(ClipboardError),
}

#[derive(Debug)]
pub enum DictationError {
    Stt(SttError),
    Audio(AudioError),
    /// 인식 작업 스레드가 결과 없이 끝남
    Crashed,
}

impl From<TranscribeError> for DictationError {
    fn from(error: TranscribeError) -> Self {
        match error {
            TranscribeError::Stt(error) => Self::Stt(error),
            TranscribeError::Audio(error) => Self::Audio(error),
        }
    }
}

/// 마이크
pub trait Microphone {
    type Recording: ActiveRecording;
    fn start(&mut self, options: &RecordingOptions) -> Result<Self::Recording, AudioError>;
}

/// 진행 중인 녹음
pub trait ActiveRecording {
    fn limit_reached(&self) -> bool;
    fn error(&self) -> Option<String>;
    fn stop(self) -> Result<AudioClip, AudioError>;
}

/// 시스템 기본 마이크 ([`Recording`])
pub struct SystemMicrophone;

impl Microphone for SystemMicrophone {
    type Recording = Recording;

    fn start(&mut self, options: &RecordingOptions) -> Result<Recording, AudioError> {
        Recording::start(options)
    }
}

impl ActiveRecording for Recording {
    fn limit_reached(&self) -> bool {
        Recording::limit_reached(self)
    }

    fn error(&self) -> Option<String> {
        Recording::error(self)
    }

    fn stop(self) -> Result<AudioClip, AudioError> {
        Recording::stop(self)
    }
}

enum Msg {
    Hotkey(HotkeyEvent),
    Settings(Box<DictationSettings>),
    Transcribed(Result<Outcome, DictationError>),
    Shutdown,
}

/// 인식(과 다듬기)을 마친 결과
#[derive(Debug)]
enum Outcome {
    /// 받아쓴 원문과 (다듬었으면) 다듬은 글. 인식된 말이 없으면 원문이 비어 있습니다.
    Text {
        transcript: String,
        refined: Option<RefinedText>,
    },
    /// 받아쓰기는 됐지만 다듬지 못했습니다.
    RefineFailed {
        transcript: String,
        prompt: String,
        error: RefineError,
    },
}

#[derive(Debug)]
struct RefinedText {
    text: String,
    /// 쓴 프롬프트 이름
    prompt: String,
}

/// 화면이 가져가는 상태
#[derive(Default)]
struct Published {
    state: DictationState,
    notices: Vec<Notice>,
    entries: Vec<Entry>,
}

/// 받아쓰기 스레드 손잡이. 버리면 스레드를 끝냅니다(붙여넣지 않은 글이 있으면 클립보드를 되돌림).
pub struct Dictation {
    tx: Sender<Msg>,
    published: Arc<Mutex<Published>>,
    thread: Option<JoinHandle<()>>,
}

impl Dictation {
    /// 시스템 마이크·클립보드·효과음으로 받아쓰기 스레드를 시작합니다.
    /// `wake`는 상태가 바뀌면 불립니다(화면을 다시 그리게 합니다).
    pub fn start(
        settings: DictationSettings,
        transcriber: SharedTranscriber,
        refiner: SharedRefiner,
        wake: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        Self::start_with(
            settings,
            transcriber,
            refiner,
            || (SystemMicrophone, SystemClipboard),
            sound::play,
            wake,
        )
    }

    /// 마이크·클립보드·효과음을 골라 시작합니다. `devices`는 받아쓰기 스레드 안에서 불립니다.
    fn start_with<M, C>(
        settings: DictationSettings,
        transcriber: SharedTranscriber,
        refiner: SharedRefiner,
        devices: impl FnOnce() -> (M, C) + Send + 'static,
        play: impl Fn(Cue) + Send + 'static,
        wake: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self>
    where
        M: Microphone,
        C: Clipboard,
    {
        let (tx, rx) = mpsc::channel();
        let published = Arc::new(Mutex::new(Published::default()));
        let service_tx = tx.clone();
        let service_published = Arc::clone(&published);
        let thread = std::thread::Builder::new()
            .name("sulsul-dictation".into())
            .spawn(move || {
                let (microphone, clipboard) = devices();
                Service {
                    rx,
                    tx: service_tx,
                    settings,
                    transcriber,
                    refiner,
                    microphone,
                    paste: PasteGuard::new(clipboard),
                    state: State::Idle,
                    published: service_published,
                    notices: Vec::new(),
                    entries: Vec::new(),
                    play: Box::new(play),
                    wake: Box::new(wake),
                }
                .run();
            })?;
        Ok(Self {
            tx,
            published,
            thread: Some(thread),
        })
    }

    /// 키보드 후크에 넘길 알림 받는 곳
    pub fn hotkey_sink(&self) -> impl Fn(HotkeyEvent) + Send + 'static {
        let tx = self.tx.clone();
        move |event| {
            let _ = tx.send(Msg::Hotkey(event));
        }
    }

    pub fn update_settings(&self, settings: DictationSettings) {
        let _ = self.tx.send(Msg::Settings(Box::new(settings)));
    }

    pub fn state(&self) -> DictationState {
        lock(&self.published).state
    }

    /// 쌓인 알림을 가져갑니다.
    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut lock(&self.published).notices)
    }

    /// 새로 남긴 받아쓰기 기록을 가져갑니다 (오래된 것부터).
    pub fn take_entries(&self) -> Vec<Entry> {
        std::mem::take(&mut lock(&self.published).entries)
    }

    /// 스레드를 끝내고 기다립니다. 붙여넣지 않은 받아쓰기 글이 있으면 클립보드를 되돌립니다.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn lock(published: &Mutex<Published>) -> MutexGuard<'_, Published> {
    published.lock().unwrap_or_else(PoisonError::into_inner)
}

enum State<R> {
    Idle,
    Recording {
        recording: R,
        started: Instant,
        push_to_talk: bool,
        /// 시작음을 내고 녹음 중으로 보이는지 (누르는 동안 모드는 잠깐 누르고 있어야 함)
        confirmed: bool,
    },
    /// 누르는 동안 모드에서 녹음을 시작하지 못함. 계속 누르고 있으면 알립니다.
    StartFailed {
        error: AudioError,
        since: Instant,
    },
    Transcribing {
        cancel: CancelToken,
        /// 녹음을 끝낸 시각 (기록에 남기는 받아쓴 시각)
        at: SystemTime,
    },
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    Idle,
    Recording { push_to_talk: bool, confirmed: bool },
    StartFailed { since: Instant },
    Transcribing,
}

/// 시간이 지나서 할 일
enum Tick {
    Nothing,
    DeviceError { error: String, report: bool },
    LimitReached,
    Confirmed,
    ReportStartFailure,
}

struct Service<M: Microphone, C: Clipboard> {
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    settings: DictationSettings,
    transcriber: SharedTranscriber,
    refiner: SharedRefiner,
    microphone: M,
    paste: PasteGuard<C>,
    state: State<M::Recording>,
    published: Arc<Mutex<Published>>,
    /// 아직 화면에 넘기지 않은 알림
    notices: Vec<Notice>,
    /// 아직 화면에 넘기지 않은 기록
    entries: Vec<Entry>,
    play: Box<dyn Fn(Cue) + Send>,
    wake: Box<dyn Fn() + Send>,
}

impl<M: Microphone, C: Clipboard> Service<M, C> {
    fn run(mut self) {
        tracing::info!("dictation service started");
        // 첫 받아쓰기가 말소리 감지 모델을 불러오느라 늦어지지 않게 미리 불러 둡니다.
        if let Err(error) = sulsul_vad::preload() {
            tracing::warn!("could not load the speech detector: {error}");
        }
        loop {
            let message = match self.next_wake() {
                Some(timeout) => match self.rx.recv_timeout(timeout) {
                    Ok(message) => Some(message),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match self.rx.recv() {
                    Ok(message) => Some(message),
                    Err(_) => break,
                },
            };
            match message {
                Some(Msg::Shutdown) => break,
                Some(Msg::Hotkey(event)) => self.on_hotkey(event),
                Some(Msg::Settings(settings)) => self.settings = *settings,
                Some(Msg::Transcribed(result)) => self.on_transcribed(result),
                None => {}
            }
            self.tick(Instant::now());
            self.publish();
        }
        self.finish();
        tracing::info!("dictation service stopped");
    }

    /// 메시지가 없어도 깨어나야 할 때까지 남은 시간
    fn next_wake(&self) -> Option<Duration> {
        match &self.state {
            State::Recording { .. } => return Some(RECORDING_POLL),
            State::StartFailed { since, .. } => {
                return Some(
                    (*since + PUSH_TO_TALK_CONFIRM).saturating_duration_since(Instant::now()),
                );
            }
            State::Idle | State::Transcribing { .. } => {}
        }
        if let Some(at) = self.paste.restore_at() {
            return Some(
                at.saturating_duration_since(Instant::now())
                    .min(WAITING_POLL),
            );
        }
        self.paste.is_waiting().then_some(WAITING_POLL)
    }

    /// 지금 상태의 종류 (상태를 빌리지 않고 판단하려고 복사해 씁니다)
    fn phase(&self) -> Phase {
        match &self.state {
            State::Idle => Phase::Idle,
            State::Recording {
                push_to_talk,
                confirmed,
                ..
            } => Phase::Recording {
                push_to_talk: *push_to_talk,
                confirmed: *confirmed,
            },
            State::StartFailed { since, .. } => Phase::StartFailed { since: *since },
            State::Transcribing { .. } => Phase::Transcribing,
        }
    }

    fn on_hotkey(&mut self, event: HotkeyEvent) {
        let now = Instant::now();
        match (event, self.phase()) {
            (HotkeyEvent::Toggle, Phase::Idle | Phase::StartFailed { .. }) => {
                self.start_recording(false, now);
            }
            (HotkeyEvent::Toggle, Phase::Recording { .. }) => self.finish_recording(),
            (HotkeyEvent::Toggle | HotkeyEvent::Press, Phase::Transcribing) => self.busy(),
            (HotkeyEvent::Press, Phase::Idle | Phase::StartFailed { .. }) => {
                self.start_recording(true, now);
            }
            (
                HotkeyEvent::Release,
                Phase::Recording {
                    push_to_talk: true,
                    confirmed: false,
                },
            ) => {
                // 너무 짧게 눌렀습니다. 아무 일도 없었던 것처럼 버립니다.
                self.state = State::Idle;
            }
            (HotkeyEvent::Release, Phase::Recording { .. }) => self.finish_recording(),
            (HotkeyEvent::Release, Phase::StartFailed { since }) => {
                if now.duration_since(since) >= PUSH_TO_TALK_CONFIRM {
                    self.report_start_failure();
                } else {
                    self.state = State::Idle;
                }
            }
            (
                HotkeyEvent::Cancel,
                Phase::Recording {
                    push_to_talk: true,
                    confirmed,
                },
            ) => {
                tracing::info!("dictation cancelled: another key was pressed");
                self.state = State::Idle;
                if confirmed {
                    self.cue(Cue::Fail);
                }
            }
            (HotkeyEvent::Cancel, Phase::StartFailed { .. }) => self.state = State::Idle,
            (HotkeyEvent::Paste, _) => self.paste.pasted(now),
            _ => {}
        }
    }

    fn start_recording(&mut self, push_to_talk: bool, now: Instant) {
        let options = RecordingOptions {
            device: self.settings.input_device.clone(),
            max_duration: Some(Duration::from_secs(u64::from(
                self.settings.max_record_seconds.max(1),
            ))),
        };
        match self.microphone.start(&options) {
            Ok(recording) => {
                self.state = State::Recording {
                    recording,
                    started: now,
                    push_to_talk,
                    confirmed: !push_to_talk,
                };
                if !push_to_talk {
                    self.on_confirmed();
                }
            }
            Err(error) => {
                tracing::warn!("could not start dictation recording: {error}");
                if push_to_talk {
                    // 다른 키와 함께 누른 것일 수도 있으니, 계속 누르고 있을 때만 알립니다.
                    self.state = State::StartFailed { error, since: now };
                } else {
                    self.state = State::Idle;
                    self.fail(Notice::RecordFailed(error));
                }
            }
        }
    }

    /// 녹음이 확정됐습니다: 시작음을 내고, 녹음하는 동안 인식 모델과 (켰으면) 다듬기 모델을 미리 읽어 둡니다.
    fn on_confirmed(&mut self) {
        tracing::info!("dictation recording started");
        self.cue(Cue::Start);
        let transcriber = self.transcriber.clone();
        let stt = self.settings.stt.clone();
        let refiner = self.refiner.clone();
        let post = self.settings.postprocess.clone();
        let spawned = std::thread::Builder::new()
            .name("sulsul-dictation-prepare".into())
            .spawn(move || {
                // 실패하면 인식·다듬기를 할 때 다시 시도하고 그때 알립니다.
                if let Err(error) = transcriber.lock().prepare(&stt, &stt.engine) {
                    tracing::debug!("could not prepare the engine while recording: {error}");
                }
                if let Err(error) = refiner.lock().prepare(&post, &CancelToken::new()) {
                    tracing::debug!(
                        "could not prepare the refinement model while recording: {error}"
                    );
                }
            });
        if let Err(error) = spawned {
            tracing::debug!("could not start the engine preparation thread: {error}");
        }
    }

    fn report_start_failure(&mut self) {
        if let State::StartFailed { error, .. } = std::mem::replace(&mut self.state, State::Idle) {
            self.fail(Notice::RecordFailed(error));
        }
    }

    fn finish_recording(&mut self) {
        let State::Recording { recording, .. } = std::mem::replace(&mut self.state, State::Idle)
        else {
            return;
        };
        let clip = match recording.stop() {
            Ok(clip) => clip,
            Err(error) => {
                tracing::warn!("dictation recording failed: {error}");
                self.fail(Notice::RecordFailed(error));
                return;
            }
        };
        if clip.is_empty() {
            self.fail(Notice::Empty);
            return;
        }
        tracing::info!(
            seconds = clip.duration().as_secs_f32(),
            "dictation recording finished"
        );
        // 말소리가 없으면 모델을 돌리지 않고 여기서 끝냅니다 (조용한 녹음은 환각을 만들기 쉬움).
        let speech = match sulsul_vad::find_speech(&clip) {
            Ok(Some(speech)) => Some(speech.range),
            Ok(None) => {
                tracing::info!("dictation skipped: no speech in the recording");
                self.fail(Notice::NoSpeech);
                return;
            }
            Err(error) => {
                tracing::warn!(
                    "could not check the recording for speech, keeping all of it: {error}"
                );
                None
            }
        };

        let at = SystemTime::now();
        let cancel = CancelToken::new();
        let transcriber = self.transcriber.clone();
        let refiner = self.refiner.clone();
        let stt = self.settings.stt.clone();
        let post = self.settings.postprocess.clone();
        let tx = self.tx.clone();
        let job_cancel = cancel.clone();
        let mut clip = clip;
        let spawned = std::thread::Builder::new()
            .name("sulsul-dictation-stt".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    // 배경 소음을 잘 재도록 자르기 전의 녹음 전체로 음량을 맞춥니다.
                    clip.auto_gain();
                    if let Some(range) = speech {
                        clip.keep_range(range);
                    }
                    let job = TranscribeJob {
                        stt: &stt,
                        engine_id: &stt.engine,
                        language: None,
                        cancel: job_cancel.clone(),
                    };
                    // 테스트 화면의 받아쓰기 테스트가 엔진을 쓰고 있으면 끝날 때까지 기다립니다.
                    let text = transcriber
                        .lock()
                        .transcribe(&clip, &job)
                        .map(|done| done.transcript.text)
                        .map_err(DictationError::from)?;
                    Ok(refine(&refiner, &post, text, &job_cancel))
                }))
                .unwrap_or(Err(DictationError::Crashed));
                let _ = tx.send(Msg::Transcribed(result));
            });
        match spawned {
            Ok(_) => {
                self.state = State::Transcribing { cancel, at };
                self.cue(Cue::Pop);
            }
            Err(error) => {
                tracing::error!("could not start the dictation transcription thread: {error}");
                self.fail(Notice::Failed(DictationError::Crashed));
            }
        }
    }

    fn on_transcribed(&mut self, result: Result<Outcome, DictationError>) {
        let at = match &self.state {
            State::Transcribing { at, .. } => *at,
            _ => return,
        };
        self.state = State::Idle;
        let (transcript, refined) = match result {
            Ok(Outcome::Text {
                transcript,
                refined,
            }) => (transcript, refined),
            Ok(Outcome::RefineFailed {
                transcript,
                prompt,
                error,
            }) => {
                self.deliver_refine_failure(at, &transcript, prompt, error);
                return;
            }
            Err(DictationError::Stt(SttError::Cancelled)) => return,
            Err(error) => {
                tracing::warn!("dictation transcription failed: {error:?}");
                self.fail(Notice::Failed(error));
                return;
            }
        };
        let transcript = transcript.trim();
        if transcript.is_empty() {
            // 클립보드는 건드리지 않고 기록도 남기지 않습니다.
            tracing::info!("dictation produced no text");
            self.fail(Notice::Empty);
            return;
        }
        let (text, refine) = match refined {
            Some(RefinedText { text, prompt }) => {
                let text = text.trim().to_owned();
                (text.clone(), Refinement::Done { text, prompt })
            }
            None => (transcript.to_owned(), Refinement::None),
        };
        let refined = !refine.is_none();
        // 클립보드에 넣지 못해도 기록에서 다시 복사할 수 있게 먼저 남깁니다.
        self.record(at, transcript, refine);
        if text.is_empty() {
            // 다듬은 글이 비었습니다. 클립보드는 건드리지 않습니다.
            tracing::info!("refinement produced no text");
            self.fail(Notice::Empty);
            return;
        }
        match self.paste.deliver(&text) {
            Ok(()) => {
                tracing::info!(
                    chars = text.chars().count(),
                    refined,
                    "dictation copied to the clipboard"
                );
                self.notices.push(Notice::Copied { refined });
                self.cue(Cue::Done);
            }
            Err(error) => {
                tracing::warn!("could not copy dictation to the clipboard: {error}");
                self.fail(Notice::ClipboardFailed(error));
            }
        }
    }

    /// 받아쓰기 기록을 남깁니다. 원문이 비어 있으면 남기지 않습니다.
    fn record(&mut self, at: SystemTime, transcript: &str, refine: Refinement) {
        let transcript = transcript.trim();
        if !transcript.is_empty() {
            self.entries.push(Entry::new(at, transcript, refine));
        }
    }

    /// 다듬지 못했습니다: 받아쓴 원문은 기록에만 남기고, 클립보드에는 원문 대신 실패 원인을 알리는 글을
    /// 넣습니다. 붙여넣으면 받아쓴 글처럼 원래 클립보드 내용으로 되돌립니다.
    fn deliver_refine_failure(
        &mut self,
        at: SystemTime,
        transcript: &str,
        prompt: String,
        error: RefineError,
    ) {
        tracing::warn!("dictation refinement failed: {error}");
        let reason = error_text::refine(&error, self.refiner.paths(), &self.settings.postprocess);
        self.record(
            at,
            transcript,
            Refinement::Failed {
                prompt,
                reason: reason.clone(),
            },
        );
        let notice = t!("dictation.refine_failed_clipboard", message = reason);
        match self.paste.deliver(&notice) {
            Ok(()) => self.fail(Notice::RefineFailed(error)),
            Err(clipboard) => {
                tracing::warn!("could not copy the refinement error to the clipboard: {clipboard}");
                self.fail(Notice::ClipboardFailed(clipboard));
            }
        }
    }

    fn tick(&mut self, now: Instant) {
        let action = match &mut self.state {
            State::Recording {
                recording,
                started,
                push_to_talk,
                confirmed,
            } => {
                if let Some(error) = recording.error() {
                    Tick::DeviceError {
                        error,
                        report: *confirmed,
                    }
                } else if recording.limit_reached() {
                    Tick::LimitReached
                } else if *push_to_talk
                    && !*confirmed
                    && now.duration_since(*started) >= PUSH_TO_TALK_CONFIRM
                {
                    *confirmed = true;
                    Tick::Confirmed
                } else {
                    Tick::Nothing
                }
            }
            State::StartFailed { since, .. }
                if now.duration_since(*since) >= PUSH_TO_TALK_CONFIRM =>
            {
                Tick::ReportStartFailure
            }
            _ => Tick::Nothing,
        };
        match action {
            Tick::Nothing => {}
            Tick::DeviceError { error, report } => {
                tracing::warn!("dictation recording stopped by a device error: {error}");
                self.state = State::Idle;
                if report {
                    self.fail(Notice::RecordFailed(AudioError::Stream(error)));
                }
            }
            Tick::LimitReached => {
                tracing::info!("dictation reached the maximum recording length");
                self.finish_recording();
            }
            Tick::Confirmed => self.on_confirmed(),
            Tick::ReportStartFailure => self.report_start_failure(),
        }
        self.paste.tick(now);
    }

    fn busy(&mut self) {
        self.notices.push(Notice::Busy);
        self.cue(Cue::Fail);
    }

    fn fail(&mut self, notice: Notice) {
        self.notices.push(notice);
        self.cue(Cue::Fail);
    }

    fn cue(&self, cue: Cue) {
        if self.settings.feedback_sounds {
            (self.play)(cue);
        }
    }

    fn publish(&mut self) {
        let state = match &self.state {
            State::Recording {
                confirmed: true, ..
            } => DictationState::Recording,
            State::Transcribing { .. } => DictationState::Transcribing,
            _ if self.paste.is_waiting() => DictationState::WaitingPaste,
            _ => DictationState::Idle,
        };
        let changed = {
            let mut published = lock(&self.published);
            let changed =
                published.state != state || !self.notices.is_empty() || !self.entries.is_empty();
            published.state = state;
            published.notices.append(&mut self.notices);
            published.entries.append(&mut self.entries);
            changed
        };
        if changed {
            (self.wake)();
        }
    }

    fn finish(&mut self) {
        if let State::Transcribing { cancel, .. } = &self.state {
            cancel.cancel();
        }
        self.state = State::Idle;
        self.paste.finish();
    }
}

/// 글 다듬기를 켰으면 받아쓴 글을 다듬습니다. 인식된 말이 없으면 다듬지 않습니다.
fn refine(
    refiner: &SharedRefiner,
    post: &PostProcessConfig,
    transcript: String,
    cancel: &CancelToken,
) -> Outcome {
    if !post.enabled || transcript.trim().is_empty() {
        return Outcome::Text {
            transcript,
            refined: None,
        };
    }
    // 기록에 남길 프롬프트 이름 (다듬기가 고르는 것과 같은 규칙)
    let prompt = post
        .active_prompt()
        .map(|prompt| prompt.name.trim().to_owned())
        .unwrap_or_default();
    match refiner.lock().refine(post, transcript.trim(), cancel) {
        Ok(refined) => {
            tracing::info!(
                seconds = refined.elapsed.as_secs_f32(),
                "dictation text refined"
            );
            Outcome::Text {
                transcript,
                refined: Some(RefinedText {
                    text: refined.text,
                    prompt,
                }),
            }
        }
        // 앱을 끝내느라 취소했습니다. 받아쓰기 스레드는 이 결과를 버립니다.
        Err(error) if error.is_cancelled() => Outcome::Text {
            transcript,
            refined: None,
        },
        Err(error) => Outcome::RefineFailed {
            transcript,
            prompt,
            error,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::paste::fake::FakeClipboard;
    use super::*;
    use sulsul_core::AppPaths;
    use sulsul_core::stt::SttRegistry;
    use sulsul_core::stt::mock::{MOCK_ENGINE_ID, MockEngineFactory};
    use sulsul_core::toml::Value;
    use sulsul_llm::CLOUD_PROCESSOR_ID;
    use sulsul_llm::client::LlmError;
    use sulsul_llm::presets::{MOCK_PROVIDER_ID, profile_from_preset};

    /// 시험용 마이크가 돌려줄 녹음
    #[derive(Debug, Clone, Copy)]
    enum Mic {
        /// 1초 무음, 말소리(약 3.4초), 1초 무음
        Speech,
        /// 1초 동안 아무 소리도 들어오지 않음
        Silent,
        /// 녹음을 시작하지 못함
        Broken,
    }

    struct FakeMicrophone(Mic);

    struct FakeRecording(Mic);

    impl Microphone for FakeMicrophone {
        type Recording = FakeRecording;
        fn start(&mut self, _options: &RecordingOptions) -> Result<FakeRecording, AudioError> {
            match self.0 {
                Mic::Broken => Err(AudioError::NoInputDevice),
                mic => Ok(FakeRecording(mic)),
            }
        }
    }

    impl ActiveRecording for FakeRecording {
        fn limit_reached(&self) -> bool {
            false
        }
        fn error(&self) -> Option<String> {
            None
        }
        fn stop(self) -> Result<AudioClip, AudioError> {
            Ok(match self.0 {
                Mic::Silent => AudioClip::new(vec![0.0; 16_000], 16_000),
                Mic::Speech | Mic::Broken => {
                    // 말소리 감지 모델이 말로 알아듣도록 Windows 음성 합성으로 만든 말을 씁니다.
                    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../sulsul-vad/testdata/hello_ko.wav");
                    let speech = sulsul_core::audio::read_wav(&path)?;
                    let quiet = std::iter::repeat_n(0.0, speech.sample_rate as usize);
                    let samples = quiet.clone().chain(speech.samples).chain(quiet).collect();
                    AudioClip::new(samples, speech.sample_rate)
                }
            })
        }
    }

    fn settings(text: &str) -> DictationSettings {
        let mut stt = SttConfig {
            engine: MOCK_ENGINE_ID.to_owned(),
            ..SttConfig::default()
        };
        let table = stt.engines.entry(MOCK_ENGINE_ID.to_owned()).or_default();
        table.insert("delay_ms".into(), Value::Integer(0));
        table.insert("text".into(), Value::String(text.to_owned()));
        DictationSettings {
            input_device: String::new(),
            max_record_seconds: 60,
            feedback_sounds: true,
            stt,
            postprocess: PostProcessConfig::default(),
        }
    }

    /// 가짜 외부 제공자로 다듬는 설정. API 키가 `fail`이면 제공자가 인증 실패로 답합니다.
    fn refining_settings(text: &str, api_key: &str) -> DictationSettings {
        let mut profile = profile_from_preset(MOCK_PROVIDER_ID, "m".into(), "가짜".into());
        profile.api_key = api_key.into();
        DictationSettings {
            postprocess: PostProcessConfig {
                enabled: true,
                processor: CLOUD_PROCESSOR_ID.into(),
                api_profiles: vec![profile],
                ..PostProcessConfig::default()
            },
            ..settings(text)
        }
    }

    struct Harness {
        dictation: Dictation,
        clipboard: FakeClipboard,
        cues: Arc<Mutex<Vec<Cue>>>,
        notices: Vec<Notice>,
        entries: Vec<Entry>,
    }

    impl Harness {
        fn new(text: &str, mic: Mic) -> Self {
            Self::with(settings(text), mic)
        }

        fn with(settings: DictationSettings, mic: Mic) -> Self {
            let mut registry = SttRegistry::new();
            registry.register(Arc::new(MockEngineFactory::new()));
            let transcriber = SharedTranscriber::new(registry, AppPaths::under("unused"));
            let clipboard = FakeClipboard::with(&["원래 내용"]);
            let cues = Arc::new(Mutex::new(Vec::new()));
            let service_clipboard = clipboard.clone();
            let service_cues = Arc::clone(&cues);
            let dictation = Dictation::start_with(
                settings,
                transcriber,
                SharedRefiner::new(AppPaths::under("unused")),
                move || (FakeMicrophone(mic), service_clipboard),
                move |cue| service_cues.lock().unwrap().push(cue),
                || {},
            )
            .unwrap();
            Self {
                dictation,
                clipboard,
                cues,
                notices: Vec::new(),
                entries: Vec::new(),
            }
        }

        fn send(&self, event: HotkeyEvent) {
            (self.dictation.hotkey_sink())(event);
        }

        /// `done`이 참이 될 때까지 기다립니다 (받아쓰기 스레드는 따로 돕니다).
        fn wait_until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let notices = self.dictation.take_notices();
                self.notices.extend(notices);
                let entries = self.dictation.take_entries();
                self.entries.extend(entries);
                if done(self) {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "기다리다 시간이 지났습니다: {what}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn cues(&self) -> Vec<Cue> {
            self.cues.lock().unwrap().clone()
        }
    }

    #[test]
    fn toggle_dictation_copies_and_restores_after_paste() {
        let mut h = Harness::new("  안녕하세요  ", Mic::Speech);
        h.send(HotkeyEvent::Toggle);
        h.wait_until("녹음 시작", |h| {
            h.dictation.state() == DictationState::Recording
        });
        assert_eq!(h.cues(), [Cue::Start]);

        h.send(HotkeyEvent::Toggle);
        h.wait_until("클립보드에 복사", |h| {
            h.dictation.state() == DictationState::WaitingPaste
        });
        assert!(matches!(
            h.notices.as_slice(),
            [Notice::Copied { refined: false }]
        ));
        assert_eq!(
            h.clipboard.content(),
            ["text:안녕하세요"],
            "앞뒤 공백은 지움"
        );
        assert_eq!(h.cues(), [Cue::Start, Cue::Pop, Cue::Done]);
        assert_eq!(h.entries.len(), 1, "받아쓴 글을 기록에 남김");
        assert_eq!(h.entries[0].transcript, "안녕하세요");
        assert_eq!(h.entries[0].refine, Refinement::None);
        assert!(h.entries[0].time > 0);

        h.send(HotkeyEvent::Paste);
        h.wait_until("원래 내용으로 되돌림", |h| {
            h.clipboard.content() == ["원래 내용"]
        });
        h.wait_until("기다리기 끝", |h| {
            h.dictation.state() == DictationState::Idle
        });
        assert_eq!(h.entries.len(), 1, "붙여넣어도 기록은 그대로");
    }

    #[test]
    fn empty_result_leaves_the_clipboard_alone() {
        let mut h = Harness::new("   ", Mic::Speech);
        h.send(HotkeyEvent::Toggle);
        h.send(HotkeyEvent::Toggle);
        h.wait_until("인식 결과", |h| !h.notices.is_empty());
        assert!(matches!(h.notices.as_slice(), [Notice::Empty]));
        assert_eq!(h.clipboard.content(), ["원래 내용"]);
        assert_eq!(h.cues(), [Cue::Start, Cue::Pop, Cue::Fail]);
        assert_eq!(h.dictation.state(), DictationState::Idle);
        assert!(h.entries.is_empty(), "인식된 말이 없으면 기록하지 않음");
    }

    #[test]
    fn speech_is_trimmed_before_transcribing() {
        // 가짜 엔진은 글을 정하지 않으면 받은 오디오의 길이를 돌려줍니다.
        let mut h = Harness::new("", Mic::Speech);
        h.send(HotkeyEvent::Toggle);
        h.send(HotkeyEvent::Toggle);
        h.wait_until("복사", |h| !h.notices.is_empty());
        let content = h.clipboard.content();
        let seconds: f64 = content[0]
            .split("s of audio")
            .next()
            .and_then(|head| head.rsplit(' ').next())
            .and_then(|seconds| seconds.parse().ok())
            .unwrap_or_else(|| panic!("받은 오디오 길이가 있어야 합니다: {content:?}"));
        // 녹음은 5.4초(앞뒤 1초 무음 + 말 3.4초)이고, 무음은 여유(앞 0.3초, 뒤 0.5초)만 남깁니다.
        assert!(
            (3.0..4.0).contains(&seconds),
            "앞뒤 무음을 잘라 내야 합니다: {content:?}"
        );
    }

    #[test]
    fn recording_without_speech_is_not_transcribed() {
        let mut h = Harness::new("들리지 않은 말", Mic::Silent);
        h.send(HotkeyEvent::Toggle);
        h.send(HotkeyEvent::Toggle);
        h.wait_until("말소리 없음", |h| !h.notices.is_empty());
        assert!(
            matches!(h.notices.as_slice(), [Notice::NoSpeech]),
            "{:?}",
            h.notices
        );
        assert_eq!(
            h.clipboard.content(),
            ["원래 내용"],
            "클립보드는 건드리지 않음"
        );
        assert_eq!(h.cues(), [Cue::Start, Cue::Fail], "인식을 시작하지 않음");
        assert_eq!(h.dictation.state(), DictationState::Idle);
    }

    #[test]
    fn short_push_to_talk_press_is_ignored() {
        let mut h = Harness::new("글", Mic::Speech);
        h.send(HotkeyEvent::Press);
        h.send(HotkeyEvent::Release);
        // 다른 키와 함께 누른 경우도 마찬가지입니다.
        h.send(HotkeyEvent::Press);
        h.send(HotkeyEvent::Cancel);
        std::thread::sleep(PUSH_TO_TALK_CONFIRM + Duration::from_millis(100));
        h.wait_until("상태 확인", |h| {
            h.dictation.state() == DictationState::Idle
        });
        assert!(h.cues().is_empty(), "짧게 누르면 소리도 내지 않음");
        assert!(h.notices.is_empty());
        assert_eq!(h.clipboard.content(), ["원래 내용"]);
    }

    #[test]
    fn held_push_to_talk_records_until_release() {
        let mut h = Harness::new("누르는 동안", Mic::Speech);
        h.send(HotkeyEvent::Press);
        h.wait_until("녹음 확정", |h| {
            h.dictation.state() == DictationState::Recording
        });
        assert_eq!(h.cues(), [Cue::Start]);
        h.send(HotkeyEvent::Release);
        h.wait_until("복사", |h| h.clipboard.content() == ["text:누르는 동안"]);
        // 끝낼 때 아직 붙여넣지 않았으면 원래 내용으로 되돌립니다.
        h.dictation.shutdown();
        assert_eq!(h.clipboard.content(), ["원래 내용"]);
    }

    #[test]
    fn microphone_failure_is_reported() {
        let mut h = Harness::new("글", Mic::Broken);
        h.send(HotkeyEvent::Toggle);
        h.wait_until("녹음 실패", |h| !h.notices.is_empty());
        assert!(matches!(
            h.notices.as_slice(),
            [Notice::RecordFailed(AudioError::NoInputDevice)]
        ));
        assert_eq!(h.cues(), [Cue::Fail]);
        assert!(h.entries.is_empty(), "녹음에 실패하면 기록하지 않음");

        // 누르는 동안 모드는 짧게 눌렀으면 알리지 않습니다.
        h.notices.clear();
        h.send(HotkeyEvent::Press);
        h.send(HotkeyEvent::Release);
        std::thread::sleep(PUSH_TO_TALK_CONFIRM + Duration::from_millis(100));
        h.wait_until("상태 확인", |h| {
            h.dictation.state() == DictationState::Idle
        });
        assert!(h.notices.is_empty());
    }

    #[test]
    fn sounds_can_be_turned_off() {
        let mut h = Harness::new("조용히", Mic::Speech);
        let mut quiet = settings("조용히");
        quiet.feedback_sounds = false;
        h.dictation.update_settings(quiet);
        h.send(HotkeyEvent::Toggle);
        h.send(HotkeyEvent::Toggle);
        h.wait_until("복사", |h| h.clipboard.content() == ["text:조용히"]);
        assert!(h.cues().is_empty());
    }
    #[test]
    fn refined_text_is_copied_instead_of_the_transcript() {
        let mut h = Harness::with(refining_settings(" 안녕 하세요 ", ""), Mic::Speech);
        h.send(HotkeyEvent::Toggle);
        h.send(HotkeyEvent::Toggle);
        h.wait_until("다듬은 글 복사", |h| !h.notices.is_empty());
        assert!(
            matches!(h.notices.as_slice(), [Notice::Copied { refined: true }]),
            "{:?}",
            h.notices
        );
        assert_eq!(h.clipboard.content(), ["text:[mock] 안녕 하세요"]);
        assert_eq!(h.cues(), [Cue::Start, Cue::Pop, Cue::Done]);
        assert_eq!(h.entries.len(), 1);
        assert_eq!(
            h.entries[0].transcript, "안녕 하세요",
            "다듬기 전 원문도 남김"
        );
        assert_eq!(
            h.entries[0].refine,
            Refinement::Done {
                text: "[mock] 안녕 하세요".into(),
                prompt: "기본 다듬기".into(),
            }
        );
    }

    #[test]
    fn refinement_failure_puts_the_reason_on_the_clipboard() {
        rust_i18n::set_locale("ko");
        let mut h = Harness::with(refining_settings("고양이가 좋아요", "fail"), Mic::Speech);
        h.send(HotkeyEvent::Toggle);
        h.send(HotkeyEvent::Toggle);
        h.wait_until("오류 안내 복사", |h| !h.notices.is_empty());
        assert!(
            matches!(
                h.notices.as_slice(),
                [Notice::RefineFailed(RefineError::Cloud(LlmError::Http {
                    status: 401,
                    ..
                }))]
            ),
            "{:?}",
            h.notices
        );
        let content = h.clipboard.content();
        assert_eq!(content.len(), 1, "{content:?}");
        let copied = content[0].strip_prefix("text:").unwrap();
        assert!(
            copied.starts_with("[술술 오류]"),
            "받아쓴 글로 착각하지 않게 오류 안내임을 밝혀야 합니다: {copied}"
        );
        assert!(
            copied.contains("401"),
            "실패 원인이 들어 있어야 합니다: {copied}"
        );
        assert!(!copied.contains("고양이"), "원문은 넣지 않습니다: {copied}");
        assert_eq!(h.cues(), [Cue::Start, Cue::Pop, Cue::Fail]);

        // 원문은 클립보드 대신 기록에 남고, 다듬기에 실패했다는 표시가 붙습니다.
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].transcript, "고양이가 좋아요");
        match &h.entries[0].refine {
            Refinement::Failed { prompt, reason } => {
                assert_eq!(prompt, "기본 다듬기");
                assert!(reason.contains("401"), "실패 원인을 남깁니다: {reason}");
            }
            other => panic!("다듬기 실패로 남아야 합니다: {other:?}"),
        }

        // 붙여넣으면 받아쓴 글처럼 원래 클립보드 내용으로 되돌립니다.
        h.send(HotkeyEvent::Paste);
        h.wait_until("원래 내용으로 되돌림", |h| {
            h.clipboard.content() == ["원래 내용"]
        });
    }

    #[test]
    fn empty_transcript_is_not_sent_for_refinement() {
        let mut h = Harness::with(refining_settings("   ", "fail"), Mic::Speech);
        h.send(HotkeyEvent::Toggle);
        h.send(HotkeyEvent::Toggle);
        h.wait_until("인식 결과", |h| !h.notices.is_empty());
        assert!(matches!(h.notices.as_slice(), [Notice::Empty]));
        assert_eq!(h.clipboard.content(), ["원래 내용"]);
        assert!(h.entries.is_empty());
    }

    #[test]
    fn clipboard_failure_still_keeps_the_entry() {
        let mut h = Harness::new("복사 못 한 글", Mic::Speech);
        h.clipboard.0.lock().unwrap().fail_set_after_clear = true;
        h.send(HotkeyEvent::Toggle);
        h.send(HotkeyEvent::Toggle);
        h.wait_until("복사 실패", |h| !h.notices.is_empty());
        assert!(
            matches!(h.notices.as_slice(), [Notice::ClipboardFailed(_)]),
            "{:?}",
            h.notices
        );
        // 기록에서 다시 복사할 수 있게 남깁니다.
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].transcript, "복사 못 한 글");
    }
}
