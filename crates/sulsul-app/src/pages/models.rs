//! 모델 관리 화면: 받아쓰기와 글 다듬기에 쓸 로컬 모델과 외부 제공자를 고르고, 내려받고, 등록합니다.
//!
//! 화면은 "음성 인식 / 글 다듬기" 두 갈래이고, 갈래마다 같은 탭 세 개가 있습니다
//! (사용 가능 모델 / 로컬 모델 내려받기 / 외부 제공자).
//!
//! - `entries`: 음성 인식 "사용 가능 모델" 목록의 항목과, 항목을 켜고 지우는 규칙 (화면을 그리지 않음).
//!   글 다듬기의 같은 규칙은 `sulsul_llm::entries`에 있습니다.
//! - `profiles`: 외부 제공자를 다루는 갈래 공통 규칙 (화면을 그리지 않음)
//! - `jobs`: 모델 내려받기와 엔진 테스트 작업 스레드
//! - `parts`: 두 갈래의 탭이 함께 쓰는 화면 조각
//! - `available` · `download` · `providers`: 음성 인식 갈래의 탭별 화면. 내려받기 카드와 제공자
//!   등록 칸처럼 두 갈래가 함께 쓰는 탭 조각도 여기에 있습니다.
//! - `llm`: 글 다듬기 갈래
//!
//! 내려받기와 테스트는 이 화면의 상태에 들어 있어, 탭이나 갈래, 화면을 바꿔도 계속 진행됩니다.
//! 내려받기는 두 갈래를 통틀어 한 번에 하나만 합니다.

mod available;
mod download;
mod entries;
mod jobs;
mod llm;
mod parts;
mod profiles;
mod providers;

use std::time::Duration;

use egui::Ui;
use rust_i18n::t;
use sulsul_core::config::SttConfig;
use sulsul_core::stt::EngineContext;

// 번역 파일 검사(`i18n` 테스트)에서 단계 이름 키를 확인할 때 씁니다.
#[cfg(test)]
pub(crate) use self::download::axis_name_key;
pub(crate) use self::entries::variant_label;
pub(crate) use self::llm::issue_text as llm_issue_text;
pub(crate) use self::profiles::profile_display_name;

use self::available::PendingConfirm;
use self::download::Picker;
use self::entries::{EntryKey, activate, available_entries, selection_ready};
use self::jobs::{ActiveTest, DownloadEnd, DownloadTarget, Downloads, TestOutcome, TestTarget};
use self::llm::LlmPane;
use self::parts::{ListState, TestState};
use self::providers::Draft;
use super::{SECTION_GAP, header};
use crate::session::{PageContext, Shared};
use crate::widgets::controls;
use crate::widgets::tabs::underline_tabs;

/// 화면의 큰 갈래
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Category {
    /// 음성 인식
    #[default]
    Stt,
    /// 글 다듬기
    Llm,
}

/// 갈래 안의 탭. 두 갈래가 같은 탭 세 개를 씁니다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    /// 지금 쓸 수 있는 모델·제공자 목록
    #[default]
    Available,
    /// 로컬 모델 내려받기
    Download,
    /// 외부 제공자 등록
    Providers,
}

/// 갈래 안의 탭 줄
fn tab_bar(ui: &mut Ui, tab: &mut Tab) {
    let tabs = [
        (Tab::Available, t!("models.tab.available").into_owned()),
        (Tab::Download, t!("models.tab.download").into_owned()),
        (Tab::Providers, t!("models.tab.providers").into_owned()),
    ];
    controls::segmented(ui, tab, &tabs);
    ui.add_space(SECTION_GAP);
}

#[derive(Default)]
pub struct ModelsPage {
    category: Category,

    /// 음성 인식 갈래에서 보고 있는 탭
    tab: Tab,
    /// 음성 인식 "사용 가능 모델" 목록의 펼침 상태
    list: ListState<EntryKey>,
    /// 음성 인식 "로컬 모델 내려받기" 탭에서 고른 엔진과 단계 값
    picker: Picker,
    /// 같은 탭의 "가지고 있는 모델 파일 쓰기"를 펼쳐 두었는지
    own_file_open: bool,
    /// 음성 인식 "외부 제공자" 탭에서 입력 중인 새 제공자
    draft: Draft,
    /// 음성 인식 엔진 테스트
    test: Option<ActiveTest>,
    last_test: Option<TestOutcome>,
    /// 음성 인식 갈래에서 확인을 받고 있는 제거
    confirm: Option<PendingConfirm>,

    /// 글 다듬기 갈래
    llm: LlmPane,

    /// 모델 내려받기. 두 갈래가 함께 씁니다.
    downloads: Downloads,
    /// 방금 다 받은 모델. 다음에 화면을 그릴 때, 그 갈래에서 지금 설정으로 쓸 수 있는 것이 없으면
    /// 이 모델을 켭니다. (`tick`에서는 설정을 바꿀 수 없어 화면을 그릴 때까지 미룹니다)
    downloaded: Option<DownloadTarget>,
}

