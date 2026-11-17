//! 프롬프트 화면: 글 다듬기(LLM 후처리)에 쓸 프롬프트를 관리합니다.
//!
//! 프롬프트마다 펼침 카드 하나를 두고, 스위치로 글 다듬기에 쓸 프롬프트 하나를 고릅니다.
//! 카드를 펼치면 그 자리에서 이름과 지시문을 고치고 지울 수 있습니다. 고친 값은 바로 설정에
//! 반영되고, 앱이 설정 파일(`[[postprocess.prompts]]`)에 저장합니다.

use egui::{Align, Label, TextEdit, Ui, vec2};
use rust_i18n::t;
use sulsul_core::config::{PostProcessConfig, Prompt, default_prompts};

use super::{SECTION_GAP, header};
use crate::icons::Icon;
use crate::session::PageContext;
use crate::theme::Palette;
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card;
use crate::widgets::confirm::{Answer, Confirm};
use crate::widgets::controls;
use crate::widgets::expander::Expander;
use crate::widgets::row::{self, Row};
use crate::widgets::text;

/// 목록의 카드 사이 간격
const CARD_GAP: f32 = 4.0;
/// 펼친 카드 안에서 입력 칸과 버튼 줄 사이 간격
const BODY_GAP: f32 = 12.0;
/// 지시문 입력 칸의 기본 높이 (줄 수). 글이 길어지면 칸이 따라 늘어납니다.
const TEXT_ROWS: usize = 10;
/// 카드 머리 줄에 보일 지시문 미리보기의 최대 글자 수
const PREVIEW_CHARS: usize = 160;
/// 카드로 화면을 옮길 때 카드 위에 남기는 여백
const REVEAL_MARGIN: f32 = 12.0;

#[derive(Default)]
pub struct PromptsPage {
    /// 펼쳐 둔 프롬프트의 ID. 한 번에 하나만 펼칩니다.
    expanded: Option<String>,
    /// 방금 만든 프롬프트의 ID. 다음에 그릴 때 화면을 그 카드로 옮기고 이름 칸에 커서를 둡니다.
    reveal: Option<String>,
    /// 지울지 묻고 있는 프롬프트
    confirm_remove: Option<PendingRemove>,
}

struct PendingRemove {
    id: String,
    name: String,
    /// 지금 쓰고 있는 프롬프트인지 (확인 문구가 달라집니다)
    in_use: bool,
}

/// 카드에서 누른 동작. 목록을 다 그린 뒤에 처리합니다.
enum CardAction {
    Toggle(String),
    Use(String),
    Remove(PendingRemove),
}

impl PromptsPage {
    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        header(ui, &t!("prompts.title"), &t!("prompts.subtitle"));

        if !cx.config.postprocess.enabled {
            Banner::new(Tone::Info)
                .line(t!("prompts.disabled_note"))
                .show(ui);
            ui.add_space(SECTION_GAP);
        }

        let locked = cx.locked;
        let post = &mut cx.config.postprocess;
        let can_use = !locked && post.enabled;

        // 설정 파일을 직접 고쳐 목록에 없는 프롬프트를 가리키게 됐으면 알립니다.
        if !post.prompts.is_empty() && post.active_prompt().is_none() {
            Banner::new(Tone::Warning)
                .line(t!("prompts.missing", id = post.prompt.trim()))
                .show(ui);
            ui.add_space(8.0);
        }

        // 펼쳐 둔 프롬프트가 목록에서 사라졌으면(파일을 직접 고친 경우) 펼침 상태를 지웁니다.
        if let Some(id) = &self.expanded
            && !post.prompts.iter().any(|prompt| &prompt.id == id)
        {
            self.expanded = None;
        }

        let mut add = false;
        row::with_actions(
            ui,
            true,
            |ui| {
                ui.add_enabled_ui(!locked, |ui| {
                    add =
                        controls::primary_icon_button(ui, Icon::Plus, t!("prompts.add")).clicked();
                });
            },
            |ui| {
                ui.label(text::caption(
                    ui,
                    t!("prompts.list_help", count = post.prompts.len()),
                ));
            },
        );
        ui.add_space(8.0);

