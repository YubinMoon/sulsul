//! SenseVoice (ONNX) 모델 목록과 다운로드 메타데이터.

use std::path::{Path, PathBuf};

use sulsul_core::AppPaths;
use sulsul_core::stt::{ModelAsset, ModelAxis, ModelFile, ModelVariant, VariantChoice};

use crate::ENGINE_ID;

const REPO: &str = "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17";

/// 내려받기 화면의 크기 단계 값. SenseVoice는 Small 모델 하나뿐입니다.
pub const MODEL_SIZE: &str = "small";
/// 내려받기 화면의 언어 단계 값. 한·영·중·일·광둥어를 함께 인식하는 다국어 모델입니다.
pub const MODEL_LANGUAGE: &str = "multi";
const REPO_REVISION: &str = "2365baeacb507f821a0c8120fcee3d484dba7a07";

pub const DEFAULT_MODEL: &str = "int8";
pub const MODEL_SUBDIR: &str = "sense_voice";
pub const TOKENS_FILE: &str = "tokens.txt";
pub const TOKENS_SIZE_BYTES: u64 = 315_894;
pub const TOKENS_SHA256: &str = "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc";

#[derive(Debug, Clone, Copy)]
pub struct SenseVoiceVariant {
    pub id: &'static str,
    /// 내려받기 화면의 정밀도 단계 값 (예: `int8`, `fp32`)
    pub precision: &'static str,
    pub model_file_name: &'static str,
    pub model_size_bytes: u64,
    pub model_sha256: &'static str,
    pub recommended: bool,
}

pub const VARIANTS: &[SenseVoiceVariant] = &[
    SenseVoiceVariant {
        id: "int8",
        precision: "int8",
        model_file_name: "model.int8.onnx",
        model_size_bytes: 239_233_841,
        model_sha256: "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51",
        recommended: true,
    },
    SenseVoiceVariant {
        id: "fp32",
        precision: "fp32",
        model_file_name: "model.onnx",
        model_size_bytes: 937_617_178,
        model_sha256: "977016bd9c79f9eb343430b5cc305e07ab64d5212dff41b0dcfa1694bee9a8cb",
        recommended: false,
    },
];

pub fn find_variant(id: &str) -> Option<&'static SenseVoiceVariant> {
    VARIANTS.iter().find(|v| v.id == id)
}

pub fn default_model_dir(paths: &AppPaths) -> PathBuf {
    paths.models_dir().join(MODEL_SUBDIR)
}

pub fn file_url(file_name: &str) -> String {
    format!("https://huggingface.co/{REPO}/resolve/{REPO_REVISION}/{file_name}")
}

impl SenseVoiceVariant {
    pub fn model_asset(&self) -> ModelAsset {
        ModelAsset {
            id: self.id.to_owned(),
            file_name: self.model_file_name.to_owned(),
            url: file_url(self.model_file_name),
            size_bytes: self.model_size_bytes,
            sha256: self.model_sha256.to_owned(),
        }
    }

