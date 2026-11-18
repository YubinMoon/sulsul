//! 테스트 화면: 단축키 받아쓰기의 흐름(녹음 → 받아쓰기 → 글 다듬기)을 위에서 아래로 한 단계씩 시험합니다.
//!
//! - 녹음(`record`): 마이크로 녹음하고, 단축키 받아쓰기와 같은 오디오 처리(자동 음량 조절, 앞뒤 무음
//!   자르기)를 거친 결과를 보여 줍니다. 입력 장치는 설정 화면에서 고릅니다.
//! - 받아쓰기(`stt`): 방금 녹음한 오디오(또는 WAV 파일)를 설정의 엔진으로 받아씁니다.
//! - 글 다듬기(`refine`): 받아쓴 글을 설정의 모델·제공자와 고른 프롬프트로 다듬습니다.
//!
//! 앞 단계의 결과가 나오면 다음 카드의 입력이 그 결과로 바뀝니다. 입력과 결과가 엇갈리지 않도록 한 번에
//! 한 단계만 실행합니다([`Gate`]). 엔진과 다듬기 모델은 단축키 받아쓰기와 함께 쓰므로, 단축키 받아쓰기가
//! 녹음하거나 받아쓰는 동안에는 받아쓰기·다듬기를 시작하지 않습니다.
//!
//! 이 화면의 결과는 복사 버튼을 누를 때만 클립보드에 넣고, 받아쓰기 기록에는 남기지 않습니다.

mod record;
mod refine;
mod stt;

use std::time::Instant;

use egui::{Frame, Label, Margin, Stroke, Ui};
use rust_i18n::t;

use self::record::RecordCard;
use self::refine::RefineCard;
use self::stt::SttCard;
use super::{SECTION_GAP, header};
use crate::format::seconds_text;
use crate::icons::Icon;
use crate::session::{DictationState, PageContext, Shared};
use crate::theme::{Palette, radius};
use crate::widgets::{controls, text};

#[derive(Default)]
pub struct TestPage {
    record: RecordCard,
    stt: SttCard,
    refine: RefineCard,
}

/// 단계를 지금 시작할 수 있는지
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    Open,
    /// 이 화면의 다른 단계가 실행 중
    OtherStep,
    /// 단축키 받아쓰기가 엔진이나 모델을 쓰는 중
    Dictation,
}

impl Gate {
    fn is_open(self) -> bool {
        self == Self::Open
    }

    /// 시작할 수 없는 이유 (화면 문구)
    fn reason(self) -> Option<String> {
        match self {
            Self::Open => None,
            Self::OtherStep => Some(t!("test.wait_step").into_owned()),
            Self::Dictation => Some(t!("test.wait_dictation").into_owned()),
        }
    }
}

impl TestPage {
    pub fn is_recording(&self) -> bool {
        self.record.is_recording()
    }

    /// 매 프레임 부릅니다(화면이 보이지 않아도). 녹음 최대 길이와 작업 스레드의 결과를 챙깁니다.
    pub fn tick(&mut self, shared: &mut Shared, ctx: &egui::Context) {
        self.record.tick(ctx);
        self.stt.tick(shared);
        self.refine.tick(shared);
        self.follow();
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        header(ui, &t!("test.title"), None);
        let dictation = dictation_busy(cx.shared.dictation.state);

        let gate = step_gate(!self.stt.is_running() && !self.refine.is_running(), false);
        self.record.ui(ui, cx, gate);
        self.follow();
        ui.add_space(SECTION_GAP);

        let gate = step_gate(
            !self.record.is_busy() && !self.refine.is_running(),
            dictation,
        );
        self.stt.ui(ui, cx, gate);
        self.follow();
        ui.add_space(SECTION_GAP);

        let gate = step_gate(!self.record.is_busy() && !self.stt.is_running(), dictation);
        self.refine.ui(ui, cx, gate);
    }

    /// 앞 단계에서 새 결과가 나왔으면 다음 카드의 입력을 그 결과로 바꿉니다.
    fn follow(&mut self) {
        if let Some(recorded) = self.record.take_new() {
            self.stt.use_recording(recorded);
        }
        if let Some(text) = self.stt.take_new_text() {
            self.refine.set_input(text);
        }
    }
}

/// 한 단계의 문: 이 화면의 다른 단계가 모두 쉬고 있고(`others_idle`), 단축키 받아쓰기가 엔진을 쓰지
/// 않을 때(`dictation_busy`가 아님) 열립니다.
fn step_gate(others_idle: bool, dictation_busy: bool) -> Gate {
    if !others_idle {
        Gate::OtherStep
    } else if dictation_busy {
        Gate::Dictation
    } else {
        Gate::Open
    }
}

/// 단축키 받아쓰기가 엔진이나 다듬기 모델을 쓰고 있을 수 있는지 (녹음하는 동안 모델을 미리 올립니다)
fn dictation_busy(state: DictationState) -> bool {
    matches!(
        state,
        DictationState::Recording | DictationState::Transcribing
    )
}

/// 결과 글 상자. 비어 있으면 `empty`를 흐리게 보여 줍니다. 글은 끌어서 고를 수 있습니다.
fn output_box(ui: &mut Ui, output: &str, empty: &str) {
    let palette = Palette::of(ui);
    Frame::new()
        .fill(palette.field)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(radius::CONTROL)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if output.trim().is_empty() {
                ui.label(text::muted(ui, empty));
            } else {
                ui.add(Label::new(output).selectable(true).wrap());
            }
        });
}

/// 결과 글을 클립보드에 복사하는 버튼
fn copy_button(ui: &mut Ui, shared: &mut Shared, output: &str) {
    let enabled = !output.trim().is_empty();
    let clicked = ui
        .add_enabled_ui(enabled, |ui| {
            controls::icon_text_button(ui, Icon::Copy, t!("test.copy")).clicked()
        })
        .inner;
    if clicked {
        ui.ctx().copy_text(output.to_owned());
        shared.toasts.info(t!("common.copied"));
    }
}

/// 실행 중인 단계의 진행 줄: 돌아가는 표시, 경과 시간, 취소 버튼. 취소를 누르면 `true`입니다.
fn progress_row(ui: &mut Ui, started: Instant, cancelling: bool) -> bool {
    let mut cancel = false;
    ui.horizontal(|ui| {
        ui.spinner();
        let status = if cancelling {
            t!("test.cancelling")
        } else {
            t!("test.running", elapsed = seconds_text(started.elapsed()))
        };
        ui.label(text::muted(ui, status));
        ui.add_enabled_ui(!cancelling, |ui| {
            cancel = controls::icon_text_button(ui, Icon::X, t!("common.cancel")).clicked();
        });
    });
    // 돌아가는 표시(spinner)가 화면을 계속 다시 그리게 하므로 경과 시간도 따라 바뀝니다.
    cancel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_is_shared_only_while_dictation_records_or_transcribes() {
        assert!(dictation_busy(DictationState::Recording));
        assert!(dictation_busy(DictationState::Transcribing));
        assert!(!dictation_busy(DictationState::Idle));
        assert!(
            !dictation_busy(DictationState::WaitingPaste),
            "붙여넣기를 기다리는 동안에는 엔진을 쓰지 않습니다"
        );
    }

    #[test]
    fn steps_wait_for_each_other_before_dictation() {
        assert_eq!(step_gate(true, false), Gate::Open);
        assert_eq!(step_gate(false, true), Gate::OtherStep);
        assert_eq!(step_gate(true, true), Gate::Dictation);
        assert!(Gate::Open.reason().is_none());
        assert!(Gate::Dictation.reason().is_some());
    }
}
