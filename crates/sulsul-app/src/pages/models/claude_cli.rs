//! 글 다듬기 갈래의 Claude Code CLI: "외부 제공자" 탭의 CLI 카드와 "사용 가능 모델" 목록의 Claude 카드.
//!
//! 이 PC에 설치하고 로그인해 둔 Claude Code CLI(`claude`)로 다듬습니다. 실행과 결과 읽기는
//! `sulsul_llm::claude_cli`가 맡습니다. 외부 제공자 탭의 카드에서 CLI 위치·판·로그인 상태를 확인하고
//! 목록에 추가하면, 사용 가능 모델의 Claude 카드에서 모델과 추론 단계를 고릅니다.
//!
//! 설정은 `[postprocess.processors.llm_claude_cli]`에 둡니다.

use egui::{TextEdit, Ui, vec2};
use rust_i18n::t;
use sulsul_core::AppPaths;
use sulsul_core::config::{PostProcessConfig, ReasoningEffort};
use sulsul_llm::claude_cli::{
    self, CliLogin, CliStatus, DEFAULT_MODEL, MODEL_ALIASES, STATUS_TIMEOUT,
};
use sulsul_llm::client::LlmError;
use sulsul_llm::entries::ClaudeCliEntry;
use sulsul_llm::job::Job;

use super::parts::{self, BODY_GAP, Badge, CardHead, TestClick, TestState};
use crate::error_text;
use crate::icons::Icon;
use crate::session::PageContext;
use crate::theme::Palette;
use crate::widgets::card;
use crate::widgets::controls::{self, ComboItem};
use crate::widgets::row::{self, Row};
use crate::widgets::text;
use crate::widgets::tile;

/// CLI 카드와 Claude 카드에서 누른 것
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeAction {
    /// 사용 가능 모델 목록에 추가합니다.
    Add,
    /// "사용 가능 모델" 탭에서 Claude 카드를 펼쳐 봅니다.
    ShowEntry,
    /// 목록에서 뺍니다.
    Remove,
}

/// CLI 설치·로그인 상태 확인
#[derive(Default)]
enum StatusCheck {
    /// 아직 확인하지 않았습니다.
    #[default]
    NotChecked,
    Checking(Job<Result<CliStatus, LlmError>>),
    Done(Result<CliStatus, LlmError>),
}

/// Claude Code CLI의 화면 상태. 상태 확인은 이 화면의 상태에 들어 있어, 탭이나 화면을 바꿔도 계속 진행됩니다.
#[derive(Default)]
pub struct ClaudeCliPane {
    status: StatusCheck,
}

impl ClaudeCliPane {
    /// 상태를 확인하는 중이면 `true`입니다.
    pub fn is_busy(&self) -> bool {
        matches!(self.status, StatusCheck::Checking(_))
    }

    /// 상태 확인이 끝났는지 봅니다. 다른 화면을 보고 있을 때도 불립니다.
    pub fn tick(&mut self) {
        if let StatusCheck::Checking(job) = &self.status
            && let Some(result) = job.poll()
        {
            if let Err(error) = &result
                && !matches!(error, LlmError::Cancelled)
            {
                tracing::warn!("could not check the Claude Code CLI: {error}");
            }
            self.status = match result {
                Err(LlmError::Cancelled) => StatusCheck::NotChecked,
                other => StatusCheck::Done(other),
            };
        }
    }

    /// 아직 확인하지 않았으면 확인을 시작합니다.
    fn ensure_checked(
        &mut self,
        egui_ctx: &egui::Context,
        paths: &AppPaths,
        config: &PostProcessConfig,
    ) {
        if matches!(self.status, StatusCheck::NotChecked) {
            self.recheck(egui_ctx, paths, config);
        }
    }