        if post.prompts.is_empty() {
            if empty_state(ui, locked) {
                restore_defaults(post);
                cx.shared.toasts.info(t!("prompts.empty.restored"));
            }
        } else {
            let action = self.list(ui, post, locked);
            match action {
                Some(CardAction::Toggle(id)) => {
                    self.expanded = (self.expanded.as_ref() != Some(&id)).then_some(id);
                }
                Some(CardAction::Use(id)) if can_use => post.prompt = id,
                Some(CardAction::Remove(pending)) if !locked => {
                    self.confirm_remove = Some(pending);
                }
                Some(CardAction::Use(_) | CardAction::Remove(_)) | None => {}
            }
        }

        if add && !locked {
            let id = add_prompt(post, new_prompt_name(&post.prompts));
            self.expanded = Some(id.clone());
            self.reveal = Some(id);
        }

        self.confirm_ui(ui, cx);
    }

    /// 프롬프트 카드 목록을 그리고, 이번 프레임에 누른 동작을 돌려줍니다.
    fn list(
        &mut self,
        ui: &mut Ui,
        post: &mut PostProcessConfig,
        locked: bool,
    ) -> Option<CardAction> {
        // 같은 ID가 둘 있어도(파일을 직접 고친 경우) 한 카드만 켜진 모습이 되도록 자리로 비교합니다.
        let active = post.active_prompt().and_then(|active| {
            post.prompts
                .iter()
                .position(|prompt| prompt.id == active.id)
        });
        let can_use = !locked && post.enabled;
        let mut action = None;
        for (index, prompt) in post.prompts.iter_mut().enumerate() {
            let id = prompt.id.clone();
            let name = prompt_display_name(prompt);
            let preview = preview_line(&prompt.text);
            let in_use = active == Some(index);
            let open = self.expanded.as_ref() == Some(&id);
            // 방금 만든 프롬프트면 이름 칸에 커서를 둡니다.
            let focus_name = open && self.reveal.as_ref() == Some(&id);

            let mut turned_on = false;
            let mut use_clicked = false;
            let mut remove_clicked = false;
            let shown = Expander::new(&name, open).selected(in_use).show(
                ui,
                ("prompt_card", index, id.as_str()),
                |ui| card_head(ui, &name, &preview, in_use),
                |ui| {
                    ui.add_enabled_ui(can_use, |ui| {
                        let label = t!("prompts.use_named", name = name.as_str());
                        turned_on = controls::choice_toggle(ui, in_use, &label);
                    });
                },
                |ui| {
                    ui.add_enabled_ui(!locked, |ui| {
                        prompt_fields(ui, (index, id.as_str()), prompt, focus_name);
                    });
                    ui.add_space(BODY_GAP);
                    row::with_actions(
                        ui,
                        true,
                        |ui| {
                            ui.add_enabled_ui(!locked, |ui| {
                                let remove = t!("prompts.remove.button");
                                remove_clicked = controls::danger_button(ui, remove).clicked();
                                if !in_use {
                                    let label = t!("prompts.use");
                                    ui.add_enabled_ui(can_use, |ui| {
                                        use_clicked =
                                            controls::icon_text_button(ui, Icon::Check, label)
                                                .clicked();
                                    });
                                }
                            });
                        },
                        |ui| {
                            // 설정 파일을 직접 고칠 때 어느 항목인지 알 수 있게 ID를 보여 줍니다.
                            ui.add(
                                Label::new(text::caption(ui, t!("prompts.id", id = id.as_str())))
                                    .truncate(),
                            );
                        },
                    );
                },
            );

            if self.reveal.as_ref() == Some(&id) {
                self.reveal = None;
                ui.scroll_to_rect(shown.response.rect.expand(REVEAL_MARGIN), Some(Align::Min));
            }
            if shown.toggled {
                action = Some(CardAction::Toggle(id));
            } else if turned_on || use_clicked {
                action = Some(CardAction::Use(id));
            } else if remove_clicked {
                action = Some(CardAction::Remove(PendingRemove { id, name, in_use }));
            }
            ui.add_space(CARD_GAP);
        }
        // 보여 주려던 카드가 목록에 없었으면 그만둡니다.
        self.reveal = None;
        action
    }

    /// 프롬프트를 지울지 묻는 창
    fn confirm_ui(&mut self, ui: &Ui, cx: &mut PageContext<'_>) {
        let Some(pending) = &self.confirm_remove else {
            return;
        };
        // 하나 남은 프롬프트를 지울 때는 대신 쓸 것이 없으니, 대신 쓴다는 안내 없이 묻습니다.
        let has_others = cx.config.postprocess.prompts.len() > 1;
        let body = if pending.in_use && has_others {
            t!("prompts.remove.body_in_use", name = pending.name.as_str())
        } else {
            t!("prompts.remove.body", name = pending.name.as_str())
        };
        let answer = Confirm {
            id: "confirm_remove_prompt",
            title: &t!("prompts.remove.title"),
            body: &body,
            confirm_label: &t!("prompts.remove.button"),
            cancel_label: &t!("common.cancel"),
            danger: true,
        }
        .show(ui);
        match answer {
            Some(Answer::Confirm) => {
                if !cx.locked && remove_prompt(&mut cx.config.postprocess, &pending.id).is_some() {
                    if self.expanded.as_ref() == Some(&pending.id) {
                        self.expanded = None;
                    }
                    cx.shared
                        .toasts
                        .info(t!("prompts.remove.done", name = pending.name.as_str()));
                }
                self.confirm_remove = None;
            }
            Some(Answer::Cancel) => self.confirm_remove = None,
            None => {}
        }
    }
}

