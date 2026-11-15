//! "로컬 모델 내려받기" 탭: 엔진과 단계(크기·언어·정밀도)를 차례로 골라 모델을 내려받습니다.

use std::path::PathBuf;

use egui::{Align, Layout, Ui, vec2};
use rust_i18n::t;
use sulsul_core::stt::{
    EngineContext, EngineKind, ModelAxis, ModelVariant, VariantChoice, axis_options, catalog_axes,
    find_by_choices, format_bytes,
};

use super::entries::{EntryKey, variant_label};
use super::jobs::{ActiveDownload, partial_bytes};
use super::parts;
use super::{DownloadFailure, ModelsPage};
use crate::i18n::{tr, tr_opt};
use crate::icons::Icon;
use crate::pages::SECTION_GAP;
use crate::session::PageContext;
use crate::system;
use crate::theme::Palette;
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card;
use crate::widgets::controls::{self, ComboItem};
use crate::widgets::row;
use crate::widgets::text;
use crate::widgets::tile;

/// 드롭다운 하나의 최소·최대 너비. 한 줄에 다 들어가지 않으면 다음 줄로 넘깁니다.
const PICKER_MIN_WIDTH: f32 = 150.0;
const PICKER_MAX_WIDTH: f32 = 220.0;
/// 드롭다운 사이 간격
const PICKER_GAP: f32 = 10.0;

/// 단계(축)의 이름 번역 키: `models.download.axis.<축 ID>`
pub fn axis_name_key(axis: ModelAxis) -> String {
    format!("models.download.axis.{}", axis.id())
}

/// 내려받을 모델을 고르는 드롭다운들의 값
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Picker {
    engine_id: String,
    /// 단계 순서대로, 단계마다 고른 값
    values: Vec<(ModelAxis, String)>,
}

impl Picker {
    pub fn engine_id(&self) -> &str {
        &self.engine_id
    }

    /// `axis` 단계에서 고른 값
    pub fn value(&self, axis: ModelAxis) -> Option<&str> {
        self.values
            .iter()
            .find(|(a, _)| *a == axis)
            .map(|(_, value)| value.as_str())
    }

    /// 엔진을 고릅니다. 단계 값은 추천 모델(없으면 첫 모델)의 값으로 채웁니다.
    pub fn select_engine(&mut self, engine_id: &str, catalog: &[ModelVariant]) {
        self.engine_id = engine_id.to_owned();
        self.values.clear();
        self.normalize(catalog);
    }

    /// 한 단계의 값을 고릅니다. 그래서 뒤 단계에서 고른 값이 없어졌으면 그 단계는 다시 정합니다.
    pub fn pick(&mut self, catalog: &[ModelVariant], axis: ModelAxis, value: &str) {
        match self.values.iter_mut().find(|(a, _)| *a == axis) {
            Some((_, current)) => value.clone_into(current),
            None => self.values.push((axis, value.to_owned())),
        }
        self.normalize(catalog);
    }

    /// 고른 값을 카탈로그에 맞춥니다. 앞 단계부터 차례로 보면서, 고른 값이 그 단계의 선택지에
    /// 없으면 추천 모델의 값(없으면 첫 선택지)으로 바꿉니다.
    pub fn normalize(&mut self, catalog: &[ModelVariant]) {
        let mut values: Vec<(ModelAxis, String)> = Vec::new();
        for axis in catalog_axes(catalog) {
            let picked = as_picked(&values);
            let options = axis_options(catalog, &picked, axis);
            let kept = self
                .value(axis)
                .filter(|value| options.iter().any(|option| option.value == *value))
                .map(str::to_owned);
            let value = kept.or_else(|| default_value(catalog, &picked, axis, &options));
            if let Some(value) = value {
                values.push((axis, value));
            }
        }
        self.values = values;
    }

    /// `axis`보다 앞 단계에서 고른 값들
    fn before(&self, axis: ModelAxis) -> Vec<(ModelAxis, &str)> {
        self.values
            .iter()
            .take_while(|(a, _)| *a != axis)
            .map(|(a, value)| (*a, value.as_str()))
            .collect()
    }

