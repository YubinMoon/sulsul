//! 받아쓴 글 다듬기: 설정에서 고른 모델에 고른 프롬프트와 받아쓴 글을 보내 다듬은 글을 받습니다.
//!
//! - 외부 제공자(`llm_cloud`)는 [`crate::client::chat`]으로 `/chat/completions`에 보냅니다.
//! - Codex 로그인(`llm_codex`)은 [`crate::codex::client::chat`]으로 ChatGPT의 Codex 백엔드에 보냅니다.
//! - Claude Code CLI(`llm_claude_cli`)는 [`crate::claude_cli::chat`]으로 이 PC의 `claude -p`를 실행합니다.
//! - 로컬 모델(`llm_local`)은 실행기 프로세스([`crate::runner`])에서 돌립니다. 실행기는 모델을 올려 둔 채
//!   남겨 두었다가 [`LOCAL_KEEP_LOADED`] 동안 쓰지 않으면 [`Refiner::unload_if_idle`]로 끝냅니다.
//!
//! `[postprocess] enabled`(다듬기를 쓸지)는 부르는 쪽이 봅니다. 여기서는 부르면 다듬습니다.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use sulsul_core::config::{ApiProfile, PostProcessConfig, ReasoningEffort};
use sulsul_core::{AppPaths, CancelToken};

use crate::catalog::find_variant;
use crate::chat::{ChatFormat, clean_output};
use crate::client::{self, LlmError};
use crate::entries::{SelectionIssue, chosen_local_model, selection_issue};
use crate::runner::{LocalError, LocalRunner};
use crate::{CLAUDE_CLI_PROCESSOR_ID, CLOUD_PROCESSOR_ID, CODEX_PROCESSOR_ID, LOCAL_PROCESSOR_ID};
use crate::{claude_cli, codex};

/// 로컬 모델을 쓰지 않고 이만큼 지나면 실행기를 끝내 메모리를 돌려줍니다 (음성 인식 모델의 기본 유지 시간과 같음).
pub const LOCAL_KEEP_LOADED: Duration = Duration::from_secs(5 * 60);

/// 로컬 모델을 올리는 제한 시간. 큰 모델을 디스크에서 처음 읽을 때도 넉넉하게 둡니다.
pub const LOCAL_LOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// 로컬 모델이 글을 만드는 제한 시간 (모델이 올라가 있지 않으면 올리는 시간도 듦). GPU가 없어 CPU로
/// 큰 모델을 돌리는 경우도 생각해 넉넉하게 둡니다.
pub const LOCAL_GENERATE_TIMEOUT: Duration = Duration::from_secs(240);

/// 다듬은 결과
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refined {
    /// 다듬은 글 (생각 블록과 앞뒤 공백을 지운 것)
    pub text: String,
    /// 요청을 보내고 답을 받기까지 걸린 시간
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RefineError {
    /// 다듬기에 쓸 모델이나 제공자를 고르지 않았습니다 (`processor = ""`).
    #[error("no model is selected for refinement")]
    NoModel,
    /// 고른 모델이나 제공자를 지금 쓸 수 없습니다.
    #[error("the selected refinement model cannot be used: {0:?}")]
    Selection(SelectionIssue),
    /// 쓸 프롬프트가 없습니다. `id`는 설정이 가리키는 프롬프트 ID입니다(비어 있으면 목록이 빔).
    #[error("prompt '{id}' was not found")]
    NoPrompt { id: String },
    /// 고른 프롬프트의 지시문이 비어 있습니다.
    #[error("prompt '{name}' has no instructions")]
    EmptyPrompt { name: String },
    #[error(transparent)]
    Cloud(LlmError),
    /// Codex 로그인으로 요청하다 난 오류
    #[error(transparent)]
    Codex(LlmError),
    /// Claude Code CLI로 요청하다 난 오류
    #[error(transparent)]
    ClaudeCli(LlmError),
    #[error(transparent)]
    Local(LocalError),
    /// 모델이 빈 글을 돌려주었습니다.
    #[error("the model returned no text")]
    EmptyOutput,
}

impl RefineError {
    /// 취소해서 멈춘 것인지 (알릴 필요 없음)
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Cloud(LlmError::Cancelled)
                | Self::Codex(LlmError::Cancelled)
                | Self::ClaudeCli(LlmError::Cancelled)
                | Self::Local(LocalError::Cancelled)
        )
    }
}

/// 설정에서 고른 다듬기 대상
enum Target<'a> {
    Local {
        model: PathBuf,
        format: ChatFormat,
    },
    Cloud(&'a ApiProfile),
    Codex {
        model: String,
        effort: ReasoningEffort,
    },
    ClaudeCli {
        cli: PathBuf,
        model: String,
        effort: ReasoningEffort,
    },
}

