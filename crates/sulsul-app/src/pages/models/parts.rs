//! 모델 관리 화면의 여러 탭이 함께 쓰는 화면 조각.
//!
//! 음성 인식과 글 다듬기 갈래가 같은 조각으로 화면을 그립니다: 목록 카드와 펼침 상태, 빈 목록 안내,
//! 테스트·제거 버튼, 테스트 진행 상황·결과, 예상 장치 칸, 내려받기 진행 상황, 제공자 입력 칸.

use egui::{
    Align, Color32, CornerRadius, Layout, Rect, Response, RichText, Role, Sense, Spinner, Stroke,
    StrokeKind, TextEdit, Ui, WidgetInfo, vec2,
};
use rust_i18n::t;
use sulsul_core::config::{ApiProfile, ReasoningEffort};
use sulsul_core::download::DownloadPhase;
use sulsul_core::stt::{
    ComputeDevice, EngineKind, LanguageSupport, WHISPER_LANGUAGES, format_bytes,
};

use super::jobs::{ActiveTest, DownloadSnapshot, TestFailure, TestOutcome, TestTarget};
use super::{Category, profiles};
use crate::accel::{self, AccelEnv, EngineAccel};
use crate::format;
use crate::icons::{self, Icon};
use crate::pages::SECTION_GAP;
use crate::theme::{self, Palette, radius, size};
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card;
use crate::widgets::confirm::{Answer, Confirm};
use crate::widgets::controls::{self, ComboItem};
use crate::widgets::expander::Expander;
use crate::widgets::language::language_name;
use crate::widgets::row::{self, Row};
use crate::widgets::text;
use crate::widgets::tile;

/// 목록의 카드 사이 간격
pub const CARD_GAP: f32 = 4.0;
/// 펼친 카드 안에서 묶음 사이 간격
pub const BODY_GAP: f32 = 12.0;
/// 정보 줄([`fact`])에서 항목 이름이 차지하는 너비
const FACT_LABEL_WIDTH: f32 = 64.0;
/// 내려받기 진행 막대의 높이
const PROGRESS_BAR_HEIGHT: f32 = 8.0;
/// 카드로 화면을 옮길 때 카드 위에 남기는 여백
const REVEAL_MARGIN: f32 = 12.0;

/// 카드 머리 줄의 배지
pub struct Badge {
    text: String,
    tone: BadgeTone,
}

#[derive(Clone, Copy)]
enum BadgeTone {
    Plain,
    /// 추천 모델
    Accent,
    /// 지금은 쓸 수 없거나 덜 끝난 상태
    Warning,
    /// 설치됨
    Success,
}

impl Badge {
    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(text, BadgeTone::Plain)
    }

    pub fn accent(text: impl Into<String>) -> Self {
        Self::new(text, BadgeTone::Accent)
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Self::new(text, BadgeTone::Warning)
    }

    pub fn success(text: impl Into<String>) -> Self {
        Self::new(text, BadgeTone::Success)
    }

    fn new(text: impl Into<String>, tone: BadgeTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }

    pub fn show(&self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        let (fill, color) = match self.tone {
            BadgeTone::Plain => (palette.control, palette.text_muted),
            BadgeTone::Accent => (palette.accent_soft, palette.accent_text),
            BadgeTone::Warning => (palette.warning_soft, palette.warning_text),
            BadgeTone::Success => (palette.success_soft, palette.success_text),
        };
        controls::tone_badge(ui, &self.text, fill, color);
    }
}

/// 목록 카드의 머리 줄에 보일 내용: 이름, 배지, 짧은 설명
pub struct CardHead {
    pub name: String,
    pub badges: Vec<Badge>,
    pub description: Option<String>,
}

impl CardHead {
    fn show(&self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 3.0;
        row::title_line(ui, |ui| {
            ui.label(text::strong(&self.name));
            for badge in &self.badges {
                badge.show(ui);
            }
        });
        if let Some(description) = &self.description {
            ui.label(text::caption(ui, description));
        }
    }
}

