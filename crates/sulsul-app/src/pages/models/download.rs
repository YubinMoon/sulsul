//! "로컬 모델 내려받기" 탭: 모델과 단계(크기·언어·정밀도)를 차례로 골라 내려받습니다.
//!
//! 드롭다운 줄([`picker_row`]), 고른 모델의 카드([`variant_card`]), 다른 모델의 진행 상황
//! ([`other_download`])은 음성 인식과 글 다듬기 갈래가 함께 씁니다. 맨 앞 드롭다운에서 고르는
//! "묶음"은 음성 인식에서는 엔진, 글 다듬기에서는 모델 계열입니다.

use std::path::{Path, PathBuf};

use egui::{Align, Layout, Ui, vec2};
use rust_i18n::t;
use sulsul_core::stt::{
    EngineContext, EngineKind, ModelAxis, ModelVariant, VariantChoice, axis_options, catalog_axes,
    find_by_choices, format_bytes,
};

use super::ModelsPage;
use super::available::engine_settings;
use super::entries::{EntryKey, available_entries, model_path_field, variant_label};
use super::jobs::{ActiveDownload, DownloadTarget, Downloads, partial_bytes};
use super::parts::{self, Badge};
use crate::i18n::{tr, tr_opt};
use crate::icons::Icon;
use crate::pages::SECTION_GAP;
use crate::session::{PageContext, Shared};
use crate::system;
use crate::theme::Palette;
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card;
use crate::widgets::controls::{self, ComboItem};
use crate::widgets::expander::Expander;
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
    /// 맨 앞 드롭다운에서 고른 묶음 (음성 인식은 엔진 ID, 글 다듬기는 모델 계열 ID)
    group_id: String,
    /// 단계 순서대로, 단계마다 고른 값
    values: Vec<(ModelAxis, String)>,
}

impl Picker {
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// `axis` 단계에서 고른 값
    pub fn value(&self, axis: ModelAxis) -> Option<&str> {
        self.values
            .iter()
            .find(|(a, _)| *a == axis)
            .map(|(_, value)| value.as_str())
    }

