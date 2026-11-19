//! 글 다듬기 갈래의 Codex 로그인: "외부 제공자" 탭의 로그인 카드와 "사용 가능 모델" 목록의 Codex 카드.
//!
//! ChatGPT 계정으로 로그인해, 그 계정의 Codex 사용량으로 글을 다듬습니다. 로그인과 요청은
//! `sulsul_llm::codex`가 맡습니다. 로그인 정보는 설정 파일이 아니라 데이터 폴더에 있어, 로그인했는지는
//! 그릴 때마다 `codex::auth::account`로 확인합니다(파일이 바뀌었을 때만 다시 읽음).
//!
//! 고른 모델과 추론 단계는 설정의 `[postprocess.processors.llm_codex]`에 둡니다.

use egui::{TextEdit, Ui, vec2};
use rust_i18n::t;
use sulsul_core::AppPaths;
use sulsul_core::config::{PostProcessConfig, ReasoningEffort};
use sulsul_llm::client::LlmError;
use sulsul_llm::codex::auth::CodexAccount;
use sulsul_llm::codex::client::{CodexModel, MODELS_TIMEOUT, list_models};
use sulsul_llm::codex::login::{BrowserPage, BrowserPages, CodexLogin, LoginError};
use sulsul_llm::codex::{self, DEFAULT_MODEL};
use sulsul_llm::entries::CodexEntry;
use sulsul_llm::job::Job;

use super::parts::{self, BODY_GAP, Badge, CardHead, TestClick, TestState};
use crate::error_text;
use crate::format;
use crate::icons::Icon;
use crate::session::{PageContext, Shared};
use crate::system;
use crate::theme::Palette;
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card;
use crate::widgets::confirm::{Answer, Confirm};
use crate::widgets::controls::{self, ComboItem};
use crate::widgets::row::{self, Row};
use crate::widgets::text;
use crate::widgets::tile;

/// 로그인한 계정으로 쓸 수 있는 모델 목록
#[derive(Default)]
enum ModelList {
    /// 아직 받지 않았습니다.
    #[default]
    NotLoaded,
    Loading(Job<Result<Vec<CodexModel>, LlmError>>),
    Loaded(Vec<CodexModel>),
    Failed(LlmError),
}

/// Codex 탭과 카드에서 누른 것
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexAction {
    /// "사용 가능 모델" 탭에서 Codex 카드를 펼쳐 봅니다.
    ShowEntry,
    /// 로그아웃 확인을 받습니다.
    SignOut,
}

/// Codex 로그인의 화면 상태. 로그인과 모델 목록 받기는 이 화면의 상태에 들어 있어, 탭이나 화면을
/// 바꿔도 계속 진행됩니다.
#[derive(Default)]
pub struct CodexPane {
    /// 브라우저 로그인을 기다리는 중
    login: Option<CodexLogin>,
    /// 지난 로그인이 실패한 이유
    login_error: Option<LoginError>,
    /// 방금 로그인했습니다. 다음에 화면을 그릴 때 처리합니다(`tick`에서는 설정을 바꿀 수 없음).
    signed_in: bool,
    models: ModelList,
    /// 로그아웃 확인 창을 띄웠는지
    confirm_sign_out: bool,
}

impl CodexPane {
    /// 로그인을 기다리거나 모델 목록을 받는 중이면 `true`입니다.
    pub fn is_busy(&self) -> bool {
        self.login.is_some() || matches!(self.models, ModelList::Loading(_))
    }