    /// 고른 값에 맞는 모델
    pub fn variant<'a>(&self, catalog: &'a [ModelVariant]) -> Option<&'a ModelVariant> {
        find_by_choices(catalog, &as_picked(&self.values))
    }
}

fn as_picked(values: &[(ModelAxis, String)]) -> Vec<(ModelAxis, &str)> {
    values
        .iter()
        .map(|(axis, value)| (*axis, value.as_str()))
        .collect()
}

/// 한 단계의 기본값: 앞 단계의 값과 맞는 추천 모델의 값, 없으면 첫 선택지
fn default_value(
    catalog: &[ModelVariant],
    picked: &[(ModelAxis, &str)],
    axis: ModelAxis,
    options: &[&VariantChoice],
) -> Option<String> {
    let matches = |variant: &&ModelVariant| {
        picked.iter().all(|(a, value)| {
            variant
                .choice(*a)
                .is_some_and(|choice| choice.value == *value)
        })
    };
    catalog
        .iter()
        .filter(|variant| variant.recommended)
        .filter(matches)
        .find_map(|variant| variant.choice(axis))
        .or_else(|| options.first().copied())
        .map(|choice| choice.value.clone())
}

/// 내려받을 모델 목록(카탈로그)이 있는 로컬 엔진
struct CatalogEngine {
    id: String,
    name: String,
    supports_prompt: bool,
    catalog: Vec<ModelVariant>,
}

impl ModelsPage {
    pub(super) fn download_tab(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        let engine_ctx = EngineContext {
            paths: cx.shared.paths.clone(),
        };
        let engines: Vec<CatalogEngine> = cx
            .shared
            .registry
            .descriptors()
            .filter(|descriptor| descriptor.kind == EngineKind::Local)
            .filter_map(|descriptor| {
                let catalog = cx
                    .shared
                    .registry
                    .model_variants(&engine_ctx, &descriptor.id)
                    .ok()?;
                (!catalog.is_empty()).then(|| CatalogEngine {
                    id: descriptor.id.clone(),
                    name: tr(&descriptor.name_key),
                    supports_prompt: descriptor.capabilities.supports_prompt,
                    catalog,
                })
            })
            .collect();
        let Some(first) = engines.first() else {
            ui.label(text::muted(ui, t!("models.download.no_engines")));
            return;
        };

        // 처음 열 때는 지금 쓰는 엔진을, 그 엔진에 내려받을 모델이 없으면 첫 엔진을 고릅니다.
        if engines.iter().all(|e| e.id != self.picker.engine_id()) {
            let initial = engines
                .iter()
                .find(|e| e.id == cx.config.stt.engine)
                .unwrap_or(first);
            self.picker.select_engine(&initial.id, &initial.catalog);
        }
        let engine = engines
            .iter()
            .find(|e| e.id == self.picker.engine_id())
            .unwrap_or(first);
        self.picker.normalize(&engine.catalog);

        ui.label(text::caption(ui, t!("models.download.help")));
        ui.add_space(6.0);
        let switched = self.picker_row(ui, &engines, engine);
        ui.add_space(SECTION_GAP - 6.0);

        match self.picker.variant(&engine.catalog) {
            Some(variant) => self.variant_card(ui, cx, engine, variant),
            None => {
                ui.label(text::muted(ui, t!("models.download.no_variant")));
            }
        }
        self.other_download(ui, engine);

        // 엔진을 바꿨으면 다음 프레임부터 그 엔진의 단계와 모델을 보여 줍니다.
        if let Some(engine) = switched.and_then(|id| engines.iter().find(|e| e.id == id)) {
            self.picker.select_engine(&engine.id, &engine.catalog);
        }
    }