    /// 묶음을 고릅니다. 단계 값은 추천 모델(없으면 첫 모델)의 값으로 채웁니다.
    pub fn select_group(&mut self, group_id: &str, catalog: &[ModelVariant]) {
        self.group_id = group_id.to_owned();
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

/// 맨 앞 드롭다운에서 고를 수 있는 묶음 하나 (엔진 또는 모델 계열)
pub struct PickerGroup<'a> {
    pub id: &'a str,
    pub name: &'a str,
}

/// 드롭다운 줄: 왼쪽부터 모델(묶음), 그리고 그 묶음 카탈로그의 단계들.
///
/// `catalog`는 지금 고른 묶음의 카탈로그입니다. 묶음을 다른 것으로 골랐으면 그 ID를 돌려주고,
/// 부르는 쪽에서 [`Picker::select_group`]으로 바꿉니다. `id_salt`는 갈래마다 다르게 줍니다.
pub fn picker_row(
    ui: &mut Ui,
    id_salt: &str,
    picker: &mut Picker,
    groups: &[PickerGroup<'_>],
    catalog: &[ModelVariant],
) -> Option<String> {
    let axes = catalog_axes(catalog);
    let mut switched = None;
    picker_grid(ui, 1 + axes.len(), |ui, index, width| {
        let Some(axis) = index.checked_sub(1).and_then(|i| axes.get(i).copied()) else {
            ui.label(text::caption(ui, t!("models.download.group")));
            let items: Vec<ComboItem<String>> = groups
                .iter()
                .map(|group| ComboItem::new(group.id.to_owned(), group.name))
                .collect();
            let mut group_id = picker.group_id().to_owned();
            let fallback = group_id.clone();
            let changed = controls::combo(
                ui,
                (id_salt, "group"),
                width,
                &mut group_id,
                &items,
                &fallback,
            );
            if changed {
                switched = Some(group_id);
            }
            return;
        };

        ui.label(text::caption(ui, tr(&axis_name_key(axis))));
        // 앞 단계까지 고른 값과 맞는 모델에 있는 값만 고를 수 있습니다.
        let items: Vec<ComboItem<String>> = axis_options(catalog, &picker.before(axis), axis)
            .into_iter()
            .map(|choice| {
                let label = tr_opt(&choice.label_key).unwrap_or_else(|| choice.value.clone());
                ComboItem::new(choice.value.clone(), label)
            })
            .collect();
        let mut value = picker.value(axis).unwrap_or_default().to_owned();
        let fallback = value.clone();
        let changed = controls::combo(
            ui,
            (id_salt, axis.id()),
            width,
            &mut value,
            &items,
            &fallback,
        );
        if changed {
            picker.pick(catalog, axis, &value);
        }
    });
    switched
}

/// 모델 카드에서 누른 버튼
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantClick {
    /// 내려받기 또는 이어받기
    Start,
    /// 내려받기 취소
    Cancel,
    /// "사용 가능 모델에서 보기"
    View,
    /// "모델 폴더 열기"
    OpenFolder,
}

/// 고른 모델의 카드: 이름, 크기, 설치 상태, 설명, 내려받기 버튼과 진행 상황.
///
/// - `target`: 이 모델을 가리키는 내려받기 대상. 지금 받는 중인지, 지난번에 실패했는지를 여기에 견줍니다.
/// - `add_facts`: 설명 아래에 놓을 배지 줄 (엔진·언어, 계열·라이선스 등 갈래마다 다른 정보)
///
/// 누른 버튼을 돌려주고, 실제 동작은 부르는 쪽에서 합니다.
pub fn variant_card(
    ui: &mut Ui,
    variant: &ModelVariant,
    target: &DownloadTarget,
    downloads: &Downloads,
    add_facts: impl FnOnce(&mut Ui),
) -> Option<VariantClick> {
    let installed = variant.is_installed();
    let downloading = downloads.of(target);
    let progress = downloading.and_then(ActiveDownload::progress);
    let is_downloading = downloading.is_some();
    // 내려받다 만 조각이 있으면 이어받을 수 있습니다.
    let resumable = !installed && !is_downloading && partial_bytes(variant) > 0;
    let other_running = downloads.active().is_some() && !is_downloading;

    let mut main_click = None;
    let mut open_folder = false;
    card::card(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        row::title_line(ui, |ui| {
            ui.label(text::section(variant_label(variant)));
            Badge::plain(format_bytes(variant.total_bytes())).show(ui);
            if variant.recommended {
                Badge::accent(t!("models.badge.recommended")).show(ui);
            }
            if installed {
                Badge::success(t!("models.download.state.installed")).show(ui);
            } else if resumable {
                Badge::warning(t!("models.download.state.resumable")).show(ui);
            } else {
                Badge::plain(t!("models.download.state.missing")).show(ui);
            }
        });
        if let Some(description) = tr_opt(&variant.description_key) {
            ui.label(text::muted(ui, description));
        }
        add_facts(ui);

        ui.add_space(2.0);
        row::separator(ui);
        ui.add_space(2.0);
        parts::action_row(
            ui,
            |ui| {
                if is_downloading {
                    if ui.button(t!("common.cancel")).clicked() {
                        main_click = Some(VariantClick::Cancel);
                    }
                } else if installed {
                    let label = t!("models.download.view_available");
                    if controls::icon_text_button(ui, Icon::ListChecks, label).clicked() {
                        main_click = Some(VariantClick::View);
                    }
                } else {
                    let label = if resumable {
                        t!("models.download.resume")
                    } else {
                        t!("models.download.start")
                    };
                    // 내려받기는 두 갈래를 통틀어 한 번에 하나만 합니다.
                    ui.add_enabled_ui(!other_running, |ui| {
                        let start = controls::primary_icon_button(ui, Icon::Download, label)
                            .on_disabled_hover_text(t!("models.download.busy"));
                        if start.clicked() {
                            main_click = Some(VariantClick::Start);
                        }
                    });
                }
            },
            |ui| {
                let label = t!("models.download.open_dir");
                open_folder = controls::icon_text_button(ui, Icon::FolderOpen, label).clicked();
            },
        );
        if let Some(progress) = &progress {
            parts::download_progress(ui, progress);
        }
        if let Some(message) = downloads.failure_of(target) {
            Banner::new(Tone::Error)
                .line(t!("models.download.failed", message = message))
                .show(ui);
        }
    });
    main_click.or(open_folder.then_some(VariantClick::OpenFolder))
}

/// 지금 보고 있는 모델(`shown`)이 아닌 다른 모델을 내려받는 중이면, 그 진행 상황을 카드 아래에 보여 줍니다.
/// 다른 갈래에서 시작한 내려받기도 여기에 나옵니다.
pub fn other_download(ui: &mut Ui, downloads: &Downloads, shown: Option<&DownloadTarget>) {
    let Some(download) = downloads.active() else {
        return;
    };
    if shown == Some(&download.target) {
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

/// 모델 파일이 있는(내려받으면 놓일) 폴더. 파일 위치를 알 수 없으면 `fallback`입니다.
pub fn variant_folder(variant: &ModelVariant, fallback: PathBuf) -> PathBuf {
    variant
        .primary_path()
        .and_then(Path::parent)
        .map_or(fallback, PathBuf::from)
}

/// 폴더를 파일 관리자로 엽니다(없으면 만듭니다). 열지 못하면 알림으로 알립니다.
pub fn open_folder(shared: &mut Shared, folder: &Path) {
    if let Err(message) = system::open_dir(folder) {
        shared
            .toasts
            .error(t!("common.open_failed", message = message));
    }
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
        if engines.iter().all(|e| e.id != self.picker.group_id()) {
            let initial = engines
                .iter()
                .find(|e| e.id == cx.config.stt.engine)
                .unwrap_or(first);
            self.picker.select_group(&initial.id, &initial.catalog);
        }
        let engine = engines
            .iter()
            .find(|e| e.id == self.picker.group_id())
            .unwrap_or(first);
        self.picker.normalize(&engine.catalog);

        ui.label(text::caption(ui, t!("models.download.help.stt")));
        ui.add_space(6.0);
        let groups: Vec<PickerGroup<'_>> = engines
            .iter()
            .map(|e| PickerGroup {
                id: &e.id,
                name: &e.name,
            })
            .collect();
        let switched = picker_row(
            ui,
            "stt_download",
            &mut self.picker,
            &groups,
            &engine.catalog,
        );
        ui.add_space(SECTION_GAP - 6.0);

        let variant = self.picker.variant(&engine.catalog);
        let target = variant.map(|variant| DownloadTarget::stt(&engine.id, &variant.id));
        match (variant, &target) {
            (Some(variant), Some(target)) => self.engine_variant(ui, cx, engine, variant, target),
            _ => {
                ui.label(text::muted(ui, t!("models.download.no_variant")));
            }
        }
        other_download(ui, &self.downloads, target.as_ref());
        self.own_file(ui, cx, engine);

        // 엔진을 바꿨으면 다음 프레임부터 그 엔진의 단계와 모델을 보여 줍니다.
        if let Some(engine) = switched.and_then(|id| engines.iter().find(|e| e.id == id)) {
            self.picker.select_group(&engine.id, &engine.catalog);
        }
    }

    /// 고른 음성 인식 모델의 카드와, 카드에서 누른 버튼의 동작
    fn engine_variant(
        &mut self,
        ui: &mut Ui,
        cx: &mut PageContext<'_>,
        engine: &CatalogEngine,
        variant: &ModelVariant,
        target: &DownloadTarget,
    ) {
        // 인식 언어는 엔진이 아니라 고른 모델을 따릅니다 (같은 엔진에도 영어 전용 모델이 있습니다).
        let languages = cx
            .shared
            .registry
            .variant_languages(&engine.id, &variant.id)
            .ok();
        let click = variant_card(ui, variant, target, &self.downloads, |ui| {
            row::title_line(ui, |ui| {
                controls::badge(ui, &engine.name);
                if let Some(languages) = &languages {
                    controls::badge(ui, &parts::language_support_label(languages));
                }
                if engine.supports_prompt {
                    controls::badge(ui, &t!("models.cap.prompt"));
                }
            });
        });
        match click {
            Some(VariantClick::Start) => {
                self.downloads
                    .start(ui.ctx(), target.clone(), variant, variant_label(variant))
            }
            Some(VariantClick::Cancel) => self.downloads.cancel(),
            Some(VariantClick::View) => {
                self.show_entry(EntryKey::variant(&engine.id, &variant.id));
            }
            Some(VariantClick::OpenFolder) => {
                let folder = variant_folder(variant, cx.shared.paths.models_dir());
                open_folder(cx.shared, &folder);
            }
            None => {}
        }
    }

    /// "가지고 있는 모델 파일 쓰기": 내려받지 않고, 가지고 있는 모델 파일·폴더의 경로를 직접 지정합니다.
    ///
    /// 고른 엔진의 스키마에 그 항목(`model_path`)이 있을 때만 보입니다. 평소에는 접어 둡니다.
    fn own_file(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, engine: &CatalogEngine) {
        let Some(field) = model_path_field(&cx.shared.registry, &engine.id) else {
            return;
        };
        let palette = Palette::of(ui);
        let key = EntryKey::Custom {
            engine_id: engine.id.clone(),
        };
        let title = t!("models.download.own_file.title");
        let mut view = false;

        ui.add_space(SECTION_GAP - 6.0);
        let shown = Expander::new(&title, self.own_file_open).show(
            ui,
            "download_own_file",
            |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(text::strong(&*title));
                ui.label(text::caption(
                    ui,
                    t!("models.download.own_file.description", engine = engine.name),
                ));
            },
            |_| {},
            |ui| {
                engine_settings(ui, cx, &engine.id, std::slice::from_ref(&field), None);
                ui.add_space(4.0);
                ui.label(text::caption(
                    ui,
                    t!("models.download.own_file.note", engine = engine.name),
                ));

                // 경로를 넣었으면 "사용 가능 모델" 목록에 직접 지정한 모델로 나옵니다.
                let engine_ctx = EngineContext {
                    paths: cx.shared.paths.clone(),
                };
                let custom = available_entries(&cx.shared.registry, &engine_ctx, &cx.config.stt)
                    .into_iter()
                    .find(|entry| entry.key == key);
                let Some(custom) = custom else {
                    return;
                };
                if !custom.ready {
                    ui.label(
                        text::caption(ui, t!("models.download.own_file.missing"))
                            .color(palette.warning_text),
                    );
                }
                ui.add_space(6.0);
                let label = t!("models.download.view_available");
                view = controls::icon_text_button(ui, Icon::ListChecks, label).clicked();
            },
        );
        if shown.toggled {
            self.own_file_open = !self.own_file_open;
        }
        if view {
            self.show_entry(key);
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
    fn selecting_a_group_starts_from_the_recommended_variant() {
        let catalog = catalog();
        let mut picker = Picker::default();
        picker.select_group(ENGINE, &catalog);

        assert_eq!(picker.group_id(), ENGINE);
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
        picker.select_group(ENGINE, &catalog);
        assert_eq!(picked_id(&picker, &catalog).as_deref(), Some("tiny"));
    }

    #[test]
    fn later_step_is_reset_when_its_value_disappears() {
        let catalog = catalog();
        let mut picker = Picker::default();
        picker.select_group(ENGINE, &catalog);

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
        picker.select_group(ENGINE, &catalog);
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
        picker.select_group(ENGINE, &catalog);
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
        picker.select_group(ENGINE, &catalog);
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
        picker.select_group(whisper, &catalog);
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
        picker.select_group(ENGINE, &catalog);
        picker.pick(&catalog, ModelAxis::Size, "tiny");

        // 엔진을 바꾸면 앞 엔진에서 고른 값은 버리고 새 카탈로그의 추천 모델부터 시작합니다.
        let other = vec![variant(
            "int8",
            true,
            &[(ModelAxis::Size, "small"), (ModelAxis::Precision, "int8")],
        )];
        picker.select_group("other", &other);
        assert_eq!(picked_id(&picker, &other).as_deref(), Some("int8"));
        assert_eq!(picker.value(ModelAxis::Language), None);
    }
}