    /// 로그인과 모델 목록 받기가 끝났는지 확인합니다. 다른 화면을 보고 있을 때도 불립니다.
    pub fn tick(&mut self, shared: &mut Shared) {
        let finished = self.login.as_ref().and_then(CodexLogin::poll);
        if let Some(result) = finished {
            self.login = None;
            match result {
                Ok(account) => {
                    self.login_error = None;
                    self.signed_in = true;
                    // 계정이 바뀌었을 수 있으니 모델 목록을 다시 받습니다.
                    self.models = ModelList::NotLoaded;
                    shared.toasts.info(t!(
                        "models.codex.signed_in",
                        account = account_name(&account)
                    ));
                }
                // 취소는 사용자가 한 일이라 알리지 않습니다.
                Err(LoginError::Cancelled) => {}
                Err(error) => {
                    tracing::warn!("Codex sign-in failed: {error}");
                    self.login_error = Some(error);
                }
            }
        }

        if let ModelList::Loading(job) = &self.models
            && let Some(result) = job.poll()
        {
            self.models = match result {
                Ok(models) => ModelList::Loaded(models),
                Err(LlmError::Cancelled) => ModelList::NotLoaded,
                Err(error) => ModelList::Failed(error),
            };
        }
    }

    /// 방금 로그인을 마쳤으면 `true`를 한 번 돌려줍니다.
    pub fn take_signed_in(&mut self) -> bool {
        std::mem::take(&mut self.signed_in)
    }

    /// 로그아웃한 뒤 부릅니다. 그 계정의 모델 목록을 잊습니다.
    pub fn forget_account(&mut self) {
        self.models = ModelList::NotLoaded;
        self.login_error = None;
    }

    // ── 외부 제공자 탭의 로그인 카드 ────────────────────────────────────────

    /// "외부 제공자" 탭의 Codex 로그인 카드. 로그인하지 않았으면 로그인 안내를, 로그인했으면 계정을 보여 줍니다.
    pub fn login_card(&mut self, ui: &mut Ui, shared: &mut Shared) -> Option<CodexAction> {
        let account = codex::auth::account(&shared.paths);
        match account {
            Some(account) if self.login.is_none() => self.account_card(ui, &account),
            _ => {
                self.sign_in_card(ui, shared);
                None
            }
        }
    }

    /// 로그인한 계정: 계정·요금제, 카드 보기, 로그아웃
    fn account_card(&mut self, ui: &mut Ui, account: &CodexAccount) -> Option<CodexAction> {
        let mut show_entry = false;
        let mut sign_out = false;
        card::card(ui, |ui| {
            row::title_line(ui, |ui| {
                ui.label(text::strong(t!("models.codex.title")));
                Badge::success(t!("models.codex.account.signed_in_badge")).show(ui);
            });
            ui.add_space(4.0);
            account_facts(ui, account);
            ui.add_space(4.0);
            ui.label(text::caption(ui, t!("models.codex.usage_note")));
            ui.add_space(BODY_GAP);
            parts::action_row(
                ui,
                |ui| {
                    let label = t!("models.codex.account.show_entry");
                    show_entry = controls::icon_text_button(ui, Icon::Boxes, label).clicked();
                },
                |ui| {
                    let label = t!("models.codex.sign_out");
                    sign_out = controls::icon_text_button(ui, Icon::LogOut, label).clicked();
                },
            );
        });
        if sign_out {
            Some(CodexAction::SignOut)
        } else {
            show_entry.then_some(CodexAction::ShowEntry)
        }
    }