impl ModelsPage {
    /// 내려받기나 테스트가 진행 중이면 `true`입니다.
    pub fn is_busy(&self) -> bool {
        self.downloads.active().is_some() || self.test.is_some() || self.llm.is_busy()
    }

    /// 내려받기와 테스트 작업 스레드가 끝났는지 확인합니다. 다른 화면을 보고 있을 때도 불립니다.
    pub fn tick(&mut self, shared: &mut Shared, ctx: &egui::Context) {
        let card_shown = self.downloads.take_card_shown();
        if let Some((download, end)) = self.downloads.take_finished() {
            match end {
                DownloadEnd::Done => {
                    shared
                        .toasts
                        .info(t!("models.download.done", model = download.variant_label));
                    self.downloaded = Some(download.target);
                }
                // 취소는 사용자가 한 일이라 알리지 않습니다.
                DownloadEnd::Cancelled => {}
                // 원인은 내려받기 카드에 남으므로, 카드를 보고 있었으면 알리지 않고
                // 다른 곳을 보고 있었으면 어느 모델인지만 알립니다.
                DownloadEnd::Failed(_) if card_shown => {}
                DownloadEnd::Failed(_) => {
                    shared.toasts.error(t!(
                        "models.download.failed_named",
                        model = download.variant_label
                    ));
                }
            }
        }

        let tested = self.test.as_ref().and_then(ActiveTest::poll);
        if let Some(result) = tested
            && let Some(test) = self.test.take()
        {
            if test.target == TestTarget::Draft {
                self.draft.finish_test(result.is_ok());
            }
            self.last_test = Some(TestOutcome {
                target: test.target,
                result,
                accel: test.accel,
                kind: test.kind,
            });
        }

        self.llm.tick();

        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        self.use_downloaded(cx);

        // 설정 파일을 읽지 못해 잠겨 있으면 앱 위쪽 배너가 알리므로 여기서는 따로 알리지 않습니다.
        header(ui, &t!("models.title"), None);

        let categories = [
            (Category::Stt, t!("models.category.stt").into_owned()),
            (Category::Llm, t!("models.category.llm").into_owned()),
        ];
        underline_tabs(ui, &mut self.category, &categories);
        ui.add_space(SECTION_GAP);

        match self.category {
            Category::Stt => self.stt_ui(ui, cx),
            Category::Llm => self.llm.ui(ui, cx, &mut self.downloads),
        }

        self.confirm_modal(ui, cx);
        self.llm.confirm_modal(ui, cx);
    }

    fn stt_ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        tab_bar(ui, &mut self.tab);
        match self.tab {
            Tab::Available => self.available_tab(ui, cx),
            Tab::Download => self.download_tab(ui, cx),
            Tab::Providers => self.providers_tab(ui, cx),
        }
    }

    /// 방금 다 받은 모델이 있고 그 갈래에서 지금 설정으로 쓸 수 있는 것이 없으면, 그 모델을 켭니다.
    /// 이미 쓸 수 있는 모델이나 제공자를 골라 두었으면 건드리지 않습니다.
    fn use_downloaded(&mut self, cx: &mut PageContext<'_>) {
        let Some(target) = self.downloaded.take() else {
            return;
        };
        if cx.locked {
            return;
        }
        match target {
            DownloadTarget::Stt {
                engine_id,
                variant_id,
            } => {
                let key = EntryKey::variant(&engine_id, &variant_id);
                let engine_ctx = EngineContext {
                    paths: cx.shared.paths.clone(),
                };
                let entries = available_entries(&cx.shared.registry, &engine_ctx, &cx.config.stt);
                if selection_ready(&entries) {
                    return;
                }
                if entries.iter().any(|entry| entry.key == key && entry.ready) {
                    activate(&cx.shared.registry, &mut cx.config.stt, &key);
                }
            }
            DownloadTarget::Llm { variant_id } => llm::use_downloaded(cx, &variant_id),
        }
    }

    /// `target`의 테스트 버튼이 보일 모습
    fn test_state(&self, target: &TestTarget) -> TestState {
        TestState::of(self.test.as_ref().map(|test| &test.target), target)
    }

    fn is_testing(&self, target: &TestTarget) -> bool {
        self.test_state(target) == TestState::Running
    }

    fn cancel_test(&self) {
        if let Some(test) = &self.test {
            test.cancel();
        }
    }

    /// `stt` 설정으로 `engine_id` 엔진을 테스트하는 작업 스레드를 띄웁니다.
    /// 테스트는 한 번에 하나만 돌립니다. 시작했으면 `true`를 돌려줍니다.
    fn start_test(
        &mut self,
        shared: &Shared,
        egui_ctx: &egui::Context,
        target: TestTarget,
        stt: SttConfig,
        engine_id: String,
    ) -> bool {
        if self.test.is_some() {
            return false;
        }
        self.last_test = None;
        self.test = ActiveTest::start(shared, egui_ctx, target, stt, engine_id);
        self.test.is_some()
    }

    /// `target`의 지난 테스트 결과를 지웁니다.
    fn forget_test(&mut self, target: &TestTarget) {
        if self
            .last_test
            .as_ref()
            .is_some_and(|outcome| &outcome.target == target)
        {
            self.last_test = None;
        }
    }
}