/// 등록한 제공자 카드의 머리 줄: 이름, 공급자, 값이 덜 채워졌으면 그 표시, 모델 이름.
pub fn provider_head(category: Category, profile: &ApiProfile, ready: bool) -> CardHead {
    let mut badges = vec![Badge::plain(profiles::provider_label(
        category,
        &profile.provider,
    ))];
    if !ready {
        badges.push(Badge::warning(t!(
            "models.available.provider.incomplete_badge"
        )));
    }
    let model = profile.model.trim();
    CardHead {
        name: profiles::profile_display_name(profile),
        badges,
        description: (!model.is_empty()).then(|| model.to_owned()),
    }
}

/// 목록 카드 하나를 그린 결과
pub struct CardOutcome {
    /// 머리 줄을 눌러 펼치거나 접었는지
    toggled: bool,
    /// 스위치를 눌러 이 항목을 켰는지
    pub turned_on: bool,
    response: Response,
}

/// "사용 가능 모델" 목록의 카드 하나: 머리 줄, 쓸 항목을 고르는 스위치, 펼쳤을 때의 내용.
///
/// - `open`: 지금 펼쳐져 있는지 ([`ListState::is_open`])
/// - `shown_on`: 스위치를 켜진 모습으로 보여 줄지
/// - `can_turn_on`: 스위치를 누를 수 있는지
///
/// 그린 뒤에는 결과를 [`ListState::after_card`]에 넘겨 펼침 상태를 맞춥니다.
pub fn entry_card(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    head: &CardHead,
    open: bool,
    shown_on: bool,
    can_turn_on: bool,
    add_body: impl FnOnce(&mut Ui),
) -> CardOutcome {
    let toggle_label = t!("models.available.use", name = head.name);
    let mut turned_on = false;
    let shown = Expander::new(&head.name, open).selected(shown_on).show(
        ui,
        id_salt,
        |ui| head.show(ui),
        |ui| {
            ui.add_enabled_ui(can_turn_on, |ui| {
                turned_on = controls::choice_toggle(ui, shown_on, &toggle_label);
            });
        },
        add_body,
    );
    CardOutcome {
        toggled: shown.toggled,
        turned_on,
        response: shown.response,
    }
}

/// "사용 가능 모델" 목록의 펼침 상태. 갈래마다 하나씩 둡니다.
pub struct ListState<K> {
    /// 펼쳐 둔 카드. 한 번에 하나만 펼칩니다.
    expanded: Option<K>,
    /// 다음에 그릴 때 화면을 옮겨 보여 줄 카드
    reveal: Option<K>,
    /// 펼친 제공자 카드에서 API 키를 가리지 않고 보여 줄지
    pub reveal_api_key: bool,
}

impl<K> Default for ListState<K> {
    fn default() -> Self {
        Self {
            expanded: None,
            reveal: None,
            reveal_api_key: false,
        }
    }
}

impl<K: PartialEq + Clone> ListState<K> {
    pub fn is_open(&self, key: &K) -> bool {
        self.expanded.as_ref() == Some(key)
    }

    /// 목록을 그리기 전에 부릅니다. 펼쳐 둔 항목이 목록에서 사라졌으면 펼침 상태를 지웁니다.
    pub fn begin(&mut self, exists: impl Fn(&K) -> bool) {
        if self.expanded.as_ref().is_some_and(|key| !exists(key)) {
            self.expanded = None;
        }
    }

    /// `key` 카드를 그린 뒤에 부릅니다. 머리 줄을 눌렀으면 펼치거나 접고,
    /// 다른 탭에서 "이 항목 보기"로 넘어왔으면 화면을 그 카드로 옮깁니다.
    pub fn after_card(&mut self, ui: &mut Ui, key: &K, outcome: &CardOutcome) {
        if outcome.toggled {
            self.expanded = (!self.is_open(key)).then(|| key.clone());
            self.reveal_api_key = false;
        }
        if self.reveal.as_ref() == Some(key) {
            self.reveal = None;
            ui.scroll_to_rect(
                outcome.response.rect.expand(REVEAL_MARGIN),
                Some(Align::Min),
            );
        }
    }

    /// 목록을 다 그린 뒤에 부릅니다. 보여 주려던 카드가 목록에 없었으면 그만둡니다.
    /// 나중에 그 항목이 생겼을 때 갑자기 화면이 옮겨지지 않게 하려는 것입니다.
    pub fn end(&mut self) {
        self.reveal = None;
    }

