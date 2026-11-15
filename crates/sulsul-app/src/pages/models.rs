//! 모델 관리 화면: 받아쓰기에 쓸 로컬 모델과 외부 제공자를 고르고, 내려받고, 등록합니다.
//!
//! 화면은 STT / LLM 두 갈래이고, STT 안은 탭 세 개로 나뉩니다.
//!
//! - `entries`: "사용 가능 엔진" 목록의 항목과, 항목을 켜고 지우는 규칙 (화면을 그리지 않음)
//! - `jobs`: 모델 내려받기와 엔진 테스트 작업 스레드
//! - `parts`: 여러 탭이 함께 쓰는 화면 조각
//! - `available` · `download` · `providers`: 탭별 화면
//!
//! 내려받기와 테스트는 이 화면의 상태에 들어 있어, 탭이나 화면을 바꿔도 계속 진행됩니다.

mod available;
mod download;
mod entries;
mod jobs;
mod parts;
mod providers;

use std::time::Duration;

use egui::Ui;
use rust_i18n::t;
use sulsul_core::config::SttConfig;
use sulsul_core::stt::EngineContext;

// 번역 파일 검사(`i18n` 테스트)에서 단계 이름 키를 확인할 때 씁니다.
#[cfg(test)]
pub(crate) use self::download::axis_name_key;

use self::available::PendingConfirm;
use self::download::Picker;
use self::entries::{EntryKey, activate, available_entries, selection_ready};
use self::jobs::{ActiveDownload, ActiveTest, DownloadEnd, TestOutcome, TestTarget};
use self::providers::Draft;
use super::{SECTION_GAP, header};
use crate::session::{PageContext, Shared};
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::controls;
use crate::widgets::tabs::underline_tabs;
use crate::widgets::text;

/// 화면의 큰 갈래
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Category {
    /// 음성 인식
    #[default]
    Stt,
    /// 다듬기 (아직 준비 중)
    Llm,
}

/// STT 갈래 안의 탭
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SttTab {
    /// 지금 쓸 수 있는 모델·제공자 목록
    #[default]
    Available,
    /// 로컬 모델 내려받기
    Download,
    /// 외부 제공자 등록
    Providers,
}

/// 내려받지 못한 모델과 그 이유
struct DownloadFailure {
    engine_id: String,
    variant_id: String,
    /// 화면에 보일 실패 이유
    message: String,
}

#[derive(Default)]
pub struct ModelsPage {
    category: Category,
    tab: SttTab,

    /// "사용 가능 엔진" 탭에서 펼쳐 둔 카드. 한 번에 하나만 펼칩니다.
    expanded: Option<EntryKey>,
    /// 다음에 그릴 때 화면을 옮겨 보여 줄 카드
    reveal: Option<EntryKey>,
    /// 펼친 제공자 카드에서 API 키를 가리지 않고 보여 줄지
    reveal_api_key: bool,

    /// "로컬 모델 내려받기" 탭에서 고른 엔진과 단계 값
    picker: Picker,
    /// "외부 제공자" 탭에서 입력 중인 새 제공자
    draft: Draft,

    download: Option<ActiveDownload>,
    /// 방금 다 받은 모델. 다음에 화면을 그릴 때, 지금 설정으로 받아쓰기를 할 수 없으면 이 모델을 켭니다.
    /// (`tick`에서는 설정을 바꿀 수 없어 화면을 그릴 때까지 미룹니다)
    downloaded: Option<EntryKey>,
    download_failure: Option<DownloadFailure>,

    test: Option<ActiveTest>,
    last_test: Option<TestOutcome>,

    confirm: Option<PendingConfirm>,
}

impl ModelsPage {
    /// 내려받기나 테스트가 진행 중이면 `true`입니다.
    pub fn is_busy(&self) -> bool {
        self.download.is_some() || self.test.is_some()
    }

    /// 내려받기와 테스트 작업 스레드가 끝났는지 확인합니다. 다른 화면을 보고 있을 때도 불립니다.
    pub fn tick(&mut self, shared: &mut Shared, ctx: &egui::Context) {
        let ended = self.download.as_ref().and_then(ActiveDownload::poll);
        if let Some(end) = ended
            && let Some(download) = self.download.take()
        {
            match end {
                DownloadEnd::Done => {
                    shared
                        .toasts
                        .info(t!("models.download.done", model = download.variant_label));
                    self.download_failure = None;
                    self.downloaded =
                        Some(EntryKey::variant(&download.engine_id, &download.variant_id));
                }
                DownloadEnd::Cancelled => {
                    shared.toasts.info(t!("models.download.cancelled"));
                }
                DownloadEnd::Failed(message) => {
                    shared
                        .toasts
                        .error(t!("models.download.failed", message = message));
                    self.download_failure = Some(DownloadFailure {
                        engine_id: download.engine_id,
                        variant_id: download.variant_id,
                        message,
                    });
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

        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        self.use_downloaded(cx);

        header(ui, &t!("models.title"), &t!("models.subtitle"));

        if cx.locked {
            Banner::new(Tone::Warning)
                .line(t!("models.locked"))
                .show(ui);
            ui.add_space(10.0);
        }

        let categories = [
            (Category::Stt, t!("models.category.stt").into_owned()),
            (Category::Llm, t!("models.category.llm").into_owned()),
        ];
        underline_tabs(ui, &mut self.category, &categories);
        ui.add_space(SECTION_GAP);

        match self.category {
            Category::Stt => self.stt_ui(ui, cx),
            Category::Llm => {
                ui.label(text::muted(ui, t!("models.llm.coming_soon")));
            }
        }

        self.confirm_modal(ui, cx);
    }

    fn stt_ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let tabs = [
            (SttTab::Available, t!("models.tab.available").into_owned()),
            (SttTab::Download, t!("models.tab.download").into_owned()),
            (SttTab::Providers, t!("models.tab.providers").into_owned()),
        ];
        controls::segmented(ui, &mut self.tab, &tabs);
        ui.add_space(SECTION_GAP);

        match self.tab {
            SttTab::Available => self.available_tab(ui, cx),
            SttTab::Download => self.download_tab(ui, cx),
            SttTab::Providers => self.providers_tab(ui, cx),
        }
    }

    /// 방금 다 받은 모델이 있고 지금 설정으로는 받아쓰기를 할 수 없으면, 그 모델을 켭니다.
    /// 이미 쓸 수 있는 모델이나 제공자를 골라 두었으면 건드리지 않습니다.
    fn use_downloaded(&mut self, cx: &mut PageContext<'_>) {
        let Some(key) = self.downloaded.take() else {
            return;
        };
        if cx.locked {
            return;
        }
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

    fn is_testing(&self, target: &TestTarget) -> bool {
        self.test
            .as_ref()
            .is_some_and(|test| &test.target == target)
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