    /// 드롭다운 줄: 왼쪽부터 모델(엔진), 그리고 카탈로그의 단계들.
    /// 엔진을 다른 것으로 골랐으면 그 엔진의 ID를 돌려줍니다.
    fn picker_row(
        &mut self,
        ui: &mut Ui,
        engines: &[CatalogEngine],
        engine: &CatalogEngine,
    ) -> Option<String> {
        let axes = catalog_axes(&engine.catalog);
        let mut switched = None;
        picker_grid(ui, 1 + axes.len(), |ui, index, width| {
            let Some(axis) = index.checked_sub(1).and_then(|i| axes.get(i).copied()) else {
                ui.label(text::caption(ui, t!("models.download.engine")));
                let items: Vec<ComboItem<String>> = engines
                    .iter()
                    .map(|e| ComboItem::new(e.id.clone(), e.name.as_str()))
                    .collect();
                let mut engine_id = engine.id.clone();
                let changed = controls::combo(
                    ui,
                    "download_engine",
                    width,
                    &mut engine_id,
                    &items,
                    &engine.name,
                );
                if changed {
                    switched = Some(engine_id);
                }
                return;
            };

            ui.label(text::caption(ui, tr(&axis_name_key(axis))));
            // 앞 단계까지 고른 값과 맞는 모델에 있는 값만 고를 수 있습니다.
            let items: Vec<ComboItem<String>> =
                axis_options(&engine.catalog, &self.picker.before(axis), axis)
                    .into_iter()
                    .map(|choice| {
                        let label =
                            tr_opt(&choice.label_key).unwrap_or_else(|| choice.value.clone());
                        ComboItem::new(choice.value.clone(), label)
                    })
                    .collect();
            let mut value = self.picker.value(axis).unwrap_or_default().to_owned();
            let fallback = value.clone();
            let changed = controls::combo(
                ui,
                ("download_axis", axis.id()),
                width,
                &mut value,
                &items,
                &fallback,
            );
            if changed {
                self.picker.pick(&engine.catalog, axis, &value);
            }
        });
        switched
    }

