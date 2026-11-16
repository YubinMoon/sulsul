//! 받아쓰기 오버레이: 단축키 받아쓰기가 녹음하고 인식하는 동안 화면 아래 가운데에 물방울을 띄웁니다.
//!
//! 물방울은 미리보기와 같은 셰이더(`prototypes/liquid-droplet/water.frag`)로 그립니다.
//! 녹음이 시작되면 수면에서 떠올라 떠다니고, 녹음이 끝나면 터져서 작은 물방울들이 고리를 이뤄 돌며 인식을 기다립니다.
//! 인식이 끝나면(성공이든 실패든) 물방울들이 가운데로 모여 뿅 하고 사라지고, 그다음에 창을 닫습니다.
//! 렌즈(물방울 너머로 휘어 보이는 뒤 화면)는 넣지 않았습니다.
//!
//! wgpu 창을 바로 투명하게 하려면 앱 전체의 그리기 방식을 바꿔야 해서, 앱 창과 같은 GPU 장치로 화면 밖 그림에
//! 그린 뒤 그 그림을 클릭이 통과하고 초점을 가져가지 않는 투명 창에 올립니다.
//! 그리기와 창은 전용 스레드(`sulsul-overlay`)가 맡고, 화면 스레드는 [`Overlay::update`]로 상태만 넘깁니다.
//! Windows만 지원합니다.

mod render;
mod shader;
#[cfg(windows)]
mod windows;

use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::session::DictationState;

/// 물방울 크기를 정하는 무대 높이 (논리 픽셀). 미리보기에서 높이 800px 화면일 때 지름이 약 74px입니다.
#[cfg_attr(not(windows), allow(dead_code))]
const STAGE_HEIGHT: f32 = 800.0;
/// 창이 덮는 셰이더 좌표의 범위. 셰이더는 무대 높이를 4로 두고 가운데를 원점으로 씁니다.
/// 가로는 셰이더가 물방울 둘레를 계산하는 범위(중심에서 0.6)와 떠다니는 폭을,
/// 세로는 무대 아래 끝(-2)에서 물방울과 빛 번짐의 위쪽 끝까지를 덮습니다.
#[cfg_attr(not(windows), allow(dead_code))]
const SPAN_X: f32 = 1.4;
#[cfg_attr(not(windows), allow(dead_code))]
const SPAN_Y: f32 = 0.95;
/// 움직임 줄이기를 켰을 때 멈춰 보여 줄 시각 (초, 등장이 끝난 순간)
#[cfg_attr(not(windows), allow(dead_code))]
const STILL_TIME: f32 = 0.45;
/// 터지는 동작의 길이 (초). 셰이더의 `POP_LENGTH`와 같아야 합니다.
#[cfg_attr(not(windows), allow(dead_code))]
const POP_LENGTH: f32 = 0.75;
/// 인식이 끝난 뒤 물방울들이 모여 뿅 하고 사라지기까지의 길이 (초). 셰이더의 `DONE_LENGTH`와 같아야 합니다.
#[cfg_attr(not(windows), allow(dead_code))]
const DONE_LENGTH: f32 = 0.62;

#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    #[error("the recording overlay is not supported on this platform")]
    /// 지원하지 않는 OS에서만 만듭니다.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
    #[error("the recording overlay needs the wgpu renderer")]
    #[cfg_attr(not(windows), allow(dead_code))]
    NoRenderer,
    #[error("could not start the overlay thread: {0}")]
    #[cfg_attr(not(windows), allow(dead_code))]
    Thread(std::io::Error),
}

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    /// 녹음 시작: 창을 열고 수면에서 떠오르는 등장부터 재생합니다. 이미 떠 있으면 처음부터 다시 엽니다.
    Start,
    /// 녹음 끝, 인식 시작: 터지고 작은 물방울들이 고리를 이뤄 돕니다.
    Pop,
    /// 인식 끝: 아직 터지지 않았으면 터뜨리고, 물방울들을 모아 뿅 하고 없앤 뒤 창을 닫습니다.
    Finish,
    Shutdown,
}

/// 오버레이가 따라가는 받아쓰기 단계
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Recording,
    Transcribing,
}

impl Phase {
    fn of(state: DictationState) -> Self {
        match state {
            DictationState::Recording => Self::Recording,
            DictationState::Transcribing => Self::Transcribing,
            DictationState::Idle | DictationState::WaitingPaste => Self::Idle,
        }
    }
}

/// 단계가 바뀔 때 오버레이 스레드에 보낼 명령
fn command_for(from: Phase, to: Phase) -> Option<Command> {
    match (from, to) {
        (_, Phase::Recording) if from != to => Some(Command::Start),
        (Phase::Recording, Phase::Transcribing) => Some(Command::Pop),
        (Phase::Recording | Phase::Transcribing, Phase::Idle) => Some(Command::Finish),
        // 녹음 없이 인식만 하는 경우(띄운 적 없음)와 같은 단계는 그대로 둡니다.
        _ => None,
    }
}

