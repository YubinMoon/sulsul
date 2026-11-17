//! 테스트 화면: 마이크 녹음과 받아쓰기가 제대로 되는지 한곳에서 확인합니다.
//!
//! 위쪽은 녹음 테스트(입력 음량과 방금 녹음한 오디오), 아래쪽은 받아쓰기 테스트(방금 녹음한
//! 오디오나 WAV 파일을 엔진에 넣어 결과와 속도 확인)입니다. 입력 장치는 설정 화면에서 고릅니다.

mod record;
mod transcribe;

use egui::Ui;
use rust_i18n::t;

use self::record::RecordTest;
use self::transcribe::TranscribeTest;
use super::{SECTION_GAP, header};
use crate::session::{PageContext, Shared};

#[derive(Default)]
pub struct TestPage {
    record: RecordTest,
    transcribe: TranscribeTest,
}

impl TestPage {
    pub fn is_recording(&self) -> bool {
        self.record.is_recording()
    }

    /// 매 프레임 부릅니다(화면이 보이지 않아도). 녹음 최대 길이와 받아쓰기 결과를 챙깁니다.
    pub fn tick(&mut self, shared: &mut Shared, ctx: &egui::Context) {
        self.record.tick(shared, ctx);
        self.follow_new_recording();
        self.transcribe.tick(shared);
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        header(ui, &t!("test.title"), &t!("test.subtitle"));

        self.record.ui(ui, cx);
        self.follow_new_recording();
        ui.add_space(SECTION_GAP);
        self.transcribe.ui(ui, cx);
    }

    /// 새로 녹음했으면 받아쓸 오디오를 그 녹음으로 바꿉니다.
    /// 녹음하자마자 아래에서 바로 받아쓰기를 누를 수 있게 하려는 것입니다.
    fn follow_new_recording(&mut self) {
        if self.record.take_new_recording() {
            self.transcribe.use_recording();
        }
    }
}