/// 다듬기를 실행합니다. 로컬 실행기를 올려 둔 채 다음 요청에 다시 씁니다.
pub struct Refiner {
    paths: AppPaths,
    runner_exe: PathBuf,
    local: Option<Kept>,
}

/// 띄워 둔 실행기와 마지막으로 쓴 시각
struct Kept {
    runner: LocalRunner,
    last_used: Instant,
}

impl Refiner {
    /// `runner_exe`는 로컬 모델 실행기 경로입니다(보통 [`crate::runner::default_runner_path`]).
    pub fn new(paths: AppPaths, runner_exe: PathBuf) -> Self {
        Self {
            paths,
            runner_exe,
            local: None,
        }
    }

    /// 로컬 모델을 고른 상태면 미리 올려 둡니다(녹음하는 동안 부름). 다듬기를 끈 상태면 아무것도 하지 않습니다.
    pub fn prepare(
        &mut self,
        config: &PostProcessConfig,
        cancel: &CancelToken,
    ) -> Result<(), RefineError> {
        if !config.enabled {
            return Ok(());
        }
        let Target::Local { model, .. } = self.target(config)? else {
            return Ok(());
        };
        let runner = self.runner()?;
        let result = runner.load(&model, true, LOCAL_LOAD_TIMEOUT, cancel);
        self.finish_local(result).map(|device| {
            if let Some(device) = device {
                tracing::info!(model = %model.display(), %device, "refinement model loaded");
            }
        })
    }

    /// `text`를 지금 고른 프롬프트에 따라 지금 고른 모델로 다듬습니다. 블로킹 함수입니다.
    pub fn refine(
        &mut self,
        config: &PostProcessConfig,
        text: &str,
        cancel: &CancelToken,
    ) -> Result<Refined, RefineError> {
        let prompt = config
            .active_prompt()
            .ok_or_else(|| RefineError::NoPrompt {
                id: config.prompt.trim().to_owned(),
            })?;
        if prompt.text.trim().is_empty() {
            return Err(RefineError::EmptyPrompt {
                name: prompt.name.clone(),
            });
        }
        let started = Instant::now();
        let raw = match self.target(config)? {
            Target::Cloud(profile) => {
                let timeout = client::default_timeout(profile);
                client::chat(profile, &prompt.text, text, timeout, cancel)
                    .map_err(RefineError::Cloud)?
                    .text
            }
            Target::Codex { model, effort } => {
                codex::client::chat(
                    &self.paths,
                    &model,
                    effort,
                    &prompt.text,
                    text,
                    client::REMOTE_TIMEOUT,
                    cancel,
                )
                .map_err(RefineError::Codex)?
                .text
            }
            Target::ClaudeCli { cli, model, effort } => {
                let target = claude_cli::CliTarget {
                    cli: &cli,
                    model: &model,
                    effort,
                };
                claude_cli::chat(
                    &self.paths,
                    target,
                    &prompt.text,
                    text,
                    claude_cli::REQUEST_TIMEOUT,
                    cancel,
                )
                .map_err(RefineError::ClaudeCli)?
                .text
            }
            Target::Local { model, format } => {
                let rendered = format.render(&prompt.text, text);
                let runner = self.runner()?;
                let result =
                    runner.generate(&model, true, &rendered, LOCAL_GENERATE_TIMEOUT, cancel);
                let generated = self.finish_local(result)?;
                tracing::info!(
                    device = %generated.device,
                    prompt_tokens = generated.prompt_tokens,
                    output_tokens = generated.output_tokens,
                    truncated = generated.truncated,
                    "local refinement finished"
                );
                generated.text
            }
        };
        let text = clean_output(&raw);
        if text.is_empty() {
            return Err(RefineError::EmptyOutput);
        }
        Ok(Refined {
            text,
            elapsed: started.elapsed(),
        })
    }

    /// 실행기를 내려놓을 시각 (마지막으로 쓴 시각 + [`LOCAL_KEEP_LOADED`]). 띄워 둔 실행기가 없으면 `None`
    pub fn idle_deadline(&self) -> Option<Instant> {
        self.local
            .as_ref()
            .map(|kept| kept.last_used + LOCAL_KEEP_LOADED)
    }

    /// `now`가 [`Self::idle_deadline`]을 지났으면 실행기를 끝내고 `true`를 돌려줍니다.
    pub fn unload_if_idle(&mut self, now: Instant) -> bool {
        if self.idle_deadline().is_none_or(|deadline| now < deadline) {
            return false;
        }
        tracing::info!("unloading the idle refinement model");
        self.local = None;
        true
    }

