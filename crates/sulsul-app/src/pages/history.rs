//! 기록 화면: 단축키로 받아쓴 글을 최근 것부터 보여 줍니다.
//!
//! 기록마다 펼침 카드 하나를 둡니다. 접힌 카드는 받아쓴 시각과 글 한 줄(다듬은 글, 없으면 받아쓴 원문)만
//! 보여 주고 넘치는 부분은 말줄임표로 잘라, 여러 기록을 한눈에 볼 수 있게 합니다. 카드를 누르면 펼쳐져
//! 다듬은 글과 받아쓴 원문을 모두 보여 주고, 글마다 복사 버튼이, 맨 아래에 기록을 지우는 버튼이 나옵니다.
//! 복사는 클립보드에 그대로 넣습니다(단축키 받아쓰기처럼 붙여넣은 뒤 원래 내용으로 되돌리지 않음).
//! 기록이 많으면 [`PAGE_SIZE`]개씩 더 보여 줍니다.

use std::collections::HashSet;

use egui::{Label, RichText, TextStyle, Ui};
use rust_i18n::t;

use super::header;
use crate::clock::DateTime;
use crate::history::{Entry, MAX_ENTRIES, Refinement};
use crate::icons::Icon;
use crate::session::PageContext;
use crate::theme::Palette;
use crate::widgets::confirm::{Answer, Confirm};
use crate::widgets::expander::Expander;
use crate::widgets::row;
use crate::widgets::{card, controls, text};

/// 한 번에 보여 주는 기록 수 ("더 보기"를 누를 때마다 이만큼 늘림)
const PAGE_SIZE: usize = 50;
/// 카드 사이 간격
const CARD_GAP: f32 = 4.0;
/// 펼친 카드에서 글 사이 간격
const BODY_SPACING: f32 = 6.0;

#[derive(Default)]
pub struct HistoryPage {
    /// 펼쳐 둔 기록의 번호
    expanded: HashSet<u64>,
    /// "더 보기"로 늘린 보여 줄 기록 수 (최소 [`PAGE_SIZE`])
    shown: usize,
    /// 전체 지우기를 묻는 중
    confirm_clear: bool,
}

/// 카드에서 누른 동작. 목록을 다 그린 뒤에 처리합니다.
enum Action {
    Copy(String),
    Remove(u64),
    Toggle(u64),
}

/// 시각 표시에 쓰는 오늘·어제 날짜 (프레임마다 한 번 구함)
struct Today {
    today: DateTime,
    yesterday: DateTime,
}

impl HistoryPage {
    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        header(
            ui,
            &t!("history.title"),
            Some(&t!("history.subtitle", max = MAX_ENTRIES)),
        );

        let history = &cx.shared.history;
        if history.is_loading() {
            ui.label(text::muted(ui, t!("history.loading")));
            return;
        }
        let entries = history.entries();
        if entries.is_empty() {
            self.confirm_clear = false;
            empty_state(ui);
            return;
        }

        row::with_actions(
            ui,
            true,
            |ui| {
                let label = t!("history.clear.button");
                if controls::icon_text_button(ui, Icon::Trash2, label).clicked() {
                    self.confirm_clear = true;
                }
            },
            |ui| {
                ui.label(text::caption(
                    ui,
                    t!("history.count", count = entries.len()),
                ));
            },
        );
        ui.add_space(8.0);

        let today = Today {
            today: DateTime::now_local(),
            yesterday: DateTime::yesterday_local(),
        };
        let shown = self.shown.max(PAGE_SIZE).min(entries.len());
        let mut action = None;
        for entry in entries.iter().rev().take(shown) {
            let open = self.expanded.contains(&entry.id);
            if let Some(clicked) = entry_card(ui, entry, open, &today) {
                action = Some(clicked);
            }
            ui.add_space(CARD_GAP);
        }
        let hidden = entries.len() - shown;
        if hidden > 0
            && controls::icon_text_button(
                ui,
                Icon::ChevronDown,
                t!("history.more", count = hidden.min(PAGE_SIZE)),
            )
            .clicked()
        {
            self.shown = shown + PAGE_SIZE;
        }

