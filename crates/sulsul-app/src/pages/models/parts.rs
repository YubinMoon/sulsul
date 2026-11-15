//! 모델 관리 화면의 여러 탭이 함께 쓰는 화면 조각.
//!
//! 테스트 버튼과 진행 상황·결과, 예상 장치 칸, 내려받기 진행 상황, 제공자 입력 칸이 있습니다.

use egui::{
    Align, Color32, CornerRadius, Layout, Rect, RichText, Role, Sense, Spinner, Stroke, StrokeKind,
    TextEdit, Ui, WidgetInfo, vec2,
};
use rust_i18n::t;
use sulsul_core::config::ApiProfile;
use sulsul_core::download::DownloadPhase;
use sulsul_core::stt::{
    ComputeDevice, EngineKind, LanguageSupport, WHISPER_LANGUAGES, format_bytes,
};
use sulsul_stt_cloud::{find_preset, normalize_transcriptions_url};

use super::jobs::{ActiveTest, DownloadSnapshot, TestFailure, TestOutcome, TestTarget};
use crate::accel::{self, AccelEnv, EngineAccel};
use crate::format;
use crate::icons::{self, Icon};
use crate::theme::{self, Palette, radius, size};
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::controls;
use crate::widgets::language::language_name;
use crate::widgets::row::{self, Row};
use crate::widgets::text;
use crate::widgets::tile;

/// 예상 장치 칸에서 항목 이름이 차지하는 너비
const ACCEL_FACT_LABEL_WIDTH: f32 = 64.0;
/// 내려받기 진행 막대의 높이
const PROGRESS_BAR_HEIGHT: f32 = 8.0;

/// 목록의 묶음 제목 (예: "로컬 모델")
pub fn group_title(ui: &mut Ui, title: &str) {
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

/// 테스트 버튼을 누른 결과
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestClick {
    Start,
    Cancel,
}

/// 테스트 버튼. `target`을 테스트하는 중이면 취소 버튼으로 바뀝니다.
///
/// 테스트는 한 번에 하나만 돌리므로, 다른 것을 테스트하는 중이면 누를 수 없습니다.
pub fn test_button(
    ui: &mut Ui,
    label: &str,
    running: Option<&ActiveTest>,
    target: &TestTarget,
) -> Option<TestClick> {
    match running {
        Some(active) if &active.target == target => ui
            .button(t!("common.cancel"))
            .clicked()
            .then_some(TestClick::Cancel),
        other => ui
            .add_enabled_ui(other.is_none(), |ui| {
                controls::icon_text_button(ui, Icon::PlugZap, label)
                    .on_disabled_hover_text(t!("models.test.busy"))
                    .clicked()
            })
            .inner
            .then_some(TestClick::Start),
    }
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
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add(Spinner::new().size(16.0));
        ui.label(text::muted(ui, status));
    });
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
        Err(TestFailure::Cancelled) => {
            Banner::new(Tone::Info)
                .line(t!("models.test.cancelled"))
                .show(ui);
        }
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
        accel_fact(ui, &t!("models.accel.backend"), &backend, palette.text);
        for (kind, names) in &accel.devices {
            if names.is_empty() {
                let missing = t!("models.accel.not_found");
                accel_fact(ui, &kind.label(), &missing, palette.text_faint);
            } else {
                accel_fact(ui, &kind.label(), &names.join(", "), palette.text);
            }
        }
        accel_fact(
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

/// 예상 장치 칸의 한 줄: 왼쪽에 항목 이름, 오른쪽에 값. 값이 길면 줄을 바꿉니다.
fn accel_fact(ui: &mut Ui, label: &str, value: &str, value_color: Color32) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.allocate_ui_with_layout(
            vec2(ACCEL_FACT_LABEL_WIDTH, 0.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(ACCEL_FACT_LABEL_WIDTH);
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

/// 제공자의 이름·주소·API 키·모델 입력 칸. 공급자는 부르는 쪽에서 따로 보여 줍니다.
///
/// `id_salt`는 입력 칸 ID가 다른 제공자의 것과 겹치지 않게 하는 값입니다(예: 제공자 ID).
pub fn profile_fields(ui: &mut Ui, id_salt: &str, profile: &mut ApiProfile, reveal_key: &mut bool) {
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
    let endpoint_preview = normalize_transcriptions_url(&profile.endpoint);
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
            Err(error) => {
                ui.label(
                    text::caption(
                        ui,
                        t!(
                            "models.providers.field.endpoint_invalid",
                            message = error.to_string()
                        ),
                    )
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
    Row::new(&t!("models.providers.field.model")).show_below(ui, |ui| {
        ui.add(
            TextEdit::singleline(&mut profile.model)
                .id_salt((id_salt, "model"))
                .desired_width(f32::INFINITY)
                .hint_text(t!("models.providers.field.model_hint")),
        );
        if let Some(preset) = find_preset(&profile.provider)
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