    /// CLI를 다시 찾고, 판과 로그인 상태를 작업 스레드에서 확인합니다.
    fn recheck(&mut self, egui_ctx: &egui::Context, paths: &AppPaths, config: &PostProcessConfig) {
        let Some(cli) = claude_cli::find_cli_now(config) else {
            self.status = StatusCheck::Done(Err(LlmError::CliNotFound));
            return;
        };
        let worker_paths = paths.clone();
        let repaint_ctx = egui_ctx.clone();
        let job = Job::start(
            "sulsul-claude-cli-status",
            Err(LlmError::Cancelled),
            move || repaint_ctx.request_repaint(),
            move |cancel| claude_cli::check(&worker_paths, &cli, STATUS_TIMEOUT, cancel),
        );
        self.status = match job {
            Ok(job) => StatusCheck::Checking(job),
            Err(error) => {
                tracing::error!("could not start the Claude Code CLI check thread: {error}");
                StatusCheck::Done(Err(LlmError::CliFailed(error.to_string())))
            }
        };
    }

    // ── 외부 제공자 탭의 CLI 카드 ───────────────────────────────────────────

    /// "외부 제공자" 탭의 Claude Code CLI 카드: CLI 위치·판·로그인 상태, 목록에 추가
    pub fn add_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) -> Option<ClaudeAction> {
        self.ensure_checked(ui.ctx(), &cx.shared.paths, &cx.config.postprocess);
        let registered = claude_cli::is_registered(&cx.config.postprocess);
        let found = matches!(&self.status, StatusCheck::Done(Ok(_)));
        let mut recheck = false;
        let mut action = None;
        card::card(ui, |ui| {
            row::title_line(ui, |ui| {
                ui.label(text::strong(t!("models.claude_cli.title")));
                if registered {
                    Badge::success(t!("models.claude_cli.added_badge")).show(ui);
                }
            });
            ui.label(text::muted(ui, t!("models.claude_cli.body")));
            ui.label(text::caption(ui, t!("models.claude_cli.latency_note")));
            ui.add_space(8.0);
            self.status_panel(ui);
            ui.add_space(8.0);

            ui.add_enabled_ui(!cx.locked, |ui| {
                Row::new(&t!("models.claude_cli.path")).show_below(ui, |ui| {
                    let mut path = claude_cli::path_setting(&cx.config.postprocess).to_owned();
                    let response = ui.add(
                        TextEdit::singleline(&mut path)
                            .id_salt("claude_cli_path")
                            .desired_width(f32::INFINITY)
                            .hint_text(t!("models.claude_cli.path_hint")),
                    );
                    if response.changed() {
                        claude_cli::set_path(&mut cx.config.postprocess, &path);
                    }
                    // 입력을 마치면 그 위치로 다시 확인합니다.
                    recheck |= response.lost_focus();
                });
            });
            ui.add_space(BODY_GAP);

            parts::action_row(
                ui,
                |ui| {
                    ui.add_enabled_ui(!self.is_busy(), |ui| {
                        let label = t!("models.claude_cli.recheck");
                        recheck |= controls::icon_text_button(ui, Icon::RefreshCw, label).clicked();
                    });
                },
                |ui| {
                    if registered {
                        let label = t!("models.claude_cli.show_entry");
                        if controls::icon_text_button(ui, Icon::Boxes, label).clicked() {
                            action = Some(ClaudeAction::ShowEntry);
                        }
                    } else {
                        ui.add_enabled_ui(found && !cx.locked, |ui| {
                            let label = t!("models.claude_cli.add");
                            if controls::primary_button(ui, label).clicked() {
                                action = Some(ClaudeAction::Add);
                            }
                        });
                    }
                },
            );
        });
        if recheck {
            self.recheck(ui.ctx(), &cx.shared.paths, &cx.config.postprocess);
        }
        action
    }

    /// 확인한 상태: 확인하는 중이면 진행 표시, 찾았으면 위치·판·로그인, 못 찾았으면 그 안내
    fn status_panel(&self, ui: &mut Ui) {
        let palette = Palette::of(ui);
        match &self.status {
            StatusCheck::NotChecked | StatusCheck::Checking(_) => {
                parts::progress_line(ui, &t!("models.claude_cli.checking"));
            }
            StatusCheck::Done(Ok(status)) => {
                tile::panel(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 5.0;
                    status_facts(ui, status);
                });
                if status.login == CliLogin::LoggedOut {
                    ui.label(
                        text::caption(ui, t!("models.claude_cli.login.hint"))
                            .color(palette.warning_text),
                    );
                }
            }
            StatusCheck::Done(Err(LlmError::CliNotFound)) => {
                ui.label(
                    text::caption(ui, t!("models.claude_cli.not_found"))
                        .color(palette.warning_text),
                );
            }
            StatusCheck::Done(Err(error)) => {
                let message = t!(
                    "models.claude_cli.check_failed",
                    message = error_text::claude_cli(error)
                );
                ui.label(text::caption(ui, message).color(palette.warning_text));
            }
        }
    }

    // ── 사용 가능 모델의 Claude 카드 ────────────────────────────────────────

    /// Claude 카드의 내용: CLI 위치·로그인, 모델, 추론 단계, 연결 테스트와 제거.
    ///
    /// `test_status`는 테스트 버튼 아래에 진행 상황과 결과를 그립니다.
    pub fn entry_body(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        entry: &ClaudeCliEntry,
        test_state: TestState,
        test_status: impl FnOnce(&mut Ui),
    ) -> (Option<TestClick>, Option<ClaudeAction>) {
        self.ensure_checked(ui.ctx(), &cx.shared.paths, &cx.config.postprocess);
        let palette = Palette::of(ui);

        tile::panel(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            match (&entry.cli, &self.status) {
                (Some(_), StatusCheck::Done(Ok(status))) => status_facts(ui, status),
                (Some(cli), _) => parts::fact(
                    ui,
                    &t!("models.claude_cli.fact.path"),
                    &cli.display().to_string(),
                    palette.text,
                ),
                (None, _) => parts::fact(
                    ui,
                    &t!("models.claude_cli.fact.path"),
                    &t!("models.claude_cli.missing_badge"),
                    palette.warning_text,
                ),
            }
        });
        ui.add_space(4.0);
        ui.label(text::caption(ui, t!("models.claude_cli.latency_note")));
        ui.add_space(BODY_GAP);

        ui.add_enabled_ui(!cx.locked, |ui| {
            model_row(ui, &mut cx.config.postprocess);
            row::separator(ui);
            reasoning_row(ui, &mut cx.config.postprocess);
        });
        ui.add_space(BODY_GAP);

        let remove_blocked = if cx.locked {
            Some(t!("models.available.remove_blocked.locked"))
        } else if test_state == TestState::Running {
            Some(t!("models.available.remove_blocked.testing"))
        } else {
            None
        };
        // CLI를 찾지 못했으면 테스트할 수 없습니다.
        let test_state = if entry.cli.is_none() && test_state == TestState::Idle {
            TestState::Busy
        } else {
            test_state
        };
        let mut test_click = None;
        let mut remove = false;
        parts::action_row(
            ui,
            |ui| test_click = parts::test_button(ui, &t!("models.providers.test"), test_state),
            |ui| remove = parts::remove_button(ui, remove_blocked.as_deref()),
        );
        test_status(ui);
        (test_click, remove.then_some(ClaudeAction::Remove))
    }
}