/// 오버레이 스레드 손잡이. 버리면 창을 닫고 스레드를 끝냅니다.
pub struct Overlay {
    commands: Sender<Command>,
    /// 마지막으로 반영한 받아쓰기 단계
    phase: Phase,
    thread: Option<JoinHandle<()>>,
}

impl Overlay {
    /// 앱 창이 쓰는 GPU 장치로 오버레이 스레드를 시작합니다. 셰이더 컴파일은 그 스레드에서 합니다.
    pub fn start(
        render_state: Option<&eframe::egui_wgpu::RenderState>,
    ) -> Result<Self, OverlayError> {
        #[cfg(windows)]
        {
            let state = render_state.ok_or(OverlayError::NoRenderer)?;
            let device = state.device.clone();
            let queue = state.queue.clone();
            let (commands, rx) = mpsc::channel();
            let thread = std::thread::Builder::new()
                .name("sulsul-overlay".into())
                .spawn(move || windows::run(device, queue, rx))
                .map_err(OverlayError::Thread)?;
            Ok(Self {
                commands,
                phase: Phase::Idle,
                thread: Some(thread),
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (render_state, mpsc::channel::<Command>);
            Err(OverlayError::Unsupported)
        }
    }

    /// 받아쓰기 상태를 반영합니다. 녹음을 시작하면 띄우고, 인식이 끝나면 사라지는 동작 뒤에 닫습니다.
    pub fn update(&mut self, state: DictationState) {
        let phase = Phase::of(state);
        if let Some(command) = command_for(self.phase, phase) {
            let _ = self.commands.send(command);
        }
        self.phase = phase;
    }

    /// 창을 닫고 스레드가 끝날 때까지 기다립니다.
    pub fn shutdown(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 셰이더에 넘길 시각들 (초). 음수는 아직 일어나지 않은 동작입니다.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct Moment {
    time: f32,
    pop: f32,
    done: f32,
}

/// 창이 떠 있는 동안 녹음 시작, 터짐, 인식 끝이 언제 일어났는지
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
struct Timeline {
    started: Instant,
    popped: Option<Instant>,
    done: Option<Instant>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Timeline {
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            popped: None,
            done: None,
        }
    }

    fn pop(&mut self, now: Instant) {
        self.popped.get_or_insert(now);
    }

    /// 인식이 끝났습니다. 녹음에서 바로 끝났으면(실패 등) 터짐도 함께 시작합니다.
    fn finish(&mut self, now: Instant) {
        self.pop(now);
        self.done.get_or_insert(now);
    }

    /// `now`에 그릴 장면. 사라지는 동작까지 다 끝났으면 `None`입니다.
    /// `still`(움직임 줄이기)이면 등장이 끝난 모습이나 고리가 다 모인 모습으로 멈추고, 인식이 끝나면 바로 사라집니다.
    fn moment(&self, now: Instant, still: bool) -> Option<Moment> {
        let since = |at: Option<Instant>| at.map(|at| now.duration_since(at).as_secs_f32());
        let (pop, done) = (since(self.popped), since(self.done));
        if still {
            return done.is_none().then_some(Moment {
                time: STILL_TIME,
                pop: if pop.is_some() { POP_LENGTH } else { -1.0 },
                done: -1.0,
            });
        }
        if let (Some(pop), Some(done)) = (pop, done)
            && pop >= POP_LENGTH
            && done >= DONE_LENGTH
        {
            return None;
        }
        Some(Moment {
            time: now.duration_since(self.started).as_secs_f32(),
            pop: pop.unwrap_or(-1.0),
            done: done.unwrap_or(-1.0),
        })
    }
}

/// 작업 영역 (작업 표시줄을 뺀 모니터 영역, 물리 픽셀)
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkArea {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// 오버레이 창의 자리와 크기 (물리 픽셀)
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    /// 셰이더의 무대 높이
    stage_height: f32,
}

/// 작업 영역 아래 가운데에 창을 놓습니다. 물방울은 창 아래 끝(작업 표시줄 바로 위)의 수면에서 떠오릅니다.
/// `scale`은 모니터 배율(1.0 = 100%)입니다.
#[cfg_attr(not(windows), allow(dead_code))]
fn place(work: WorkArea, scale: f32) -> Placement {
    let stage_height = STAGE_HEIGHT * scale.max(0.5);
    let unit = stage_height / 4.0;
    let width = (SPAN_X * unit).round() as u32;
    let height = (SPAN_Y * unit).round() as u32;
    let x = work.left + (work.right - work.left - width as i32) / 2;
    Placement {
        x,
        y: work.bottom - height as i32,
        width,
        height,
        stage_height,
    }
}

/// 셰이더가 낸 RGBA(알파를 곱하지 않은 값)를 Windows 투명 창이 받는 BGRA(알파를 곱한 값)로 바꿉니다.
#[cfg_attr(not(windows), allow(dead_code))]
fn to_premultiplied_bgra(rgba: &[u8], bgra: &mut [u8]) {
    for (&[r, g, b, a], dst) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(bgra.as_chunks_mut::<4>().0)
    {
        let alpha = u16::from(a);
        let multiply = |channel: u8| ((u16::from(channel) * alpha + 127) / 255) as u8;
        *dst = [multiply(b), multiply(g), multiply(r), a];
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn overlay_follows_the_dictation_phases() {
        use Phase::*;
        assert_eq!(command_for(Idle, Recording), Some(Command::Start));
        assert_eq!(command_for(Recording, Transcribing), Some(Command::Pop));
        assert_eq!(command_for(Transcribing, Idle), Some(Command::Finish));
        // 녹음이 바로 실패하거나 취소되면 터지고 곧바로 사라집니다.
        assert_eq!(command_for(Recording, Idle), Some(Command::Finish));
        // 사라지는 동안 다시 녹음하면 처음부터 띄웁니다.
        assert_eq!(command_for(Transcribing, Recording), Some(Command::Start));
        // 녹음 없이 인식만 하는 경우는 띄우지 않습니다.
        assert_eq!(command_for(Idle, Transcribing), None);
        assert_eq!(command_for(Recording, Recording), None);
        assert_eq!(Phase::of(DictationState::WaitingPaste), Idle);
    }

    #[test]
    fn timeline_plays_pop_wait_and_finish_in_order() {
        let start = Instant::now();
        let at = |seconds: f32| start + Duration::from_secs_f32(seconds);
        let mut timeline = Timeline::new(start);
        let floating = timeline.moment(at(2.0), false).unwrap();
        assert_eq!((floating.pop, floating.done), (-1.0, -1.0));

        timeline.pop(at(2.0));
        // 인식이 오래 걸리면 고리가 계속 돕니다.
        let waiting = timeline.moment(at(10.0), false).unwrap();
        assert!((waiting.pop - 8.0).abs() < 1e-3 && waiting.done < 0.0);

        timeline.finish(at(10.0));
        let finishing = timeline.moment(at(10.3), false).unwrap();
        assert!((finishing.done - 0.3).abs() < 1e-3);
        assert!(
            (finishing.time - 10.3).abs() < 1e-3,
            "녹음을 시작한 뒤의 시계는 계속 흐름"
        );
        assert!(timeline.moment(at(10.01 + DONE_LENGTH), false).is_none());
    }

    #[test]
    fn timeline_finishes_after_the_pop_even_when_recognition_is_quick() {
        let start = Instant::now();
        let at = |seconds: f32| start + Duration::from_secs_f32(seconds);
        let mut timeline = Timeline::new(start);
        // 녹음이 끝나자마자 실패했습니다: 터짐과 끝이 함께 시작합니다.
        timeline.finish(at(1.0));
        let moment = timeline.moment(at(1.0 + DONE_LENGTH), false).unwrap();
        assert!(moment.pop < POP_LENGTH, "터지는 동작이 끝날 때까지 기다림");
        assert!(timeline.moment(at(1.01 + POP_LENGTH), false).is_none());
    }

    #[test]
    fn still_timeline_holds_poses_and_closes_at_once() {
        let start = Instant::now();
        let mut timeline = Timeline::new(start);
        let floating = timeline.moment(start, true).unwrap();
        assert_eq!((floating.time, floating.pop), (STILL_TIME, -1.0));
        timeline.pop(start);
        assert_eq!(timeline.moment(start, true).unwrap().pop, POP_LENGTH);
        timeline.finish(start);
        assert!(timeline.moment(start, true).is_none());
    }

    const FULL_HD: WorkArea = WorkArea {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1032,
    };

    #[test]
    fn window_sits_at_the_bottom_center_of_the_work_area() {
        let placement = place(FULL_HD, 1.0);
        assert_eq!((placement.width, placement.height), (280, 190));
        assert_eq!(placement.x, (1920 - 280) / 2);
        assert_eq!(placement.y + placement.height as i32, 1032);
        assert_eq!(placement.stage_height, 800.0);
    }

    #[test]
    fn window_follows_the_monitor_scale_and_position() {
        // 주 모니터 아래쪽에 놓인 150% 모니터
        let work = WorkArea {
            left: -320,
            top: 1080,
            right: 2240,
            bottom: 2472,
        };
        let placement = place(work, 1.5);
        assert_eq!((placement.width, placement.height), (420, 285));
        assert_eq!(placement.x, -320 + (2560 - 420) / 2);
        assert_eq!(placement.y + placement.height as i32, 2472);
        assert_eq!(placement.stage_height, 1200.0);
    }

    #[test]
    fn premultiplies_and_swaps_to_bgra() {
        let rgba = [255, 128, 0, 128, 10, 20, 30, 0, 1, 2, 3, 255];
        let mut bgra = [0; 12];
        to_premultiplied_bgra(&rgba, &mut bgra);
        assert_eq!(bgra, [0, 64, 128, 128, 0, 0, 0, 0, 3, 2, 1, 255]);
    }
}
