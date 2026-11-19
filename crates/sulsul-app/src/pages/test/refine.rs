//! 3단계 글 다듬기: 2단계에서 받아쓴 글(고쳐 넣을 수 있음)을 설정의 모델·제공자로 다듬습니다.
//!
//! 단축키 받아쓰기와 같은 [`SharedRefiner`]를 쓰므로, 로컬 모델을 여기서 올려 두면 단축키 받아쓰기도
//! 그대로 씁니다. `[postprocess] enabled`(단축키 받아쓰기에서 다듬을지)가 꺼져 있어도 모델을 골라 두었으면
//! 시험할 수 있습니다. 프롬프트는 이 카드에서만 바꿔 볼 수 있고 설정에는 저장하지 않습니다.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use egui::{Label, TextEdit, Ui};
use rust_i18n::t;
use sulsul_core::config::{Config, PostProcessConfig};
use sulsul_core::{AppPaths, CancelToken};
use sulsul_llm::catalog::find_variant;
use sulsul_llm::entries::{chosen_local_model, selection_issue};
use sulsul_llm::refine::{RefineError, Refined};
use sulsul_llm::{CLOUD_PROCESSOR_ID, CODEX_PROCESSOR_ID, LOCAL_PROCESSOR_ID, codex};

use super::{Gate, copy_button, output_box, progress_row};
use crate::error_text;
use crate::format::elapsed_text;
use crate::icons::Icon;
use crate::pages::Page;
use crate::pages::models::{llm_issue_text, profile_display_name, variant_label};
use crate::refiner::SharedRefiner;
use crate::session::{PageContext, Shared};
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card::Section;
use crate::widgets::controls::{self, ComboItem};
use crate::widgets::row::{self, Row};
use crate::widgets::stat::{Stat, stats};
use crate::widgets::text;

/// 입력 칸의 기본 높이 (줄 수). 글이 길어지면 칸이 따라 늘어납니다.
const INPUT_ROWS: usize = 3;

#[derive(Default)]
pub struct RefineCard {
    /// 다듬을 글. 2단계에서 받아쓰면 그 글로 바뀌고, 직접 고칠 수 있습니다.
    input: String,
    /// 이 카드에서만 쓸 프롬프트 ID. `None`이면 설정에서 고른 프롬프트를 씁니다.
    prompt: Option<String>,
    running: Option<Running>,
    result: Option<Attempt>,
}

struct Running {
    output: Receiver<JobOutput>,
    cancel: CancelToken,
    started: Instant,
    /// 오류 문구를 만들 때 쓰는, 이 작업에 넘긴 설정
    post: PostProcessConfig,
    model: String,
    prompt: String,
}

impl Drop for Running {
    /// 결과를 받지 않고 버리면(앱을 끝낼 때 등) 작업도 멈춥니다.
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct JobInput {
    refiner: SharedRefiner,
    post: PostProcessConfig,
    text: String,
    cancel: CancelToken,
}

struct JobOutput {
    result: Result<Refined, RefineError>,
    /// 로컬 모델을 새로 올렸는지 (걸린 시간에 모델을 올리는 시간이 들어감)
    cold: bool,
}

/// 끝난 다듬기 한 번
struct Attempt {
    model: String,
    prompt: String,
    outcome: Outcome,
}

enum Outcome {
    Done { refined: Refined, cold: bool },
    Cancelled,
    Failed(String),
}

impl RefineCard {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// 2단계에서 새로 받아쓴 글을 다듬을 글로 넣습니다. 이전 결과는 다른 글의 것이라 지웁니다.
    pub fn set_input(&mut self, text: String) {
        self.input = text;
        self.result = None;
    }

    /// 매 프레임 부릅니다(다른 화면을 보고 있어도). 작업 스레드가 끝났으면 결과를 기록합니다.
    /// 쉬는 실행기를 끝내는 일은 단축키 받아쓰기와 함께 쓰는 보관소라서 앱이 맡습니다
    /// ([`SharedRefiner::release_idle`]).
    pub fn tick(&mut self, shared: &Shared) {
        let Some(running) = &self.running else {
            return;
        };
        let output = match running.output.try_recv() {
            Ok(output) => Some(output),
            Err(TryRecvError::Empty) => return,
            // 작업 스레드가 결과 없이 끝났습니다(실행기 쪽 패닉 등).
            Err(TryRecvError::Disconnected) => None,
        };
        let Some(running) = self.running.take() else {
            return;
        };
        let outcome = match output {
            Some(JobOutput {
                result: Ok(refined),
                cold,
            }) => {
                tracing::info!(
                    seconds = refined.elapsed.as_secs_f32(),
                    cold,
                    "test refinement finished"
                );
                Outcome::Done { refined, cold }
            }
            Some(JobOutput {
                result: Err(error), ..
            }) if error.is_cancelled() => Outcome::Cancelled,
            Some(JobOutput {
                result: Err(error), ..
            }) => {
                tracing::warn!("test refinement failed: {error}");
                Outcome::Failed(error_text::refine(&error, &shared.paths, &running.post))
            }
            None => {
                tracing::error!("refinement thread ended without a result");
                Outcome::Failed(t!("test.refine.crashed").into_owned())
            }
        };
        self.result = Some(Attempt {
            model: running.model.clone(),
            prompt: running.prompt.clone(),
            outcome,
        });
    }