/// "사용 가능 모델" 목록의 Claude 카드 머리 줄: 이름, CLI가 없으면 그 표시, 고른 모델
pub fn entry_head(entry: &ClaudeCliEntry) -> CardHead {
    let mut badges = Vec::new();
    if entry.cli.is_none() {
        badges.push(Badge::warning(t!("models.claude_cli.missing_badge")));
    }
    CardHead {
        name: t!("models.claude_cli.card_name").into_owned(),
        badges,
        description: Some(entry.model.clone()),
    }
}

/// 상태 정보 줄: 위치, 판, 로그인
fn status_facts(ui: &mut Ui, status: &CliStatus) {
    let palette = Palette::of(ui);
    parts::fact(
        ui,
        &t!("models.claude_cli.fact.path"),
        &status.path.display().to_string(),
        palette.text,
    );
    let unknown = t!("models.claude_cli.unknown");
    parts::fact(
        ui,
        &t!("models.claude_cli.fact.version"),
        status.version.as_deref().unwrap_or(&unknown),
        palette.text,
    );
    let (login, color) = match &status.login {
        CliLogin::LoggedIn {
            plan: Some(plan), ..
        } => (
            t!(
                "models.claude_cli.login.logged_in_plan",
                plan = plan_name(plan)
            ),
            palette.text,
        ),
        CliLogin::LoggedIn { plan: None, .. } => {
            (t!("models.claude_cli.login.logged_in"), palette.text)
        }
        CliLogin::LoggedOut => (
            t!("models.claude_cli.login.logged_out"),
            palette.warning_text,
        ),
        CliLogin::Unknown => (t!("models.claude_cli.login.unknown"), palette.text_muted),
    };
    parts::fact(ui, &t!("models.claude_cli.fact.login"), &login, color);
}