    /// `key` 카드를 펼치고, 다음에 그릴 때 화면을 그 카드로 옮깁니다.
    pub fn show(&mut self, key: K) {
        self.reveal_api_key = false;
        self.expanded = Some(key.clone());
        self.reveal = Some(key);
    }

    /// `key` 카드가 펼쳐져 있으면 접습니다 (항목을 지웠을 때).
    pub fn forget(&mut self, key: &K) {
        if self.is_open(key) {
            self.expanded = None;
        }
    }
}

/// 모델 준비 안내. 첫 줄은 제목, 나머지는 모델 이름 등의 보조 정보입니다.
pub fn model_notice(ui: &mut Ui, message: &str) {
    let mut lines = message.lines();
    Banner::new(Tone::Warning)
        .subtle()
        .title(lines.next().unwrap_or_default())
        .lines(lines.map(str::to_owned))
        .show(ui);
}

/// 빈 목록 안내. 내려받기 화면으로 이동하는 버튼을 눌렀으면 `true`입니다.
pub fn empty_state(ui: &mut Ui) -> bool {
    let mut go_download = false;
    card::card(ui, |ui| {
        ui.label(text::strong(t!("models.available.empty.title")));
        ui.label(text::muted(ui, t!("models.available.empty.body")));
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            let download = t!("models.available.empty.go_download");
            if controls::primary_icon_button(ui, Icon::Download, download).clicked() {
                go_download = true;
            }
        });
    });
    go_download
}

/// "사용 가능 모델" 목록의 묶음
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListGroup {
    /// 로컬 모델
    Local,
    /// Codex 로그인 (글 다듬기에만 있음)
    Codex,
    /// Claude Code CLI (글 다듬기에만 있음)
    ClaudeCli,
    /// 외부 제공자
    Providers,
}

/// 목록에서 묶음("로컬 모델", "외부 제공자" 등)이 시작되는 자리에 묶음 제목을 놓습니다.
///
/// 카드를 그리기 전에 항목마다 부릅니다. `previous`는 앞 항목의 묶음을 기억하는 값이고,
/// 목록을 그리기 시작할 때 `None`으로 둡니다.
pub fn list_group(ui: &mut Ui, previous: &mut Option<ListGroup>, group: ListGroup) {
    if *previous == Some(group) {
        return;
    }
    if previous.is_some() {
        ui.add_space(SECTION_GAP - CARD_GAP);
    }
    *previous = Some(group);
    let title = match group {
        ListGroup::Local => t!("models.available.group.local"),
        ListGroup::Codex => t!("models.available.group.codex"),
        ListGroup::ClaudeCli => t!("models.available.group.claude_cli"),
        ListGroup::Providers => t!("models.available.group.providers"),
    };
    group_title(ui, &title);
}

/// 목록의 묶음 제목 (예: "로컬 모델")
fn group_title(ui: &mut Ui, title: &str) {
    let palette = Palette::of(ui);
    ui.label(
        RichText::new(title)
            .font(theme::semibold(size::CAPTION))
            .color(palette.text_muted),
    );
    ui.add_space(2.0);
}

/// 펼친 카드 안의 작은 제목 (예: "엔진 설정")
pub fn sub_title(ui: &mut Ui, title: &str) {
    ui.label(text::strong(title));
    ui.add_space(2.0);
}

/// 한 줄에 왼쪽 버튼 묶음과 오른쪽 버튼 묶음을 놓습니다.
///
/// `add_right`는 오른쪽 끝부터, `add_left`는 왼쪽 끝부터 놓입니다.
pub fn action_row(ui: &mut Ui, add_left: impl FnOnce(&mut Ui), add_right: impl FnOnce(&mut Ui)) {
    let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        add_right(ui);
        ui.with_layout(Layout::left_to_right(Align::Center), add_left);
    });
}

/// 제거 버튼. 눌렀으면 `true`를 돌려줍니다.
///
/// 지금은 제거할 수 없으면 `blocked`에 그 이유를 넘깁니다. 버튼을 잠그고 이유를 풍선말로 보여 줍니다.
pub fn remove_button(ui: &mut Ui, blocked: Option<&str>) -> bool {
    ui.add_enabled_ui(blocked.is_none(), |ui| {
        let button = controls::danger_button(ui, t!("models.available.remove"));
        match blocked {
            Some(reason) => {
                button.on_disabled_hover_text(reason);
                false
            }
            None => button.clicked(),
        }
    })
    .inner
}