/// 카드 머리 줄: 이름, "사용 중" 표시, 지시문 앞부분
fn card_head(ui: &mut Ui, name: &str, preview: &str, in_use: bool) {
    let palette = Palette::of(ui);
    ui.spacing_mut().item_spacing.y = 3.0;
    row::title_line(ui, |ui| {
        ui.label(text::strong(name));
        if in_use {
            controls::tone_badge(
                ui,
                &t!("prompts.in_use"),
                palette.accent_soft,
                palette.accent_text,
            );
        }
    });
    if preview.is_empty() {
        // 쓰고 있는 프롬프트의 지시문이 비어 있으면 눈에 띄게 알립니다.
        let color = if in_use {
            palette.warning_text
        } else {
            palette.text_faint
        };
        ui.label(text::caption(ui, t!("prompts.empty_text")).color(color));
    } else {
        // 한 줄만 보여 주고 넘치는 부분은 말줄임표로 자릅니다.
        ui.add(Label::new(text::caption(ui, preview)).truncate());
    }
}

/// 펼친 카드의 입력 칸: 이름과 지시문
fn prompt_fields(ui: &mut Ui, id_salt: (usize, &str), prompt: &mut Prompt, focus_name: bool) {
    Row::new(&t!("prompts.field.name")).show(ui, |ui| {
        // 좁은 창에서는 조작 영역이 기본 너비보다 좁아집니다.
        let width = row::CONTROL_WIDTH.min(ui.available_width());
        let response = ui.add_sized(
            [width, ui.spacing().interact_size.y],
            TextEdit::singleline(&mut prompt.name)
                .id_salt((id_salt, "name"))
                .hint_text(t!("prompts.field.name_hint")),
        );
        if focus_name {
            response.request_focus();
        }
    });
    row::separator(ui);

    Row::new(&t!("prompts.field.text"))
        .help(&t!("prompts.field.text_help"))
        .show_below(ui, |ui| {
            ui.add(
                TextEdit::multiline(&mut prompt.text)
                    .id_salt((id_salt, "text"))
                    .desired_rows(TEXT_ROWS)
                    .desired_width(f32::INFINITY)
                    .hint_text(t!("prompts.field.text_hint")),
            );
        });
}

