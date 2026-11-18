//! 기록 화면: 단축키로 받아쓴 글을 최근 것부터 보여 줍니다.
//!
//! 기록마다 카드 하나를 두고, 받아쓴 원문과 (다듬었으면) 다듬은 글을 따로 보여 주며 각각 복사할 수 있습니다.
//! 복사는 클립보드에 그대로 넣습니다(단축키 받아쓰기처럼 붙여넣은 뒤 원래 내용으로 되돌리지 않음).
//! 긴 글은 몇 줄만 보여 주고 펼치기 버튼으로 전부 봅니다. 기록이 많으면 [`PAGE_SIZE`]개씩 더 보여 줍니다.

use std::collections::HashSet;
use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Galley, Label, TextStyle, Ui};
use rust_i18n::t;

use super::header;
use crate::clock::DateTime;
use crate::history::{Entry, MAX_ENTRIES, Refinement};
use crate::icons::Icon;
use crate::session::PageContext;
use crate::theme::Palette;
use crate::widgets::confirm::{Answer, Confirm};
use crate::widgets::row;
use crate::widgets::{card, controls, text};

/// 한 번에 보여 주는 기록 수 ("더 보기"를 누를 때마다 이만큼 늘림)
const PAGE_SIZE: usize = 50;
/// 접힌 글이 보여 주는 줄 수
const PREVIEW_ROWS: usize = 4;
/// 카드 사이 간격
const CARD_GAP: f32 = 8.0;

#[derive(Default)]
pub struct HistoryPage {
    /// 전부 펼쳐 둔 글 (기록 번호, 어느 글인지)
    expanded: HashSet<(u64, Part)>,
    /// "더 보기"로 늘린 보여 줄 기록 수 (최소 [`PAGE_SIZE`])
    shown: usize,
    /// 전체 지우기를 묻는 중
    confirm_clear: bool,
}

/// 기록 안의 글 하나
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Part {
    Transcript,
    Refined,
}