/// 제거 확인 창. 사용자가 답했으면 그 답을 돌려줍니다.
pub fn confirm_removal(ui: &Ui, id: &str, title: &str, body: &str) -> Option<Answer> {
    Confirm {
        id,
        title,
        body,
        confirm_label: &t!("models.available.remove"),
        cancel_label: &t!("common.cancel"),
        danger: true,
    }
    .show(ui)
}

/// 테스트 버튼을 누른 결과
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestClick {
    Start,
    Cancel,
}

/// 테스트 버튼이 보일 모습
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestState {
    /// 테스트를 시작할 수 있습니다.
    Idle,
    /// 이 버튼의 대상을 테스트하는 중입니다. 취소 버튼으로 바뀝니다.
    Running,
    /// 다른 것을 테스트하는 중입니다. 테스트는 한 번에 하나만 돌리므로 누를 수 없습니다.
    Busy,
}

impl TestState {
    /// `running`은 지금 테스트하는 대상(없으면 `None`), `target`은 이 버튼의 대상입니다.
    pub fn of<T: PartialEq>(running: Option<&T>, target: &T) -> Self {
        match running {
            None => Self::Idle,
            Some(running) if running == target => Self::Running,
            Some(_) => Self::Busy,
        }
    }
}

/// 테스트 버튼. 테스트하는 중이면 취소 버튼으로 바뀝니다.
pub fn test_button(ui: &mut Ui, label: &str, state: TestState) -> Option<TestClick> {
    if state == TestState::Running {
        return ui
            .button(t!("common.cancel"))
            .clicked()
            .then_some(TestClick::Cancel);
    }
    ui.add_enabled_ui(state == TestState::Idle, |ui| {
        controls::icon_text_button(ui, Icon::PlugZap, label)
            .on_disabled_hover_text(t!("models.test.busy"))
            .clicked()
    })
    .inner
    .then_some(TestClick::Start)
}

/// 진행 중임을 알리는 한 줄: 돌아가는 표시와 글
pub fn progress_line(ui: &mut Ui, status: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add(Spinner::new().size(16.0));
        ui.label(text::muted(ui, status));
    });
}

/// `target`을 테스트하는 중이면 경과 시간과, 가속 장치를 처음 쓸 때 오래 걸릴 수 있다는 안내를 보여 줍니다.
pub fn test_progress(ui: &mut Ui, running: Option<&ActiveTest>, target: &TestTarget) {
    let Some(active) = running.filter(|active| &active.target == target) else {
        return;
    };
    let elapsed = format::seconds_text(active.started.elapsed());
    // 클라우드 엔진은 모델을 불러오지 않고 외부 서비스에 요청을 보냅니다.
    let status = if active.kind == EngineKind::Cloud {
        t!("models.test.requesting", elapsed = elapsed)
    } else {
        t!("models.test.running", elapsed = elapsed)
    };
    let hint = active
        .accel
        .and_then(|request| accel::first_run_hint(request, AccelEnv::current()));
    progress_line(ui, &status);
    if let Some(hint) = hint {
        ui.label(text::caption(ui, hint));
    }
}