    pub fn tokens_asset() -> ModelAsset {
        ModelAsset {
            id: TOKENS_FILE.to_owned(),
            file_name: TOKENS_FILE.to_owned(),
            url: file_url(TOKENS_FILE),
            size_bytes: TOKENS_SIZE_BYTES,
            sha256: TOKENS_SHA256.to_owned(),
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.model_size_bytes + TOKENS_SIZE_BYTES
    }

    pub fn size_label(&self) -> String {
        format_size(self.total_bytes())
    }

    /// 내려받기 화면의 단계별 선택 값 (크기 → 언어 → 정밀도 순서)
    pub fn choices(&self) -> Vec<VariantChoice> {
        [
            (ModelAxis::Size, MODEL_SIZE),
            (ModelAxis::Language, MODEL_LANGUAGE),
            (ModelAxis::Precision, self.precision),
        ]
        .into_iter()
        .map(|(axis, value)| VariantChoice::new(ENGINE_ID, axis, value))
        .collect()
    }

    pub fn to_catalog_variant(&self, dir: &Path) -> ModelVariant {
        ModelVariant {
            id: self.id.to_owned(),
            name_key: format!("engine.{ENGINE_ID}.models.{}.name", self.id),
            description_key: format!("engine.{ENGINE_ID}.models.{}.description", self.id),
            recommended: self.recommended,
            files: vec![
                ModelFile {
                    path: dir.join(self.model_file_name),
                    asset: self.model_asset(),
                },
                ModelFile {
                    path: dir.join(TOKENS_FILE),
                    asset: Self::tokens_asset(),
                },
            ],
            choices: self.choices(),
        }
    }
}

fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value >= KIB * KIB * KIB {
        format!("{:.1} GB", value / (KIB * KIB * KIB))
    } else if value >= KIB * KIB {
        format!("{:.0} MB", value / (KIB * KIB))
    } else {
        format!("{:.0} KB", (value / KIB).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulsul_core::stt::{axis_options, catalog_axes, find_by_choices};

    #[test]
    fn default_variant_exists_and_hashes_are_valid() {
        assert!(find_variant(DEFAULT_MODEL).is_some());
        assert_eq!(REPO_REVISION.len(), 40);
        assert!(REPO_REVISION.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(TOKENS_SHA256.len(), 64);
        assert!(TOKENS_SHA256.bytes().all(|b| b.is_ascii_hexdigit()));
        for v in VARIANTS {
            assert!(v.model_size_bytes > 0);
            assert_eq!(v.model_sha256.len(), 64);
            assert!(v.model_sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(!file_url(v.model_file_name).contains("/resolve/main/"));
        }
    }

    #[test]
    fn catalog_variant_includes_model_and_tokens() {
        let dir = Path::new("models/sense_voice");
        let variant = find_variant("int8").unwrap().to_catalog_variant(dir);
        assert_eq!(variant.files.len(), 2);
        assert_eq!(variant.files[0].path, dir.join("model.int8.onnx"));
        assert_eq!(variant.files[1].path, dir.join("tokens.txt"));
    }

    fn catalog() -> Vec<ModelVariant> {
        let dir = Path::new("models/sense_voice");
        VARIANTS.iter().map(|v| v.to_catalog_variant(dir)).collect()
    }

    fn picked(variant: &ModelVariant) -> Vec<(ModelAxis, &str)> {
        variant
            .choices
            .iter()
            .map(|c| (c.axis, c.value.as_str()))
            .collect()
    }

    #[test]
    fn every_variant_has_same_axes_in_order() {
        let catalog = catalog();
        assert_eq!(
            catalog_axes(&catalog),
            [ModelAxis::Size, ModelAxis::Language, ModelAxis::Precision]
        );
        for variant in &catalog {
            let axes: Vec<_> = variant.choices.iter().map(|c| c.axis).collect();
            assert_eq!(
                axes,
                catalog_axes(&catalog),
                "{}: 단계 순서가 다른 변형과 다릅니다",
                variant.id
            );
        }
    }

    #[test]
    fn choice_combinations_are_unique_and_findable() {
        let catalog = catalog();
        let mut seen = std::collections::HashSet::new();
        for variant in &catalog {
            let picks = picked(variant);
            assert!(
                seen.insert(picks.clone()),
                "{}: 같은 (크기, 언어, 정밀도) 조합을 가진 변형이 또 있습니다",
                variant.id
            );
            let found = find_by_choices(&catalog, &picks).unwrap();
            assert_eq!(
                found.id, variant.id,
                "단계 값으로 찾은 변형이 {}가 아닙니다",
                variant.id
            );
        }
    }

    #[test]
    fn choice_values_and_label_keys() {
        let catalog = catalog();
        let expected = [("int8", "int8"), ("fp32", "fp32")];
        assert_eq!(catalog.len(), expected.len());
        for (variant, (id, precision)) in catalog.iter().zip(expected) {
            assert_eq!(variant.id, id, "변형 목록 순서가 다릅니다");
            let value = |axis| variant.choice(axis).unwrap().value.as_str();
            assert_eq!(value(ModelAxis::Size), "small", "{id}: 크기 값");
            assert_eq!(value(ModelAxis::Language), "multi", "{id}: 언어 값");
            assert_eq!(value(ModelAxis::Precision), precision, "{id}: 정밀도 값");
            for choice in &variant.choices {
                assert_eq!(
                    choice.label_key,
                    format!(
                        "engine.sense_voice_local.choices.{}.{}",
                        choice.axis.id(),
                        choice.value
                    ),
                    "{id}: 번역 키 규칙이 다릅니다"
                );
            }
        }
        let precisions: Vec<_> = axis_options(&catalog, &[], ModelAxis::Precision)
            .into_iter()
            .map(|c| c.value.as_str())
            .collect();
        assert_eq!(precisions, ["int8", "fp32"]);
    }
}