/// 모델 이름 입력 칸과 모델 별명
fn model_row(ui: &mut Ui, config: &mut PostProcessConfig) {
    Row::new(&t!("models.providers.field.model")).show_below(ui, |ui| {
        let mut model = claude_cli::model_setting(config).to_owned();
        let edited = ui
            .add(
                TextEdit::singleline(&mut model)
                    .id_salt("claude_cli_model")
                    .desired_width(f32::INFINITY)
                    .hint_text(t!("models.claude_cli.model_hint", model = DEFAULT_MODEL)),
            )
            .changed();
        if edited {
            claude_cli::set_model(config, &model);
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            ui.label(text::caption(ui, t!("models.claude_cli.models")));
            let chosen = claude_cli::chosen_model(config).to_owned();
            for alias in MODEL_ALIASES {
                if ui.selectable_label(chosen == alias, alias).clicked() {
                    claude_cli::set_model(config, alias);
                }
            }
        });
    });
}

/// 추론 단계. Claude Code CLI는 생각을 끄는 단계가 없어 "끄기"는 고를 수 없게 둡니다.
fn reasoning_row(ui: &mut Ui, config: &mut PostProcessConfig) {
    let mut effort = claude_cli::chosen_effort(config);
    let items: Vec<_> = ReasoningEffort::ALL
        .into_iter()
        .map(|effort| {
            let label = match effort {
                ReasoningEffort::Default => t!("models.claude_cli.reasoning_default"),
                other => parts::reasoning_label(other),
            };
            let item = ComboItem::new(effort, label);
            if claude_cli::accepts_effort(effort) {
                item
            } else {
                item.unavailable(t!("models.claude_cli.reasoning_cannot_disable"))
            }
        })
        .collect();
    let help = match effort {
        ReasoningEffort::Default => t!("models.claude_cli.reasoning_default_help"),
        _ => t!("models.providers.field.reasoning_level_help"),
    };
    Row::new(&t!("models.providers.field.reasoning"))
        .help(&help)
        .show(ui, |ui| {
            // 좁은 창에서는 조작 영역이 기본 너비보다 좁아집니다.
            let width = row::CONTROL_WIDTH.min(ui.available_width());
            if controls::combo(ui, "claude_cli_reasoning", width, &mut effort, &items, "") {
                claude_cli::set_effort(config, effort);
            }
        });
}

/// CLI가 알려 준 요금제 이름(`max` 등)을 보일 모양으로 바꿉니다 (첫 글자를 대문자로).
fn plan_name(plan: &str) -> String {
    let plan = plan.trim();
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_names_are_capitalized() {
        assert_eq!(plan_name("max"), "Max");
        assert_eq!(plan_name(" pro "), "Pro");
        assert_eq!(plan_name(""), "");
    }
}