    /// 띄워 둔 실행기를 끝내 메모리를 돌려줍니다.
    pub fn unload(&mut self) {
        self.local = None;
    }

    fn target<'a>(&self, config: &'a PostProcessConfig) -> Result<Target<'a>, RefineError> {
        if let Some(issue) = selection_issue(&self.paths, config) {
            return Err(RefineError::Selection(issue));
        }
        match config.processor.trim() {
            LOCAL_PROCESSOR_ID => {
                let variant_id = chosen_local_model(config);
                let unknown = || {
                    RefineError::Selection(SelectionIssue::UnknownModel {
                        variant_id: variant_id.to_owned(),
                    })
                };
                let (family, variant) =
                    find_variant(&self.paths, variant_id).ok_or_else(unknown)?;
                let format = ChatFormat::for_family(&family.id).ok_or_else(unknown)?;
                let model = variant.files.first().ok_or_else(unknown)?.path.clone();
                Ok(Target::Local { model, format })
            }
            CLOUD_PROCESSOR_ID => config
                .active_api_profile()
                .map(Target::Cloud)
                .ok_or(RefineError::Selection(SelectionIssue::NoProvider)),
            CODEX_PROCESSOR_ID => Ok(Target::Codex {
                model: codex::chosen_model(config).to_owned(),
                effort: codex::chosen_effort(config),
            }),
            CLAUDE_CLI_PROCESSOR_ID => claude_cli::find_cli(config)
                .map(|cli| Target::ClaudeCli {
                    cli,
                    model: claude_cli::chosen_model(config).to_owned(),
                    effort: claude_cli::chosen_effort(config),
                })
                .ok_or(RefineError::Selection(SelectionIssue::ClaudeCliMissing)),
            // 고르지 않았습니다(알 수 없는 ID는 위의 selection_issue가 걸러 냄).
            _ => Err(RefineError::NoModel),
        }
    }

    /// 띄워 둔 실행기. 없으면 새로 띄웁니다.
    fn runner(&mut self) -> Result<&mut LocalRunner, RefineError> {
        let kept = match self.local.take() {
            Some(kept) => kept,
            None => Kept {
                runner: LocalRunner::spawn(&self.runner_exe).map_err(RefineError::Local)?,
                last_used: Instant::now(),
            },
        };
        Ok(&mut self.local.insert(kept).runner)
    }

    /// 실행기 작업이 끝난 뒤 정리합니다. 실행기가 멀쩡히 답하지 못했으면 버리고 다음에 새로 띄웁니다.
    fn finish_local<T>(&mut self, result: Result<T, LocalError>) -> Result<T, RefineError> {
        match &result {
            Err(error) if !error.runner_is_healthy() => {
                if !matches!(error, LocalError::Cancelled) {
                    tracing::warn!("local model runner failed: {error}");
                }
                self.local = None;
            }
            _ => {
                if let Some(kept) = &mut self.local {
                    kept.last_used = Instant::now();
                }
            }
        }
        result.map_err(RefineError::Local)
    }
}

#[cfg(test)]
mod tests {
    use sulsul_core::config::Prompt;
    use sulsul_core::toml::Value;

    use super::*;
    use crate::presets::{MOCK_PROVIDER_ID, profile_from_preset};
    use crate::runner::RUNNER_NAME;
    use crate::{CLOUD_PROCESSOR_ID, LOCAL_MODEL_KEY};

    fn refiner(root: &std::path::Path) -> Refiner {
        Refiner::new(AppPaths::under(root), root.join(RUNNER_NAME))
    }

    fn cloud_config(api_key: &str) -> PostProcessConfig {
        let mut profile = profile_from_preset(MOCK_PROVIDER_ID, "m".into(), "가짜".into());
        profile.api_key = api_key.into();
        PostProcessConfig {
            enabled: true,
            processor: CLOUD_PROCESSOR_ID.into(),
            api_profiles: vec![profile],
            ..PostProcessConfig::default()
        }
    }