/// `target`의 마지막 테스트 결과를 보여 줍니다.
///
/// 여기 나오는 장치는 엔진이 모델을 불러온 뒤 보고한 **실제** 값입니다. 예상 장치와 섞지 않습니다.
pub fn test_outcome(ui: &mut Ui, last: Option<&TestOutcome>, target: &TestTarget) {
    let Some(outcome) = last.filter(|outcome| &outcome.target == target) else {
        return;
    };
    ui.add_space(6.0);
    match &outcome.result {
        Ok(result) => {
            let device = t!(
                "models.test.used_device",
                device = accel::reported_device_label(&result.device, outcome.kind)
            );
            let timing = t!(
                "models.test.timing",
                prepare = format::elapsed_text(result.prepare),
                recognize = format::elapsed_text(result.recognize)
            );
            let transcript = result.transcript.text.trim();
            let detail = if transcript.is_empty() {
                t!("models.test.empty_transcript")
            } else {
                t!("models.test.transcript", text = transcript)
            };
            Banner::new(Tone::Success)
                .title(&t!("models.test.success_title"))
                .line(device)
                .line(timing)
                .line(detail)
                .show(ui);
            // 가속을 요청했는데 그 장치로 실행하지 못했으면 이유를 따로 알립니다.
            let note = outcome.accel.and_then(|request| {
                accel::fallback_note(request, &result.device, AccelEnv::current())
            });
            if let Some(note) = note {
                ui.add_space(4.0);
                Banner::new(Tone::Warning).line(note).show(ui);
            }
        }
        // 취소는 사용자가 한 일이라 따로 알리지 않습니다.
        Err(TestFailure::Cancelled) => {}
        Err(TestFailure::Failed(message)) => {
            Banner::new(Tone::Error)
                .line(t!("models.test.failed", message = message))
                .show(ui);
        }
    }
}

/// 엔진 하나의 예상 장치 칸.
///
/// 위에는 지금 설정으로 쓸 것으로 **예상하는** 장치와 그 이유(`note`)를, 아래에는 이 빌드의 가속
/// 방식과 찾은 장치, 지금 설정 값을 보여 줍니다. 실제로 쓴 장치는 테스트 결과에만 나옵니다.
pub fn accel_panel(ui: &mut Ui, accel: &EngineAccel, note: &str) {
    let palette = Palette::of(ui);
    let plan = &accel.plan;
    // 가속 장치를 쓰면 초록, 쓰려던 가속을 못 쓰면 경고색, 일부러 CPU를 쓰면 보통 색입니다.
    let (icon, color) = if plan.device != ComputeDevice::Cpu {
        (Icon::Zap, palette.success_text)
    } else if plan.why.is_blocked() {
        (Icon::TriangleAlert, palette.warning_text)
    } else {
        (Icon::Cpu, palette.text_muted)
    };
    let note_color = if plan.why.is_blocked() {
        palette.warning_text
    } else {
        palette.text_muted
    };

    tile::panel(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        icon_line(
            ui,
            icon,
            color,
            text::strong(accel::device_label(&plan.device)).color(palette.text),
        );
        ui.label(text::caption(ui, note).color(note_color));

        ui.add_space(2.0);
        row::separator(ui);
        let backend = accel.backend.map_or_else(
            || t!("models.accel.backend_none").into_owned(),
            str::to_owned,
        );
        fact(ui, &t!("models.accel.backend"), &backend, palette.text);
        for (kind, names) in &accel.devices {
            if names.is_empty() {
                let missing = t!("models.accel.not_found");
                fact(ui, &kind.label(), &missing, palette.text_faint);
            } else {
                fact(ui, &kind.label(), &names.join(", "), palette.text);
            }
        }
        fact(
            ui,
            &t!("models.accel.setting"),
            &accel.request.label(),
            palette.text,
        );
    });
}

/// 아이콘과 글을 나란히 놓습니다. 글이 길면 아이콘 오른쪽에서 줄을 바꿉니다.
fn icon_line(ui: &mut Ui, icon: Icon, icon_color: Color32, text: RichText) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        ui.label(icons::text(icon).size(size::BODY + 1.0).color(icon_color));
        ui.vertical(|ui| ui.label(text));
    });
}

/// 정보 칸(예상 장치, 모델 정보)의 한 줄: 왼쪽에 항목 이름, 오른쪽에 값. 값이 길면 줄을 바꿉니다.
pub fn fact(ui: &mut Ui, label: &str, value: &str, value_color: Color32) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.allocate_ui_with_layout(
            vec2(FACT_LABEL_WIDTH, 0.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(FACT_LABEL_WIDTH);
                ui.label(text::caption(ui, label));
            },
        );
        ui.vertical(|ui| {
            ui.label(RichText::new(value).size(size::CAPTION).color(value_color));
        });
    });
}