    /// 로그인 안내와 로그인 버튼. 로그인을 기다리는 중이면 진행 상황과 주소 복사·취소 버튼입니다.
    fn sign_in_card(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let mut start = false;
        let mut cancel = false;
        let mut reopen = false;
        let mut copy = false;
        card::card(ui, |ui| {
            ui.label(text::strong(t!("models.codex.title")));
            ui.label(text::muted(ui, t!("models.codex.sign_in.body")));
            ui.add_space(2.0);
            ui.label(text::caption(ui, t!("models.codex.usage_note")));
            ui.add_space(BODY_GAP);
            match &self.login {
                Some(login) => {
                    let elapsed = format::seconds_text(login.elapsed());
                    parts::progress_line(
                        ui,
                        &t!("models.codex.sign_in.waiting", elapsed = elapsed),
                    );
                    ui.label(text::caption(ui, t!("models.codex.sign_in.no_browser")));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                        let label = t!("models.codex.sign_in.reopen");
                        reopen =
                            controls::icon_text_button(ui, Icon::ExternalLink, label).clicked();
                        let label = t!("models.codex.sign_in.copy_url");
                        copy = controls::icon_text_button(ui, Icon::Copy, label).clicked();
                        cancel = ui.button(t!("common.cancel")).clicked();
                    });
                }
                None => {
                    let label = t!("models.codex.sign_in.button");
                    start = controls::primary_icon_button(ui, Icon::LogIn, label).clicked();
                }
            }
        });

        if let Some(error) = &self.login_error
            && self.login.is_none()
        {
            ui.add_space(8.0);
            Banner::new(Tone::Error)
                .line(error_text::codex_login(error))
                .show(ui);
        }

        if start {
            self.start_login(ui.ctx(), shared);
        }
        if let Some(login) = &self.login {
            if reopen {
                open_browser(shared, login.auth_url());
            }
            if copy {
                ui.ctx().copy_text(login.auth_url().to_owned());
                shared.toasts.info(t!("models.codex.sign_in.copied"));
            }
            if cancel {
                login.cancel();
            }
        }
    }

    /// 로그인을 기다리기 시작하고 브라우저로 로그인 페이지를 엽니다.
    fn start_login(&mut self, egui_ctx: &egui::Context, shared: &mut Shared) {
        if self.login.is_some() {
            return;
        }
        self.login_error = None;
        let repaint_ctx = egui_ctx.clone();
        match CodexLogin::start(shared.paths.clone(), browser_pages(), move || {
            repaint_ctx.request_repaint();
        }) {
            Ok(login) => {
                open_browser(shared, login.auth_url());
                self.login = Some(login);
            }
            Err(error) => {
                tracing::warn!("could not start the Codex sign-in: {error}");
                self.login_error = Some(error);
            }
        }
    }

    // ── 사용 가능 모델의 Codex 카드 ─────────────────────────────────────────

    /// Codex 카드의 내용: 계정, 모델, 추론 단계, 연결 테스트와 로그아웃.
    ///
    /// `test_status`는 테스트 버튼 아래에 진행 상황과 결과를 그립니다.
    pub fn entry_body(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        entry: &CodexEntry,
        test_state: TestState,
        test_status: impl FnOnce(&mut Ui),
    ) -> (Option<TestClick>, Option<CodexAction>) {
        let locked = cx.locked;
        let config = &mut cx.config.postprocess;
        self.ensure_models(ui.ctx(), &cx.shared.paths);

        tile::panel(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            account_facts(ui, &entry.account);
        });
        ui.add_space(4.0);
        ui.label(text::caption(ui, t!("models.codex.usage_note")));
        ui.add_space(BODY_GAP);

        ui.add_enabled_ui(!locked, |ui| {
            self.model_row(ui, config);
            row::separator(ui);
            reasoning_row(ui, config);
        });
        ui.add_space(BODY_GAP);

        let sign_out_blocked = if locked {
            Some(t!("models.available.remove_blocked.locked"))
        } else if test_state == TestState::Running {
            Some(t!("models.codex.sign_out_blocked"))
        } else {
            None
        };
        let mut test_click = None;
        let mut sign_out = false;
        parts::action_row(
            ui,
            |ui| test_click = parts::test_button(ui, &t!("models.providers.test"), test_state),
            |ui| {
                ui.add_enabled_ui(sign_out_blocked.is_none(), |ui| {
                    let button =
                        controls::icon_text_button(ui, Icon::LogOut, t!("models.codex.sign_out"));
                    match sign_out_blocked.as_deref() {
                        Some(reason) => {
                            button.on_disabled_hover_text(reason);
                        }
                        None => sign_out = button.clicked(),
                    }
                });
            },
        );
        test_status(ui);
        (test_click, sign_out.then_some(CodexAction::SignOut))
    }

    /// 모델 이름 입력 칸과, 계정으로 쓸 수 있는 모델 목록
    fn model_row(&mut self, ui: &mut Ui, config: &mut PostProcessConfig) {
        let mut retry = false;
        Row::new(&t!("models.providers.field.model")).show_below(ui, |ui| {
            let mut model = codex::model_setting(config).to_owned();
            let edited = ui
                .add(
                    TextEdit::singleline(&mut model)
                        .id_salt("codex_model")
                        .desired_width(f32::INFINITY)
                        .hint_text(t!("models.codex.model_hint", model = DEFAULT_MODEL)),
                )
                .changed();
            match &self.models {
                ModelList::Loaded(models) if !models.is_empty() => {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                        ui.label(text::caption(ui, t!("models.codex.models")));
                        let chosen = codex::chosen_model(config).to_owned();
                        for listed in models {
                            let selected = chosen == listed.slug;
                            let mut chip = ui.selectable_label(selected, &listed.slug);
                            if listed.name != listed.slug || !listed.description.is_empty() {
                                chip = chip.on_hover_text(model_tooltip(listed));
                            }
                            if chip.clicked() {
                                model.clone_from(&listed.slug);
                                codex::set_model(config, &model);
                            }
                        }
                    });
                }
                ModelList::Loaded(_) | ModelList::NotLoaded => {}
                ModelList::Loading(_) => {
                    ui.label(text::caption(ui, t!("models.codex.models_loading")));
                }
                ModelList::Failed(error) => {
                    let palette = Palette::of(ui);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            text::caption(
                                ui,
                                t!(
                                    "models.codex.models_failed",
                                    message = error_text::codex(error)
                                ),
                            )
                            .color(palette.warning_text),
                        );
                        retry = ui.small_button(t!("models.codex.models_retry")).clicked();
                    });
                }
            }
            if edited {
                codex::set_model(config, &model);
            }
        });
        if retry {
            self.models = ModelList::NotLoaded;
        }
    }

    /// 모델 목록을 아직 받지 않았으면 작업 스레드에서 받기 시작합니다.
    fn ensure_models(&mut self, egui_ctx: &egui::Context, paths: &AppPaths) {
        if !matches!(self.models, ModelList::NotLoaded) {
            return;
        }
        let worker_paths = paths.clone();
        let repaint_ctx = egui_ctx.clone();
        let job = Job::start(
            "sulsul-codex-models",
            Err(LlmError::Cancelled),
            move || repaint_ctx.request_repaint(),
            move |cancel| list_models(&worker_paths, MODELS_TIMEOUT, cancel),
        );
        self.models = match job {
            Ok(job) => ModelList::Loading(job),
            Err(error) => {
                tracing::error!("could not start the Codex model list thread: {error}");
                ModelList::Failed(LlmError::Network(error.to_string()))
            }
        };
    }

    /// 로그아웃 확인 창을 띄웁니다.
    pub fn ask_sign_out(&mut self) {
        self.confirm_sign_out = true;
    }

    /// 로그아웃 확인 창. 확인을 받았으면 `true`를 돌려줍니다.
    pub fn confirm_sign_out_modal(&mut self, ui: &Ui) -> bool {
        if !self.confirm_sign_out {
            return false;
        }
        let answer = Confirm {
            id: "confirm_codex_sign_out",
            title: &t!("models.codex.sign_out_confirm.title"),
            body: &t!("models.codex.sign_out_confirm.body"),
            confirm_label: &t!("models.codex.sign_out"),
            cancel_label: &t!("common.cancel"),
            danger: false,
        }
        .show(ui);
        match answer {
            Some(Answer::Confirm) => {
                self.confirm_sign_out = false;
                true
            }
            Some(Answer::Cancel) => {
                self.confirm_sign_out = false;
                false
            }
            None => false,
        }
    }
}