/// 카드에서 누른 동작. 목록을 다 그린 뒤에 처리합니다.
enum Action {
    Copy(String),
    Remove(u64),
    Toggle(u64, Part),
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
            if let Some(clicked) = self.entry_card(ui, entry, &today) {
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
                    self.expanded.retain(|(entry, _)| *entry != id);
                }
            }
            Some(Action::Toggle(id, part)) => toggle(&mut self.expanded, (id, part)),
            None => {}
        }

        self.confirm_ui(ui, cx);
    }

    /// 기록 카드 하나를 그리고, 누른 동작을 돌려줍니다.
    fn entry_card(&self, ui: &mut Ui, entry: &Entry, today: &Today) -> Option<Action> {
        let mut action = None;
        ui.push_id(("history_entry", entry.id), |ui| {
            card::card(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                match entry.refined_text() {
                    // 다듬은 글과 원문을 따로 보여 주고 각각 복사합니다.
                    Some(refined) => {
                        row::with_actions(
                            ui,
                            true,
                            |ui| remove_button(ui, entry.id, &mut action),
                            |ui| entry_title(ui, entry, today),
                        );
                        let label = match entry.refine.prompt() {
                            Some(prompt) => t!("history.refined_with", prompt = prompt),
                            None => t!("history.refined"),
                        };
                        self.text_block(ui, entry.id, Part::Refined, refined, &mut action, |ui| {
                            ui.label(text::caption(ui, label));
                        });
                        row::separator(ui);
                        self.text_block(
                            ui,
                            entry.id,
                            Part::Transcript,
                            &entry.transcript,
                            &mut action,
                            |ui| {
                                ui.label(text::caption(ui, t!("history.transcript")));
                            },
                        );
                    }
                    // 글이 하나뿐이면 머리 줄에 복사 버튼을 함께 둡니다.
                    None => {
                        let galley =
                            self.body_galley(ui, entry.id, Part::Transcript, &entry.transcript);
                        row::with_actions(
                            ui,
                            true,
                            |ui| {
                                remove_button(ui, entry.id, &mut action);
                                text_actions(
                                    ui,
                                    entry.id,
                                    Part::Transcript,
                                    &entry.transcript,
                                    &galley,
                                    self.is_expanded(entry.id, Part::Transcript),
                                    &mut action,
                                );
                            },
                            |ui| entry_title(ui, entry, today),
                        );
                        ui.add(Label::new(galley).selectable(true));
                    }
                }
            });
        });
        action
    }

    /// 이름 줄(복사·펼치기 버튼 포함)과 글 하나
    fn text_block(
        &self,
        ui: &mut Ui,
        id: u64,
        part: Part,
        body: &str,
        action: &mut Option<Action>,
        add_label: impl FnOnce(&mut Ui),
    ) {
        let galley = self.body_galley(ui, id, part, body);
        let expanded = self.is_expanded(id, part);
        row::with_actions(
            ui,
            true,
            |ui| text_actions(ui, id, part, body, &galley, expanded, action),
            add_label,
        );
        ui.add(Label::new(galley).selectable(true));
    }

    fn is_expanded(&self, id: u64, part: Part) -> bool {
        self.expanded.contains(&(id, part))
    }

    /// 글을 지금 너비에 맞춰 줄을 바꿔 둡니다. 접혀 있으면 [`PREVIEW_ROWS`]줄까지만 둡니다.
    fn body_galley(&self, ui: &Ui, id: u64, part: Part, body: &str) -> Arc<Galley> {
        let palette = Palette::of(ui);
        // 빈 글(다듬은 결과가 비었을 때)도 자리를 차지하게 줄표를 둡니다.
        let (body, color) = if body.trim().is_empty() {
            ("–", palette.text_faint)
        } else {
            (body, palette.text)
        };
        let font = TextStyle::Body.resolve(ui.style());
        let mut job = LayoutJob::simple(body.to_owned(), font, color, ui.available_width());
        if !self.is_expanded(id, part) {
            job.wrap.max_rows = PREVIEW_ROWS;
            job.wrap.overflow_character = Some('…');
        }
        ui.fonts_mut(|fonts| fonts.layout_job(job))
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

/// 카드 머리 줄 왼쪽: 받아쓴 시각과 (실패했으면) 다듬기 실패 표시
fn entry_title(ui: &mut Ui, entry: &Entry, today: &Today) {
    let palette = Palette::of(ui);
    row::title_line(ui, |ui| {
        ui.label(text::strong(time_label(entry.time, today)));
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
    });
}

fn remove_button(ui: &mut Ui, id: u64, action: &mut Option<Action>) {
    if controls::icon_button(ui, Icon::Trash2, &t!("history.remove")).clicked() {
        *action = Some(Action::Remove(id));
    }
}

/// 글 하나의 복사 버튼과 (길어서 접혔거나 펼쳐 둔 글이면) 펼치기·접기 버튼.
/// 오른쪽에서 왼쪽으로 놓이므로 복사 버튼이 오른쪽에 옵니다.
fn text_actions(
    ui: &mut Ui,
    id: u64,
    part: Part,
    body: &str,
    galley: &Galley,
    expanded: bool,
    action: &mut Option<Action>,
) {
    if controls::icon_button(ui, Icon::Copy, &t!("history.copy")).clicked() {
        *action = Some(Action::Copy(body.to_owned()));
    }
    if expanded || galley.elided {
        let (icon, label) = if expanded {
            (Icon::ChevronUp, t!("common.collapse"))
        } else {
            (Icon::ChevronDown, t!("common.expand"))
        };
        if controls::icon_button(ui, icon, &label).clicked() {
            *action = Some(Action::Toggle(id, part));
        }
    }
}

/// 펼쳐 둔 글이면 접고, 접혀 있으면 펼칩니다.
fn toggle(expanded: &mut HashSet<(u64, Part)>, key: (u64, Part)) {
    if !expanded.remove(&key) {
        expanded.insert(key);
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