/// 내려받기 진행 상황: 몇 번째 파일인지, 받은 양과 전체 크기, 검증 중인지를 적은 글과 진행 막대.
///
/// 글은 막대 안이 아니라 위에 둡니다. 좁은 창에서는 글이 막대보다 길어지므로, 그때 잘리지 않고
/// 줄을 바꾸게 하려는 것입니다.
pub fn download_progress(ui: &mut Ui, snapshot: &DownloadSnapshot) {
    let (fraction, label) = match snapshot.phase {
        DownloadPhase::Downloading => {
            let overall = snapshot.completed_bytes_before
                + snapshot.current_downloaded.min(snapshot.current_file_size);
            let fraction = (overall as f32 / snapshot.total_bytes.max(1) as f32).clamp(0.0, 1.0);
            let progress = format!(
                "{} / {} ({})",
                format_bytes(overall),
                format_bytes(snapshot.total_bytes),
                format::percent_text(fraction)
            );
            let label = t!(
                "models.download.downloading",
                file = snapshot.file_name,
                index = snapshot.file_index,
                total_files = snapshot.total_files,
                progress = progress
            );
            (fraction, label)
        }
        DownloadPhase::Verifying => {
            let fraction = (snapshot.current_downloaded as f32
                / snapshot.current_file_size.max(1) as f32)
                .clamp(0.0, 1.0);
            let label = t!(
                "models.download.verifying",
                file = snapshot.file_name,
                index = snapshot.file_index,
                total_files = snapshot.total_files
            );
            (fraction, label)
        }
    };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        ui.label(text::caption(ui, &*label));
        progress_bar(ui, fraction, &label);
    });
}

/// 얇은 진행 막대. 남은 너비를 다 씁니다. `label`은 화면 읽기 도구에 알릴 이름입니다.
///
/// egui의 `ProgressBar`는 쓰지 않습니다. `desired_width`에 준 값을 그대로 자리로 잡아서 "남은 너비를
/// 채워라"는 뜻으로 `f32::INFINITY`를 주면 창 밖까지 늘어나고, 바탕색이 입력 칸 색이라 같은 색인
/// 타일([`tile::panel`]) 안에서는 빈 부분이 보이지 않습니다. 음량 미터와 같은 모양으로 직접 그립니다.
fn progress_bar(ui: &mut Ui, fraction: f32, label: &str) {
    let palette = Palette::of(ui);
    let fraction = fraction.clamp(0.0, 1.0);
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), PROGRESS_BAR_HEIGHT),
        Sense::hover(),
    );
    response.widget_info(|| {
        let mut info = WidgetInfo::labeled(Role::ProgressIndicator, ui.is_enabled(), label);
        info.value = Some(f64::from(fraction * 100.0).floor());
        info
    });
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let rounding = CornerRadius::same(radius::PILL);
    painter.rect_filled(rect, rounding, palette.field);
    painter.rect_stroke(
        rect,
        rounding,
        Stroke::new(1.0, palette.border),
        StrokeKind::Inside,
    );
    if fraction > 0.0 {
        // 둥근 끝이 찌그러지지 않게 막대 높이보다 좁게 채우지 않습니다.
        let width = (rect.width() * fraction).max(rect.height());
        let filled = Rect::from_min_size(rect.min, vec2(width, rect.height()));
        painter.rect_filled(filled, rounding, palette.accent);
    }
}

/// 등록한 제공자 카드의 공급자 줄. 공급자는 등록할 때 정해지고 카드에서는 바꾸지 않습니다.
/// 이 PC에서 도는 서버나 개발 빌드 전용 가짜 제공자면 그 안내를 함께 보여 줍니다.
pub fn provider_row(ui: &mut Ui, category: Category, provider: &str) {
    let note = profiles::provider_note(category, provider);
    Row::new(&t!("models.providers.field.provider"))
        .help_opt(note.as_deref())
        .show(ui, |ui| {
            controls::badge(ui, &profiles::provider_label(category, provider));
        });
}