/// "사용 가능 모델" 목록의 Codex 카드 머리 줄: 이름, 요금제, 고른 모델
pub fn entry_head(entry: &CodexEntry) -> CardHead {
    let mut badges = Vec::new();
    if let Some(plan) = entry.account.plan.as_deref() {
        badges.push(Badge::plain(t!(
            "models.codex.plan_badge",
            plan = plan_name(plan)
        )));
    }
    CardHead {
        name: t!("models.codex.card_name").into_owned(),
        badges,
        description: Some(entry.model.clone()),
    }
}

/// 계정 정보 줄: 계정(이메일)과 요금제
fn account_facts(ui: &mut Ui, account: &CodexAccount) {
    let palette = Palette::of(ui);
    let unknown = t!("models.codex.account.unknown");
    parts::fact(
        ui,
        &t!("models.codex.account.email"),
        account.email.as_deref().unwrap_or(&unknown),
        palette.text,
    );
    let plan = account.plan.as_deref().map(plan_name);
    parts::fact(
        ui,
        &t!("models.codex.account.plan"),
        plan.as_deref().unwrap_or(&unknown),
        palette.text,
    );
}

/// Codex의 추론 단계. 서버가 모르는 단계를 거절하면 다듬을 때는 빼고 다시 보냅니다.
fn reasoning_row(ui: &mut Ui, config: &mut PostProcessConfig) {
    let mut effort = codex::chosen_effort(config);
    let items: Vec<_> = ReasoningEffort::ALL
        .into_iter()
        .map(|effort| {
            let label = match effort {
                ReasoningEffort::Default => t!("models.codex.reasoning_default"),
                other => parts::reasoning_label(other),
            };
            ComboItem::new(effort, label)
        })
        .collect();
    let help = match effort {
        ReasoningEffort::Default => t!("models.codex.reasoning_default_help"),
        ReasoningEffort::None => t!("models.providers.field.reasoning_none_help"),
        _ => t!("models.providers.field.reasoning_level_help"),
    };
    Row::new(&t!("models.providers.field.reasoning"))
        .help(&help)
        .show(ui, |ui| {
            // 좁은 창에서는 조작 영역이 기본 너비보다 좁아집니다.
            let width = row::CONTROL_WIDTH.min(ui.available_width());
            if controls::combo(ui, "codex_reasoning", width, &mut effort, &items, "") {
                codex::set_effort(config, effort);
            }
        });
}