/// 프롬프트가 하나도 없을 때의 안내. "기본 프롬프트 다시 넣기"를 눌렀으면 `true`를 돌려줍니다.
fn empty_state(ui: &mut Ui, locked: bool) -> bool {
    let mut restore = false;
    card::card(ui, |ui| {
        ui.label(text::strong(t!("prompts.empty.title")));
        ui.label(text::muted(ui, t!("prompts.empty.body")));
        ui.add_space(8.0);
        ui.add_enabled_ui(!locked, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                let label = t!("prompts.empty.restore");
                restore = controls::icon_text_button(ui, Icon::RotateCcw, label).clicked();
            });
        });
    });
    restore
}

/// 화면에 보일 프롬프트 이름. 이름을 비워 뒀으면 "이름 없는 프롬프트"입니다.
pub fn prompt_display_name(prompt: &Prompt) -> String {
    let name = prompt.name.trim();
    if name.is_empty() {
        t!("prompts.unnamed").into_owned()
    } else {
        name.to_owned()
    }
}

/// 지시문을 한 줄 미리보기로 줄입니다: 줄바꿈과 이어진 빈칸을 빈칸 하나로 바꾸고 앞부분만 남깁니다.
fn preview_line(text: &str) -> String {
    let mut line = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
        if count > 0 {
            line.push(' ');
            count += 1;
        }
        for ch in word.chars() {
            if count >= PREVIEW_CHARS {
                line.push('…');
                return line;
            }
            line.push(ch);
            count += 1;
        }
    }
    line
}

/// 목록의 다른 프롬프트와 겹치지 않는 새 이름 ("새 프롬프트", "새 프롬프트 2", …)
fn new_prompt_name(prompts: &[Prompt]) -> String {
    let taken = |name: &str| prompts.iter().any(|prompt| prompt.name.trim() == name);
    let base = t!("prompts.new_name").into_owned();
    if !taken(&base) {
        return base;
    }
    (2_usize..)
        .map(|n| t!("prompts.new_name_numbered", n = n).into_owned())
        .find(|name| !taken(name))
        .unwrap_or(base)
}

/// 빈 프롬프트를 목록 끝에 더하고 그 ID를 돌려줍니다. 지금 쓰는 프롬프트는 바꾸지 않습니다.
fn add_prompt(post: &mut PostProcessConfig, name: String) -> String {
    let id = post.new_prompt_id();
    post.prompts.push(Prompt {
        id: id.clone(),
        name,
        text: String::new(),
    });
    id
}

/// 프롬프트를 목록에서 지우고 지운 것을 돌려줍니다.
///
/// 쓰고 있던 프롬프트를 지웠으면 남은 첫 프롬프트를 쓰도록 옮기고, 남은 것이 없으면 고른 값을 비웁니다.
/// 설정이 목록에 없는 ID를 가리키는 상태가 남지 않게 하려는 것입니다.
fn remove_prompt(post: &mut PostProcessConfig, id: &str) -> Option<Prompt> {
    let index = post.prompts.iter().position(|prompt| prompt.id == id)?;
    let removed = post.prompts.remove(index);
    // 같은 ID가 하나 더 남아 있으면(파일을 직접 고친 경우) 고른 값은 그대로 둡니다.
    let still_listed = post.prompts.iter().any(|prompt| prompt.id == id);
    if post.prompt.trim() == id && !still_listed {
        post.prompt = post
            .prompts
            .first()
            .map(|prompt| prompt.id.clone())
            .unwrap_or_default();
    }
    Some(removed)
}