/// 제공자의 이름·주소·API 키·모델 입력 칸. 공급자는 부르는 쪽에서 따로 보여 줍니다.
/// 글 다듬기 제공자면 모델 아래에 추론 단계도 보여 줍니다.
///
/// 실제 요청 주소와 추천 모델, 모델 이름의 예시는 `category`(음성 인식인지 글 다듬기인지)를 따릅니다.
/// `id_salt`는 입력 칸 ID가 다른 제공자의 것과 겹치지 않게 하는 값입니다(예: 제공자 ID).
pub fn profile_fields(
    ui: &mut Ui,
    category: Category,
    id_salt: &str,
    profile: &mut ApiProfile,
    reveal_key: &mut bool,
) {
    let palette = Palette::of(ui);

    // 이름
    Row::new(&t!("models.providers.field.name")).show(ui, |ui| {
        // 좁은 창에서는 조작 영역이 기본 너비보다 좁아집니다.
        let width = row::CONTROL_WIDTH.min(ui.available_width());
        ui.add_sized(
            [width, ui.spacing().interact_size.y],
            TextEdit::singleline(&mut profile.name)
                .id_salt((id_salt, "name"))
                .hint_text(t!("models.providers.field.name_hint")),
        );
    });
    row::separator(ui);

    // 주소. 실제로 요청을 보낼 주소를 아래에 미리 보여 줍니다.
    let endpoint_preview = profiles::request_url(category, &profile.endpoint);
    Row::new(&t!("models.providers.field.endpoint")).show_below(ui, |ui| {
        ui.add(
            TextEdit::singleline(&mut profile.endpoint)
                .id_salt((id_salt, "endpoint"))
                .desired_width(f32::INFINITY)
                .hint_text(t!("models.providers.field.endpoint_hint")),
        );
        match &endpoint_preview {
            Ok(url) => {
                ui.label(text::caption(
                    ui,
                    t!("models.providers.field.endpoint_resolved", url = url),
                ));
            }
            Err(_) => {
                ui.label(
                    text::caption(ui, t!("models.providers.field.endpoint_invalid"))
                        .color(palette.warning_text),
                );
            }
        }
    });
    row::separator(ui);

    // API 키 (가려서 보여 주고, 버튼으로 잠깐 볼 수 있습니다)
    Row::new(&t!("models.providers.field.api_key")).show_below(ui, |ui| {
        let (icon, tooltip) = if *reveal_key {
            (Icon::EyeOff, t!("models.providers.field.api_key_hide"))
        } else {
            (Icon::Eye, t!("models.providers.field.api_key_show"))
        };
        // 버튼을 오른쪽 끝에 먼저 놓고, 남은 너비를 입력 칸이 씁니다.
        // 너비를 미리 어림하면 줄이 카드 밖으로 조금 넘칩니다.
        let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
            if controls::icon_button(ui, icon, &tooltip).clicked() {
                *reveal_key = !*reveal_key;
            }
            ui.add(
                TextEdit::singleline(&mut profile.api_key)
                    .id_salt((id_salt, "api_key"))
                    .password(!*reveal_key)
                    .desired_width(ui.available_width())
                    .hint_text(t!("models.providers.field.api_key_hint")),
            );
        });
    });
    row::separator(ui);

    // 모델 이름과 추천 모델
    let model_hint = match category {
        Category::Stt => t!("models.providers.field.model_hint.stt"),
        Category::Llm => t!("models.providers.field.model_hint.llm"),
    };
    Row::new(&t!("models.providers.field.model")).show_below(ui, |ui| {
        ui.add(
            TextEdit::singleline(&mut profile.model)
                .id_salt((id_salt, "model"))
                .desired_width(f32::INFINITY)
                .hint_text(model_hint),
        );
        if let Some(preset) = profiles::find_preset(category, &profile.provider)
            && !preset.model_suggestions.is_empty()
        {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                ui.label(text::caption(ui, t!("models.providers.field.suggestions")));
                for suggestion in preset.model_suggestions {
                    let selected = profile.model.trim() == *suggestion;
                    if ui.selectable_label(selected, *suggestion).clicked() {
                        profile.model = (*suggestion).to_owned();
                    }
                }
            });
        }
    });

    if category == Category::Llm {
        row::separator(ui);
        reasoning_row(ui, id_salt, profile);
    }
}