    /// 이 카드에서 쓸 설정: 설정의 `[postprocess]`에 이 카드에서 고른 프롬프트를 넣은 것
    fn post(&self, config: &Config) -> PostProcessConfig {
        let mut post = config.postprocess.clone();
        if let Some(id) = &self.prompt {
            post.prompt.clone_from(id);
        }
        post
    }

    fn start(&mut self, shared: &Shared, post: PostProcessConfig, ctx: &egui::Context) {
        let cancel = CancelToken::new();
        let input = JobInput {
            refiner: shared.refiner.clone(),
            post: post.clone(),
            text: self.input.trim().to_owned(),
            cancel: cancel.clone(),
        };
        self.result = None;
        self.running = Some(Running {
            output: spawn(input, ctx.clone()),
            cancel,
            started: Instant::now(),
            model: model_name(&shared.paths, &post).unwrap_or_default(),
            prompt: post
                .active_prompt()
                .map(|prompt| prompt.name.clone())
                .unwrap_or_default(),
            post,
        });
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, gate: Gate) {
        Section::new(Icon::WandSparkles, &t!("test.refine.title")).show(ui, |ui| {
            let post = self.post(cx.config);
            let busy = self.running.is_some();
            model_row(ui, cx.shared, &post);
            row::separator(ui);
            ui.add_enabled_ui(!busy, |ui| {
                self.prompt_row(ui, cx.config);
                row::separator(ui);
                Row::new(&t!("test.refine.input")).show_below(ui, |ui| {
                    ui.add(
                        TextEdit::multiline(&mut self.input)
                            .desired_rows(INPUT_ROWS)
                            .desired_width(f32::INFINITY)
                            .hint_text(t!("test.refine.input_hint")),
                    );
                });
            });

            ui.add_space(10.0);
            self.action_row(ui, cx.shared, gate, post);

            if let Some(attempt) = &self.result {
                ui.add_space(10.0);
                result_ui(ui, cx.shared, attempt);
            }
        });
    }

    fn prompt_row(&mut self, ui: &mut Ui, config: &Config) {
        let post = &config.postprocess;
        let configured = post.active_prompt().map_or_else(
            || post.prompt.trim().to_owned(),
            |prompt| prompt.name.clone(),
        );
        let mut items = vec![ComboItem::new(
            None,
            t!("test.refine.prompt_settings", name = configured),
        )];
        items.extend(
            post.prompts
                .iter()
                .map(|prompt| ComboItem::new(Some(prompt.id.clone()), prompt.name.clone())),
        );
        // 이 카드에서 고른 프롬프트가 목록에서 사라졌으면(프롬프트 화면에서 지운 경우) 설정 값으로 돌아갑니다.
        if let Some(id) = &self.prompt
            && !post.prompts.iter().any(|prompt| &prompt.id == id)
        {
            self.prompt = None;
        }
        Row::new(&t!("test.refine.prompt"))
            .help(&t!("test.refine.prompt_help"))
            .show(ui, |ui| {
                let width = row::CONTROL_WIDTH.min(ui.available_width());
                controls::combo(
                    ui,
                    "test_refine_prompt",
                    width,
                    &mut self.prompt,
                    &items,
                    "",
                );
            });
    }