/// 처음에 들어 있던 프롬프트를 다시 넣고 그 첫 프롬프트를 쓰도록 고릅니다.
fn restore_defaults(post: &mut PostProcessConfig) {
    post.prompts = default_prompts();
    post.prompt = post
        .prompts
        .first()
        .map(|prompt| prompt.id.clone())
        .unwrap_or_default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::config::DEFAULT_PROMPT_ID;

    fn prompt(id: &str, name: &str) -> Prompt {
        Prompt {
            id: id.into(),
            name: name.into(),
            text: String::new(),
        }
    }

    #[test]
    fn added_prompt_gets_unused_id_and_name() {
        rust_i18n::set_locale("ko");
        let mut post = PostProcessConfig::default();
        let first_name = new_prompt_name(&post.prompts);
        let first = add_prompt(&mut post, first_name.clone());
        let second_name = new_prompt_name(&post.prompts);
        let second = add_prompt(&mut post, second_name.clone());

        assert_ne!(first, second, "새 프롬프트의 ID가 겹칩니다");
        assert_ne!(first, DEFAULT_PROMPT_ID);
        assert_eq!(first_name, "새 프롬프트");
        assert_eq!(second_name, "새 프롬프트 2");
        assert_eq!(post.prompts.len(), 3);
        // 새로 만들기만 해서는 쓰는 프롬프트가 바뀌지 않습니다.
        assert_eq!(post.prompt, DEFAULT_PROMPT_ID);
    }

    #[test]
    fn removing_the_prompt_in_use_moves_to_the_first_remaining() {
        let mut post = PostProcessConfig {
            prompt: "b".into(),
            prompts: vec![prompt("a", "가"), prompt("b", "나"), prompt("c", "다")],
            ..PostProcessConfig::default()
        };

        // 쓰지 않는 프롬프트를 지우면 고른 값은 그대로입니다.
        assert_eq!(
            remove_prompt(&mut post, "c").map(|p| p.name),
            Some("다".into())
        );
        assert_eq!(post.prompt, "b");

        // 쓰던 프롬프트를 지우면 남은 첫 프롬프트로 옮깁니다.
        assert!(remove_prompt(&mut post, "b").is_some());
        assert_eq!(post.prompt, "a");
        assert_eq!(post.active_prompt().map(|p| p.id.as_str()), Some("a"));

        // 마지막 프롬프트까지 지우면 고른 값을 비웁니다.
        assert!(remove_prompt(&mut post, "a").is_some());
        assert!(post.prompts.is_empty());
        assert_eq!(post.prompt, "");

        // 목록에 없는 ID는 아무것도 지우지 않습니다.
        assert!(remove_prompt(&mut post, "a").is_none());
    }

    #[test]
    fn removing_the_first_prompt_keeps_an_empty_selection() {
        // 고른 값이 비어 있으면 첫 프롬프트를 쓰는 규칙이라, 비운 채로 둬도 없는 ID를 가리키지 않습니다.
        let mut post = PostProcessConfig {
            prompt: String::new(),
            prompts: vec![prompt("a", "가"), prompt("b", "나")],
            ..PostProcessConfig::default()
        };
        assert!(remove_prompt(&mut post, "a").is_some());
        assert_eq!(post.prompt, "");
        assert_eq!(post.active_prompt().map(|p| p.id.as_str()), Some("b"));
    }

    #[test]
    fn restoring_defaults_selects_the_default_prompt() {
        let mut post = PostProcessConfig {
            prompt: String::new(),
            prompts: Vec::new(),
            ..PostProcessConfig::default()
        };
        restore_defaults(&mut post);
        assert_eq!(post.prompts, default_prompts());
        assert_eq!(
            post.active_prompt().map(|p| p.id.as_str()),
            Some(DEFAULT_PROMPT_ID)
        );
    }

    #[test]
    fn preview_is_a_single_shortened_line() {
        assert_eq!(
            preview_line("  첫 줄\n\n둘째   줄\t끝 "),
            "첫 줄 둘째 줄 끝"
        );
        assert_eq!(preview_line(" \n "), "");

        let long = "가".repeat(PREVIEW_CHARS + 50);
        let preview = preview_line(&long);
        assert_eq!(preview.chars().count(), PREVIEW_CHARS + 1);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn unnamed_prompt_gets_a_placeholder_name() {
        rust_i18n::set_locale("ko");
        assert_eq!(prompt_display_name(&prompt("a", "  회의록  ")), "회의록");
        assert_eq!(
            prompt_display_name(&prompt("a", "  ")),
            "이름 없는 프롬프트"
        );
    }
}