/// 글 다듬기 제공자의 추론 단계.
///
/// 공급자의 모델이 모두 받지 않는 단계는 꺼진 항목으로, 추론 단계를 바꿀 수 없는 공급자는 줄을 꺼서 보여 줍니다
/// (그 규칙은 `sulsul_llm::presets::ReasoningSupport`에 있습니다).
fn reasoning_row(ui: &mut Ui, id_salt: &str, profile: &mut ApiProfile) {
    let support = sulsul_llm::presets::reasoning_support(&profile.provider);
    let items: Vec<_> = ReasoningEffort::ALL
        .into_iter()
        .map(|effort| {
            let item = ComboItem::new(effort, reasoning_label(effort));
            if support.accepts(effort) || !support.adjustable() {
                item
            } else if effort == ReasoningEffort::None {
                item.unavailable(t!("models.providers.field.reasoning_cannot_disable"))
            } else {
                item.unavailable(t!("models.providers.field.reasoning_unsupported"))
            }
        })
        .collect();
    let help = if !support.adjustable() {
        t!("models.providers.field.reasoning_ignored")
    } else {
        match profile.reasoning_effort {
            ReasoningEffort::Default => t!("models.providers.field.reasoning_default_help"),
            ReasoningEffort::None => t!("models.providers.field.reasoning_none_help"),
            _ => t!("models.providers.field.reasoning_level_help"),
        }
    };
    Row::new(&t!("models.providers.field.reasoning"))
        .help(&help)
        .show(ui, |ui| {
            ui.add_enabled_ui(support.adjustable(), |ui| {
                // 좁은 창에서는 조작 영역이 기본 너비보다 좁아집니다.
                let width = row::CONTROL_WIDTH.min(ui.available_width());
                controls::combo(
                    ui,
                    (id_salt, "reasoning"),
                    width,
                    &mut profile.reasoning_effort,
                    &items,
                    "",
                );
            });
        });
}

/// 추론 단계의 이름 (예: "낮음")
pub fn reasoning_label(effort: ReasoningEffort) -> std::borrow::Cow<'static, str> {
    match effort {
        ReasoningEffort::Default => t!("models.providers.field.reasoning_default"),
        ReasoningEffort::None => t!("models.providers.field.reasoning_none"),
        ReasoningEffort::Low => t!("models.providers.field.reasoning_low"),
        ReasoningEffort::Medium => t!("models.providers.field.reasoning_medium"),
        ReasoningEffort::High => t!("models.providers.field.reasoning_high"),
    }
}

/// 모델이 인식할 수 있는 언어를 배지에 넣을 만큼 짧게 적은 글
/// (예: "다국어 · 자동 감지", "영어 전용", "자동 감지 + 5개 언어")
pub fn language_support_label(support: &LanguageSupport) -> String {
    if let Some(only) = support.single() {
        return t!(
            "models.cap.languages_single",
            language = language_name(only)
        )
        .into_owned();
    }
    match support {
        LanguageSupport::Any => t!("models.cap.languages_any"),
        LanguageSupport::Only(codes) => t!("models.cap.languages_count", count = codes.len()),
        // Whisper 언어를 모두 인식하는 모델은 개수를 세지 않고 "다국어"로 줄여 적습니다.
        LanguageSupport::AutoAnd(codes) if codes.len() >= WHISPER_LANGUAGES.len() => {
            t!("models.cap.languages_any")
        }
        LanguageSupport::AutoAnd(codes) => {
            t!("models.cap.languages_count_auto", count = codes.len())
        }
    }
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|code| (*code).to_owned()).collect()
    }

    #[test]
    fn language_badge_is_short_for_each_kind_of_model() {
        rust_i18n::set_locale("ko");
        let label = language_support_label;

        assert_eq!(label(&LanguageSupport::Any), "다국어 · 자동 감지");
        // Whisper 다국어 모델: 언어 100개를 세지 않고 줄여 적습니다.
        assert_eq!(
            label(&LanguageSupport::AutoAnd(codes(WHISPER_LANGUAGES))),
            "다국어 · 자동 감지"
        );
        assert_eq!(
            label(&LanguageSupport::Only(codes(&["en"]))),
            "영어 전용",
            "한 언어만 인식하는 모델은 그 언어 전용이라고 적어야 합니다"
        );
        assert_eq!(
            label(&LanguageSupport::AutoAnd(codes(&[
                "zh", "en", "ja", "ko", "yue"
            ]))),
            "자동 감지 + 5개 언어"
        );
        assert_eq!(
            label(&LanguageSupport::Only(codes(&["ko", "en"]))),
            "2개 언어 지원"
        );
    }
}