        match action {
            Some(Action::Copy(text)) => {
                ui.ctx().copy_text(text);
                cx.shared.toasts.info(t!("common.copied"));
            }
            Some(Action::Remove(id)) => {
                if cx.shared.history.remove(id) {
                    self.expanded.remove(&id);
                }
            }
            Some(Action::Toggle(id)) => toggle(&mut self.expanded, id),
            None => {}
        }

        self.confirm_ui(ui, cx);
    }

    /// 기록을 모두 지울지 묻는 창
    fn confirm_ui(&mut self, ui: &Ui, cx: &mut PageContext<'_>) {
        if !self.confirm_clear {
            return;
        }
        let count = cx.shared.history.entries().len();
        let answer = Confirm {
            id: "confirm_clear_history",
            title: &t!("history.clear.title"),
            body: &t!("history.clear.body", count = count),
            confirm_label: &t!("history.clear.confirm"),
            cancel_label: &t!("common.cancel"),
            danger: true,
        }
        .show(ui);
        match answer {
            Some(Answer::Confirm) => {
                cx.shared.history.clear();
                self.expanded.clear();
                self.shown = 0;
                self.confirm_clear = false;
            }
            Some(Answer::Cancel) => self.confirm_clear = false,
            None => {}
        }
    }
}

/// 기록 카드 하나를 그리고, 누른 동작을 돌려줍니다.
///
/// 접혀 있으면 머리 줄에 글 한 줄을 함께 보여 주고, 펼치면 머리 줄에는 시각만 두고 아래에 글을 모두 보여 줍니다.
/// 지우기 버튼은 펼친 내용 맨 아래에 둡니다. 머리 줄에 두면 펼친 뒤 펼치기 버튼이 옆으로 밀려, 접으려고
/// 같은 자리를 다시 누르면 기록이 지워지기 때문입니다.
fn entry_card(ui: &mut Ui, entry: &Entry, open: bool, today: &Today) -> Option<Action> {
    let time = time_label(entry.time, today);
    let summary = text::preview_line(summary_text(entry));
    // 화면 낭독기가 읽을 카드 이름
    let label = format!("{time} {summary}");
    let mut remove = false;
    let mut copy = None;
    let shown = Expander::new(&label, open).show(
        ui,
        ("history_entry", entry.id),
        |ui| entry_head(ui, entry, &time, (!open).then_some(summary.as_str())),
        |_| {},
        |ui| entry_body(ui, entry, &mut copy, &mut remove),
    );
    if remove {
        Some(Action::Remove(entry.id))
    } else if let Some(text) = copy {
        Some(Action::Copy(text))
    } else if shown.toggled {
        Some(Action::Toggle(entry.id))
    } else {
        None
    }
}

/// 접힌 카드에 한 줄로 보여 줄 글: 다듬은 글, 다듬지 않았거나 다듬은 글이 비었으면 받아쓴 원문
fn summary_text(entry: &Entry) -> &str {
    entry
        .refined_text()
        .filter(|refined| !refined.trim().is_empty())
        .unwrap_or(&entry.transcript)
}

/// 카드 머리 줄 왼쪽: 받아쓴 시각, (실패했으면) 다듬기 실패 표시, (접혀 있으면) 글 한 줄
fn entry_head(ui: &mut Ui, entry: &Entry, time: &str, summary: Option<&str>) {
    let palette = Palette::of(ui);
    // 줄 높이를 글자 높이에 맞춰, 머리 줄의 펼치기 버튼과 글의 세로 가운데가 맞게 합니다
    // (`row::title_line`과 같은 방법. 한 줄로 자르려고 줄을 바꾸지 않는 칸을 씁니다).
    ui.spacing_mut().interact_size.y = ui.text_style_height(&TextStyle::Body);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(text::caption(ui, time));
        if let Refinement::Failed { reason, .. } = &entry.refine {
            let badge = controls::tone_badge(
                ui,
                &t!("history.refine_failed"),
                palette.danger_soft,
                palette.danger_text,
            );
            if !reason.trim().is_empty() {
                badge.on_hover_text(reason.as_str());
            }
        }
        match summary {
            Some("") => {
                ui.label(RichText::new("–").color(palette.text_faint));
            }
            // 남은 너비에 한 줄로 넣고 넘치는 부분은 말줄임표로 자릅니다.
            Some(line) => {
                ui.add(Label::new(line).truncate());
            }
            None => {}
        }
    });
}