    #[test]
    fn cloud_provider_refines_with_the_active_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let refined = refiner(dir.path())
            .refine(&cloud_config(""), " 안녕 하세요 ", &CancelToken::new())
            .unwrap();
        assert_eq!(refined.text, "[mock] 안녕 하세요");
    }

    #[test]
    fn provider_errors_are_passed_through() {
        let dir = tempfile::tempdir().unwrap();
        let error = refiner(dir.path())
            .refine(&cloud_config("fail"), "글", &CancelToken::new())
            .unwrap_err();
        assert!(
            matches!(
                error,
                RefineError::Cloud(LlmError::Http { status: 401, .. })
            ),
            "{error:?}"
        );

        let cancel = CancelToken::new();
        cancel.cancel();
        let error = refiner(dir.path())
            .refine(&cloud_config(""), "글", &cancel)
            .unwrap_err();
        assert!(error.is_cancelled(), "{error:?}");
    }

    #[test]
    fn missing_selection_or_prompt_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = refiner(dir.path());
        let cancel = CancelToken::new();

        let nothing = PostProcessConfig {
            enabled: true,
            ..PostProcessConfig::default()
        };
        assert_eq!(r.refine(&nothing, "글", &cancel), Err(RefineError::NoModel));

        let mut no_provider = cloud_config("");
        no_provider.api_profiles.clear();
        assert_eq!(
            r.refine(&no_provider, "글", &cancel),
            Err(RefineError::Selection(SelectionIssue::NoProvider))
        );

        let mut missing_prompt = cloud_config("");
        missing_prompt.prompt = "gone".into();
        assert_eq!(
            r.refine(&missing_prompt, "글", &cancel),
            Err(RefineError::NoPrompt { id: "gone".into() })
        );

        let mut no_prompts = cloud_config("");
        no_prompts.prompts.clear();
        no_prompts.prompt.clear();
        assert_eq!(
            r.refine(&no_prompts, "글", &cancel),
            Err(RefineError::NoPrompt { id: String::new() })
        );

        let mut blank = cloud_config("");
        blank.prompts = vec![Prompt {
            id: "blank".into(),
            name: "빈 지시문".into(),
            text: "  \n".into(),
        }];
        blank.prompt = "blank".into();
        assert_eq!(
            r.refine(&blank, "글", &cancel),
            Err(RefineError::EmptyPrompt {
                name: "빈 지시문".into()
            })
        );
    }

    #[test]
    fn codex_needs_a_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = refiner(dir.path());
        let config = PostProcessConfig {
            enabled: true,
            processor: CODEX_PROCESSOR_ID.into(),
            ..PostProcessConfig::default()
        };
        assert_eq!(
            r.refine(&config, "글", &CancelToken::new()),
            Err(RefineError::Selection(SelectionIssue::CodexSignedOut))
        );
        // 녹음하는 동안 미리 준비할 때도 외부 제공자처럼 같은 이유로 알립니다.
        assert!(matches!(
            r.prepare(&config, &CancelToken::new()),
            Err(RefineError::Selection(SelectionIssue::CodexSignedOut))
        ));
    }

    fn local_config(variant_id: &str) -> PostProcessConfig {
        let mut config = PostProcessConfig {
            enabled: true,
            processor: LOCAL_PROCESSOR_ID.into(),
            ..PostProcessConfig::default()
        };
        config
            .processors
            .entry(LOCAL_PROCESSOR_ID.into())
            .or_default()
            .insert(LOCAL_MODEL_KEY.into(), Value::String(variant_id.into()));
        config
    }

    #[test]
    fn local_model_needs_the_file_and_the_runner() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = refiner(dir.path());
        let cancel = CancelToken::new();
        let config = local_config("qwen3_5-2b-q4_k_m");

        assert_eq!(
            r.refine(&config, "글", &cancel),
            Err(RefineError::Selection(SelectionIssue::NotInstalled {
                variant_id: "qwen3_5-2b-q4_k_m".into()
            }))
        );
        assert_eq!(
            r.prepare(&config, &cancel),
            Err(RefineError::Selection(SelectionIssue::NotInstalled {
                variant_id: "qwen3_5-2b-q4_k_m".into()
            }))
        );

        // 모델 파일이 있어도 실행기가 없으면 알립니다.
        let paths = AppPaths::under(dir.path());
        let (_, variant) = find_variant(&paths, "qwen3_5-2b-q4_k_m").unwrap();
        let file = &variant.files[0];
        std::fs::create_dir_all(file.path.parent().unwrap()).unwrap();
        std::fs::write(&file.path, b"").unwrap();
        assert_eq!(
            r.refine(&config, "글", &cancel),
            Err(RefineError::Local(LocalError::RunnerMissing(
                dir.path().join(RUNNER_NAME)
            )))
        );
        assert_eq!(
            r.idle_deadline(),
            None,
            "띄우지 못한 실행기는 남기지 않습니다"
        );
    }

    #[test]
    fn disabled_refinement_does_not_prepare_anything() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = local_config("qwen3_5-2b-q4_k_m");
        config.enabled = false;
        assert_eq!(
            refiner(dir.path()).prepare(&config, &CancelToken::new()),
            Ok(())
        );
    }
}