/// 모델 칩에 올렸을 때 보일 이름과 설명
fn model_tooltip(model: &CodexModel) -> String {
    if model.description.is_empty() {
        model.name.clone()
    } else {
        format!("{}\n{}", model.name, model.description)
    }
}

/// 로그인 알림에 쓸 계정 이름 (이메일, 없으면 "ChatGPT 계정")
fn account_name(account: &CodexAccount) -> String {
    account
        .email
        .clone()
        .unwrap_or_else(|| t!("models.codex.account.fallback_name").into_owned())
}

/// 로그인 정보에 적힌 요금제 이름(`plus` 등)을 보일 모양으로 바꿉니다 (첫 글자를 대문자로).
fn plan_name(plan: &str) -> String {
    let plan = plan.trim();
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// 로그인을 마친 브라우저에 보여 줄 페이지
fn browser_pages() -> BrowserPages {
    BrowserPages {
        success: BrowserPage {
            title: t!("models.codex.browser.success_title").into_owned(),
            body: t!("models.codex.browser.success_body").into_owned(),
        },
        failure: BrowserPage {
            title: t!("models.codex.browser.failure_title").into_owned(),
            body: t!("models.codex.browser.failure_body").into_owned(),
        },
    }
}

/// 로그인 페이지를 기본 브라우저로 엽니다. 열지 못했으면 주소를 복사하라고 알립니다.
fn open_browser(shared: &mut Shared, url: &str) {
    if system::open_url(url).is_err() {
        shared
            .toasts
            .warning(t!("models.codex.sign_in.open_failed"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_names_are_capitalized() {
        assert_eq!(plan_name("plus"), "Plus");
        assert_eq!(plan_name(" pro "), "Pro");
        assert_eq!(plan_name(""), "");
    }

    #[test]
    fn browser_pages_are_translated() {
        rust_i18n::set_locale("ko");
        let pages = browser_pages();
        for text in [
            &pages.success.title,
            &pages.success.body,
            &pages.failure.title,
            &pages.failure.body,
        ] {
            assert!(!text.starts_with("models."), "번역이 빠졌습니다: {text}");
        }
        assert_ne!(pages.success.title, pages.failure.title);
    }
}