    /// 고른 모델의 정보와 내려받기 버튼
    fn variant_card(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        engine: &CatalogEngine,
        variant: &ModelVariant,
    ) {
        let palette = Palette::of(ui);
        let name = variant_label(variant);
        let installed = variant.is_installed();
        let downloading = self
            .download
            .as_ref()
            .filter(|download| download.is_for(&engine.id, &variant.id));
        let progress = downloading.and_then(ActiveDownload::progress);
        let is_downloading = downloading.is_some();
        // 내려받다 만 조각이 있으면 이어받을 수 있습니다.
        let resumable = !installed && !is_downloading && partial_bytes(variant) > 0;
        let other_running = self.download.is_some() && !is_downloading;
        let failure = self
            .download_failure
            .as_ref()
            .filter(|failure| failure.engine_id == engine.id && failure.variant_id == variant.id)
            .map(|failure| failure.message.clone());
        let folder: PathBuf = variant
            .primary_path()
            .and_then(|path| path.parent())
            .map_or_else(|| cx.shared.paths.models_dir(), PathBuf::from);
        // 인식 언어는 엔진이 아니라 고른 모델을 따릅니다 (같은 엔진에도 영어 전용 모델이 있습니다).
        let languages = cx
            .shared
            .registry
            .variant_languages(&engine.id, &variant.id)
            .ok();

        let mut start = false;
        let mut cancel = false;
        let mut view = false;
        let mut open_folder = false;
        card::card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            row::title_line(ui, |ui| {
                ui.label(text::section(&name));
                controls::badge(ui, &format_bytes(variant.total_bytes()));
                if variant.recommended {
                    controls::tone_badge(
                        ui,
                        &t!("models.badge.recommended"),
                        palette.accent_soft,
                        palette.accent_text,
                    );
                }
                if installed {
                    controls::tone_badge(
                        ui,
                        &t!("models.download.state.installed"),
                        palette.success_soft,
                        palette.success_text,
                    );
                } else if resumable {
                    controls::tone_badge(
                        ui,
                        &t!("models.download.state.resumable"),
                        palette.warning_soft,
                        palette.warning_text,
                    );
                } else {
                    controls::badge(ui, &t!("models.download.state.missing"));
                }
            });
            if let Some(description) = tr_opt(&variant.description_key) {
                ui.label(text::muted(ui, description));
            }
            row::title_line(ui, |ui| {
                controls::badge(ui, &engine.name);
                if let Some(languages) = &languages {
                    controls::badge(ui, &parts::language_support_label(languages));
                }
                if engine.supports_prompt {
                    controls::badge(ui, &t!("models.cap.prompt"));
                }
            });

            ui.add_space(2.0);
            row::separator(ui);
            ui.add_space(2.0);
            parts::action_row(
                ui,
                |ui| {
                    if is_downloading {
                        cancel = ui.button(t!("common.cancel")).clicked();
                    } else if installed {
                        view = controls::icon_text_button(
                            ui,
                            Icon::ListChecks,
                            t!("models.download.view_available"),
                        )
                        .clicked();
                    } else {
                        let label = if resumable {
                            t!("models.download.resume")
                        } else {
                            t!("models.download.start")
                        };
                        // 내려받기는 한 번에 하나만 합니다.
                        ui.add_enabled_ui(!other_running, |ui| {
                            start = controls::primary_icon_button(ui, Icon::Download, label)
                                .on_disabled_hover_text(t!("models.download.busy"))
                                .clicked();
                        });
                    }
                },
                |ui| {
                    open_folder = controls::icon_text_button(
                        ui,
                        Icon::FolderOpen,
                        t!("models.download.open_dir"),
                    )
                    .clicked();
                },
            );
            if let Some(progress) = &progress {
                parts::download_progress(ui, progress);
            }
            if let Some(message) = &failure {
                Banner::new(Tone::Error)
                    .line(t!("models.download.failed", message = message))
                    .show(ui);
            }
        });

        if start {
            self.start_download(ui.ctx(), &engine.id, variant);
        }
        if cancel && let Some(download) = &self.download {
            download.cancel();
        }
        if view {
            self.show_entry(EntryKey::variant(&engine.id, &variant.id));
        }
        if open_folder && let Err(message) = system::open_dir(&folder) {
            cx.shared
                .toasts
                .error(t!("common.open_failed", message = message));
        }
    }

    /// 지금 보고 있는 모델이 아닌 다른 모델을 내려받는 중이면, 그 진행 상황을 카드 아래에 보여 줍니다.
    fn other_download(&self, ui: &mut Ui, engine: &CatalogEngine) {
        let Some(download) = &self.download else {
            return;
        };
        let shown = self
            .picker
            .variant(&engine.catalog)
            .is_some_and(|variant| download.is_for(&engine.id, &variant.id));
        if shown {
            return;
        }
        let Some(progress) = download.progress() else {
            return;
        };
        ui.add_space(SECTION_GAP - 6.0);
        tile::panel(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            row::with_actions(
                ui,
                true,
                |ui| {
                    if ui.button(t!("common.cancel")).clicked() {
                        download.cancel();
                    }
                },
                |ui| {
                    ui.label(text::strong(t!(
                        "models.download.other",
                        model = download.variant_label
                    )));
                },
            );
            parts::download_progress(ui, &progress);
        });
    }

    fn start_download(
        &mut self,
        egui_ctx: &egui::Context,
        engine_id: &str,
        variant: &ModelVariant,
    ) {
        if self.download.is_some() || variant.missing_files().is_empty() {
            return;
        }
        self.download_failure = None;
        self.download = ActiveDownload::start(egui_ctx, engine_id, variant, variant_label(variant));
        if self.download.is_none() {
            // 작업 스레드를 띄우지 못했습니다.
            self.download_failure = Some(DownloadFailure {
                engine_id: engine_id.to_owned(),
                variant_id: variant.id.clone(),
                message: t!("models.download.crashed").into_owned(),
            });
        }
    }
}