/// 펼친 카드의 내용: 다듬은 글(다듬었으면)과 받아쓴 원문을 글마다 복사 버튼과 함께 모두 보여 주고,
/// 맨 아래에 기록을 지우는 버튼을 둡니다.
fn entry_body(ui: &mut Ui, entry: &Entry, copy: &mut Option<String>, remove: &mut bool) {
    ui.spacing_mut().item_spacing.y = BODY_SPACING;
    if let Some(refined) = entry.refined_text() {
        let label = match entry.refine.prompt() {
            Some(prompt) => t!("history.refined_with", prompt = prompt),
            None => t!("history.refined"),
        };
        text_block(ui, &label, refined, copy);
        row::separator(ui);
    }
    text_block(ui, &t!("history.transcript"), &entry.transcript, copy);
    ui.add_space(BODY_SPACING);
    row::with_actions(
        ui,
        true,
        |ui| {
            let label = t!("history.remove");
            *remove = controls::icon_text_button(ui, Icon::Trash2, label).clicked();
        },
        |_| {},
    );
}

/// 이름 줄(복사 버튼 포함)과 글 전체
fn text_block(ui: &mut Ui, label: &str, body: &str, copy: &mut Option<String>) {
    let palette = Palette::of(ui);
    row::with_actions(
        ui,
        true,
        |ui| {
            if controls::icon_button(ui, Icon::Copy, &t!("history.copy")).clicked() {
                *copy = Some(body.to_owned());
            }
        },
        |ui| {
            ui.label(text::caption(ui, label));
        },
    );
    // 빈 글(다듬은 결과가 비었을 때)도 자리를 차지하게 줄표를 둡니다.
    if body.trim().is_empty() {
        ui.label(RichText::new("–").color(palette.text_faint));
    } else {
        ui.add(Label::new(body).selectable(true).wrap());
    }
}

/// 펼쳐 둔 기록이면 접고, 접혀 있으면 펼칩니다.
fn toggle(expanded: &mut HashSet<u64>, id: u64) {
    if !expanded.remove(&id) {
        expanded.insert(id);
    }
}

/// 기록이 없을 때의 안내
fn empty_state(ui: &mut Ui) {
    card::card(ui, |ui| {
        ui.label(text::strong(t!("history.empty.title")));
        ui.label(text::muted(ui, t!("history.empty.body")));
    });
}

/// 받아쓴 시각 (예: "오늘 14:05", "11월 16일 09:30")
fn time_label(millis: i64, today: &Today) -> String {
    let at = DateTime::local(millis);
    let time = format!("{:02}:{:02}", at.hour, at.minute);
    if at.same_day(&today.today) {
        t!("history.time.today", time = time)
    } else if at.same_day(&today.yesterday) {
        t!("history.time.yesterday", time = time)
    } else if at.year == today.today.year {
        t!(
            "history.time.date",
            month = at.month,
            day = at.day,
            time = time
        )
    } else {
        t!(
            "history.time.full",
            year = at.year,
            month = at.month,
            day = at.day,
            time = time
        )
    }
    .into_owned()
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;

    fn entry(transcript: &str, refine: Refinement) -> Entry {
        Entry::new(SystemTime::now(), transcript, refine)
    }

    #[test]
    fn summary_shows_the_refined_text_or_else_the_transcript() {
        let done = |text: &str| Refinement::Done {
            text: text.into(),
            prompt: "기본 다듬기".into(),
        };
        assert_eq!(
            summary_text(&entry("받아쓴 글", done("다듬은 글"))),
            "다듬은 글"
        );
        assert_eq!(
            summary_text(&entry("받아쓴 글", Refinement::None)),
            "받아쓴 글",
            "다듬지 않았으면 받아쓴 원문을 보여 줍니다"
        );
        assert_eq!(
            summary_text(&entry(
                "받아쓴 글",
                Refinement::Failed {
                    prompt: String::new(),
                    reason: "연결 실패".into(),
                }
            )),
            "받아쓴 글",
            "다듬기에 실패했으면 받아쓴 원문을 보여 줍니다"
        );
        assert_eq!(
            summary_text(&entry("받아쓴 글", done("  "))),
            "받아쓴 글",
            "다듬은 글이 비었으면 받아쓴 원문을 보여 줍니다"
        );
    }
}