    fn action_row(
        &mut self,
        ui: &mut Ui,
        shared: &mut Shared,
        gate: Gate,
        post: PostProcessConfig,
    ) {
        if let Some(running) = &self.running {
            if progress_row(ui, running.started, running.cancel.is_cancelled()) {
                running.cancel.cancel();
            }
            return;
        }
        // 다듬을 글이 비어 있으면 입력란의 안내 글이 보이므로 버튼 옆에는 적지 않습니다.
        let has_input = !self.input.trim().is_empty();
        let reason = blocker(&shared.paths, &post).or_else(|| gate.reason());
        ui.horizontal(|ui| {
            let clicked = ui
                .add_enabled_ui(reason.is_none() && has_input, |ui| {
                    controls::primary_icon_button(ui, Icon::WandSparkles, t!("test.refine.run"))
                        .clicked()
                })
                .inner;
            if clicked {
                let ctx = ui.ctx().clone();
                self.start(shared, post, &ctx);
            }
            if let Some(reason) = &reason {
                // 이유가 길면(모델 안내 등) 버튼 옆 남은 너비에서 줄을 바꿉니다.
                ui.add(Label::new(text::caption(ui, reason)).wrap());
            }
        });
    }
}

/// 다듬기에 쓸 모델·제공자. 단축키 받아쓰기에서 다듬기를 꺼 두었으면 함께 알립니다.
fn model_row(ui: &mut Ui, shared: &mut Shared, post: &PostProcessConfig) {
    let mut help =
        model_name(&shared.paths, post).unwrap_or_else(|| t!("test.refine.no_model").into_owned());
    if !post.enabled {
        help.push('\n');
        help.push_str(&t!("test.refine.dictation_off"));
    }
    let manage = Row::new(&t!("test.refine.model"))
        .help(&help)
        .show(ui, |ui| {
            controls::icon_text_button(ui, Icon::Boxes, t!("test.manage_models")).clicked()
        });
    if manage {
        shared.navigate = Some(Page::Models);
    }
}

/// 설정에서 고른 모델·제공자의 이름. 고르지 않았으면 `None`입니다.
fn model_name(paths: &AppPaths, post: &PostProcessConfig) -> Option<String> {
    match post.processor.trim() {
        LOCAL_PROCESSOR_ID => {
            let variant_id = chosen_local_model(post);
            Some(find_variant(paths, variant_id).map_or_else(
                || variant_id.to_owned(),
                |(_, variant)| variant_label(&variant),
            ))
        }
        CLOUD_PROCESSOR_ID => post.active_api_profile().map(|profile| {
            t!(
                "test.refine.provider",
                name = profile_display_name(profile),
                model = profile.model.trim()
            )
            .into_owned()
        }),
        CODEX_PROCESSOR_ID => {
            Some(t!("test.refine.codex", model = codex::chosen_model(post)).into_owned())
        }
        "" => None,
        other => Some(other.to_owned()),
    }
}

/// 지금 설정으로는 다듬을 수 없는 이유 (모델을 고르지 않았거나, 고른 모델·제공자·프롬프트를 쓸 수 없음).
/// [`sulsul_llm::refine::Refiner::refine`]이 확인하는 것과 같은 조건입니다.
fn blocker(paths: &AppPaths, post: &PostProcessConfig) -> Option<String> {
    if post.processor.trim().is_empty() {
        return Some(error_text::refine(&RefineError::NoModel, paths, post));
    }
    if let Some(issue) = selection_issue(paths, post) {
        return Some(llm_issue_text(paths, post, &issue));
    }
    let error = match post.active_prompt() {
        None => RefineError::NoPrompt {
            id: post.prompt.trim().to_owned(),
        },
        Some(prompt) if prompt.text.trim().is_empty() => RefineError::EmptyPrompt {
            name: prompt.name.clone(),
        },
        Some(_) => return None,
    };
    Some(error_text::refine(&error, paths, post))
}

fn result_ui(ui: &mut Ui, shared: &mut Shared, attempt: &Attempt) {
    let (refined, cold) = match &attempt.outcome {
        Outcome::Done { refined, cold } => (refined, *cold),
        // 취소는 사용자가 한 일이라 따로 알리지 않습니다.
        Outcome::Cancelled => return,
        Outcome::Failed(message) => {
            Banner::new(Tone::Error)
                .title(&t!("test.refine.failed"))
                .line(message)
                .show(ui);
            return;
        }
    };

    output_box(ui, &refined.text, "");
    ui.horizontal_wrapped(|ui| {
        copy_button(ui, shared, &refined.text);
        ui.label(text::caption(
            ui,
            [attempt.model.as_str(), attempt.prompt.as_str()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
        ));
    });
    ui.add_space(8.0);
    let mut elapsed = Stat::new(t!("test.refine.elapsed"), elapsed_text(refined.elapsed));
    if cold {
        elapsed = elapsed.note(t!("test.refine.cold"));
    }
    stats(ui, &[elapsed]);
}

/// 다듬기 작업 스레드를 시작합니다. 끝나면 결과를 보내고 화면을 다시 그리게 합니다.
fn spawn(input: JobInput, ctx: egui::Context) -> Receiver<JobOutput> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("sulsul-test-refine".into())
        .spawn(move || {
            let output = run(input);
            // 받는 쪽이 사라졌다면 화면이 닫힌 것이므로 결과를 버립니다.
            let _ = tx.send(output);
            ctx.request_repaint();
        });
    if let Err(error) = spawned {
        // 보내는 쪽이 버려져서 받는 쪽은 "결과 없이 끝남"으로 처리합니다.
        tracing::error!("could not start the refinement thread: {error}");
    }
    rx
}

fn run(input: JobInput) -> JobOutput {
    let JobInput {
        refiner,
        post,
        text,
        cancel,
    } = input;
    // 단축키 받아쓰기가 실행기를 쓰고 있으면 끝날 때까지 기다립니다.
    let mut refiner = refiner.lock();
    let cold = post.processor.trim() == LOCAL_PROCESSOR_ID && refiner.idle_deadline().is_none();
    let result = refiner.refine(&post, &text, &cancel);
    JobOutput { result, cold }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use sulsul_core::config::Prompt;
    use sulsul_llm::client::LlmError;
    use sulsul_llm::presets::{MOCK_PROVIDER_ID, profile_from_preset};

    /// 가짜 외부 제공자로 다듬는 설정. API 키가 `fail`이면 제공자가 인증 실패로 답합니다.
    fn cloud(api_key: &str) -> PostProcessConfig {
        let mut profile = profile_from_preset(MOCK_PROVIDER_ID, "m".into(), "가짜".into());
        profile.api_key = api_key.into();
        PostProcessConfig {
            // 단축키 받아쓰기에서 다듬기를 꺼 두어도 시험할 수 있어야 합니다.
            enabled: false,
            processor: CLOUD_PROCESSOR_ID.into(),
            api_profiles: vec![profile],
            ..PostProcessConfig::default()
        }
    }

    fn run_on_thread(post: PostProcessConfig, text: &str, cancel: CancelToken) -> JobOutput {
        let dir = tempfile::tempdir().unwrap();
        let input = JobInput {
            refiner: SharedRefiner::new(AppPaths::under(dir.path())),
            post,
            text: text.into(),
            cancel,
        };
        spawn(input, egui::Context::default())
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
    }

    #[test]
    fn refines_even_when_dictation_refinement_is_off() {
        let output = run_on_thread(cloud(""), "안녕 하세요", CancelToken::new());
        assert_eq!(output.result.unwrap().text, "[mock] 안녕 하세요");
        assert!(!output.cold, "외부 제공자는 모델을 올리지 않습니다");
    }

    #[test]
    fn provider_failure_and_cancel_are_reported() {
        let output = run_on_thread(cloud("fail"), "글", CancelToken::new());
        assert!(
            matches!(
                output.result,
                Err(RefineError::Cloud(LlmError::Http { status: 401, .. }))
            ),
            "{:?}",
            output.result
        );

        let cancel = CancelToken::new();
        cancel.cancel();
        let output = run_on_thread(cloud(""), "글", cancel);
        assert!(output.result.unwrap_err().is_cancelled());
    }

    #[test]
    fn prompt_chosen_here_overrides_the_setting_only_for_this_card() {
        let mut config = Config {
            postprocess: cloud(""),
            ..Config::default()
        };
        config.postprocess.prompts.push(Prompt {
            id: "short".into(),
            name: "짧게".into(),
            text: "짧게 줄여 주세요.".into(),
        });
        let mut card = RefineCard::default();
        assert_eq!(card.post(&config), config.postprocess);

        card.prompt = Some("short".into());
        let post = card.post(&config);
        assert_eq!(post.active_prompt().unwrap().name, "짧게");
        assert_ne!(config.postprocess.prompt, "short", "설정은 바꾸지 않습니다");
    }

    #[test]
    fn missing_model_or_prompt_blocks_the_run() {
        rust_i18n::set_locale("ko");
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        assert_eq!(blocker(&paths, &cloud("")), None);

        let nothing = PostProcessConfig::default();
        assert!(blocker(&paths, &nothing).is_some(), "모델을 고르지 않음");

        let mut no_provider = cloud("");
        no_provider.api_profiles.clear();
        assert!(blocker(&paths, &no_provider).is_some());

        let mut no_prompt = cloud("");
        no_prompt.prompts.clear();
        no_prompt.prompt.clear();
        assert!(blocker(&paths, &no_prompt).is_some());
    }
}