/// 드롭다운 `count`개를 한 줄에 들어가는 만큼씩 나눠 놓습니다. 좁으면 다음 줄로 넘어갑니다.
///
/// `add_cell`은 칸의 순서와 너비를 받아 이름표와 드롭다운을 그립니다.
fn picker_grid(ui: &mut Ui, count: usize, mut add_cell: impl FnMut(&mut Ui, usize, f32)) {
    let available = ui.available_width();
    let fits =
        (((available + PICKER_GAP) / (PICKER_MIN_WIDTH + PICKER_GAP)).floor() as usize).max(1);
    // 줄을 나눠야 하면 줄마다 개수가 비슷하게 나눕니다 (네 개면 3+1이 아니라 2+2).
    let rows = count.div_ceil(fits).max(1);
    let per_row = count.div_ceil(rows).max(1);
    let width = ((available - PICKER_GAP * (per_row - 1) as f32) / per_row as f32)
        .min(PICKER_MAX_WIDTH)
        .floor();
    let mut index = 0;
    while index < count {
        let end = (index + per_row).min(count);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = PICKER_GAP;
            for cell in index..end {
                ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    add_cell(ui, cell, width);
                });
            }
        });
        index = end;
        if index < count {
            ui.add_space(4.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use sulsul_core::stt::{ModelAsset, ModelFile};

    use super::*;

    const ENGINE: &str = "test_engine";

    fn variant(id: &str, recommended: bool, choices: &[(ModelAxis, &str)]) -> ModelVariant {
        ModelVariant {
            id: id.into(),
            name_key: format!("engine.{ENGINE}.models.{id}.name"),
            description_key: format!("engine.{ENGINE}.models.{id}.description"),
            recommended,
            files: vec![ModelFile {
                path: PathBuf::from(format!("{id}.bin")),
                asset: ModelAsset {
                    id: id.into(),
                    file_name: format!("{id}.bin"),
                    url: "https://example.com".into(),
                    size_bytes: 1,
                    sha256: "a".repeat(64),
                },
            }],
            choices: choices
                .iter()
                .map(|(axis, value)| VariantChoice::new(ENGINE, *axis, value))
                .collect(),
        }
    }

    /// 크기 세 가지, 그중 `large`만 양자화(q5)가 있고 추천은 `large` 원본인 카탈로그
    fn catalog() -> Vec<ModelVariant> {
        use ModelAxis::{Language, Precision, Size};
        vec![
            variant(
                "tiny",
                false,
                &[(Size, "tiny"), (Language, "multi"), (Precision, "f16")],
            ),
            variant(
                "large-q5",
                false,
                &[(Size, "large"), (Language, "multi"), (Precision, "q5")],
            ),
            variant(
                "large",
                true,
                &[(Size, "large"), (Language, "multi"), (Precision, "f16")],
            ),
        ]
    }

    fn picked_id(picker: &Picker, catalog: &[ModelVariant]) -> Option<String> {
        picker.variant(catalog).map(|variant| variant.id.clone())
    }

    #[test]
    fn selecting_an_engine_starts_from_the_recommended_variant() {
        let catalog = catalog();
        let mut picker = Picker::default();
        picker.select_engine(ENGINE, &catalog);

        assert_eq!(picker.engine_id(), ENGINE);
        assert_eq!(
            picked_id(&picker, &catalog).as_deref(),
            Some("large"),
            "추천 모델이 처음에 골라져 있어야 합니다"
        );
    }

    #[test]
    fn first_variant_is_used_when_nothing_is_recommended() {
        let mut catalog = catalog();
        for variant in &mut catalog {
            variant.recommended = false;
        }
        let mut picker = Picker::default();
        picker.select_engine(ENGINE, &catalog);
        assert_eq!(picked_id(&picker, &catalog).as_deref(), Some("tiny"));
    }

    #[test]
    fn later_step_is_reset_when_its_value_disappears() {
        let catalog = catalog();
        let mut picker = Picker::default();
        picker.select_engine(ENGINE, &catalog);

        picker.pick(&catalog, ModelAxis::Precision, "q5");
        assert_eq!(picked_id(&picker, &catalog).as_deref(), Some("large-q5"));

        // tiny에는 q5가 없으므로 정밀도가 tiny의 첫 선택지로 바뀝니다.
        picker.pick(&catalog, ModelAxis::Size, "tiny");
        assert_eq!(picker.value(ModelAxis::Precision), Some("f16"));
        assert_eq!(picked_id(&picker, &catalog).as_deref(), Some("tiny"));

        // 다시 large로 가면, 남아 있는 값(f16)이 large에도 있으니 그대로 둡니다.
        picker.pick(&catalog, ModelAxis::Size, "large");
        assert_eq!(picked_id(&picker, &catalog).as_deref(), Some("large"));
    }

    #[test]
    fn reset_step_prefers_the_recommended_value() {
        use ModelAxis::{Precision, Size};
        // large의 선택지는 q5가 먼저지만, 추천 모델의 값은 f16입니다.
        let catalog = vec![
            variant("tiny", false, &[(Size, "tiny"), (Precision, "int8")]),
            variant("large-q5", false, &[(Size, "large"), (Precision, "q5")]),
            variant("large", true, &[(Size, "large"), (Precision, "f16")]),
        ];
        let mut picker = Picker::default();
        picker.select_engine(ENGINE, &catalog);
        picker.pick(&catalog, Size, "tiny");
        assert_eq!(picker.value(Precision), Some("int8"));

        picker.pick(&catalog, Size, "large");
        assert_eq!(
            picker.value(Precision),
            Some("f16"),
            "없어진 값은 추천 모델의 값으로 바뀌어야 합니다"
        );
    }

    #[test]
    fn steps_before_an_axis_exclude_the_axis_itself() {
        let catalog = catalog();
        let mut picker = Picker::default();
        picker.select_engine(ENGINE, &catalog);
        assert!(picker.before(ModelAxis::Size).is_empty());
        assert_eq!(
            picker.before(ModelAxis::Precision),
            [(ModelAxis::Size, "large"), (ModelAxis::Language, "multi")]
        );
    }

    #[test]
    fn catalog_without_steps_resolves_to_its_first_variant() {
        let catalog = vec![variant("only", false, &[])];
        let mut picker = Picker::default();
        picker.select_engine(ENGINE, &catalog);
        assert_eq!(picked_id(&picker, &catalog).as_deref(), Some("only"));
    }

    /// 실제 Whisper 카탈로그에서 언어 단계가 "다국어 / 영어 전용"으로 보이고,
    /// 고른 모델에 따라 언어 배지가 달라지는지 봅니다.
    #[test]
    fn whisper_language_step_offers_multilingual_and_english_only() {
        use sulsul_stt_whisper::models::{ENGLISH_ONLY, MULTILINGUAL};

        rust_i18n::set_locale("ko");
        let whisper = sulsul_stt_whisper::ENGINE_ID;
        let registry = crate::engines::registry();
        let dir = tempfile::tempdir().unwrap();
        let ctx = EngineContext {
            paths: sulsul_core::AppPaths::under(dir.path()),
        };
        let catalog = registry.model_variants(&ctx, whisper).unwrap();

        let mut picker = Picker::default();
        picker.select_engine(whisper, &catalog);
        picker.pick(&catalog, ModelAxis::Size, "tiny");
        let labels: Vec<String> = axis_options(
            &catalog,
            &picker.before(ModelAxis::Language),
            ModelAxis::Language,
        )
        .into_iter()
        .map(|choice| tr(&choice.label_key))
        .collect();
        assert_eq!(labels, ["다국어", "영어 전용"]);

        // 고른 모델의 ID와 카드에 보일 언어 배지
        let shown = |picker: &Picker| {
            let variant = picker.variant(&catalog).unwrap();
            let languages = registry.variant_languages(whisper, &variant.id).unwrap();
            (
                variant.id.clone(),
                parts::language_support_label(&languages),
            )
        };
        picker.pick(&catalog, ModelAxis::Language, MULTILINGUAL);
        assert_eq!(
            shown(&picker),
            ("tiny".to_owned(), "다국어 · 자동 감지".to_owned())
        );
        picker.pick(&catalog, ModelAxis::Language, ENGLISH_ONLY);
        assert_eq!(
            shown(&picker),
            ("tiny-en".to_owned(), "영어 전용".to_owned()),
            "영어 전용 모델을 고르면 배지도 영어 전용이어야 합니다"
        );
    }

    #[test]
    fn stale_values_from_another_catalog_are_replaced() {
        let catalog = catalog();
        let mut picker = Picker::default();
        picker.select_engine(ENGINE, &catalog);
        picker.pick(&catalog, ModelAxis::Size, "tiny");

        // 엔진을 바꾸면 앞 엔진에서 고른 값은 버리고 새 카탈로그의 추천 모델부터 시작합니다.
        let other = vec![variant(
            "int8",
            true,
            &[(ModelAxis::Size, "small"), (ModelAxis::Precision, "int8")],
        )];
        picker.select_engine("other", &other);
        assert_eq!(picked_id(&picker, &other).as_deref(), Some("int8"));
        assert_eq!(picker.value(ModelAxis::Language), None);
    }
}
