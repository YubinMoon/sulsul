//! 내려받을 수 있는 로컬 LLM(GGUF, llama.cpp용) 목록.
//!
//! 받아쓰기 결과의 문장 부호·띄어쓰기·오탈자를 다듬는 용도라, 한국어를 잘하는 작은 instruct
//! 모델만 둡니다. 계열마다 크기(작은 것부터)와 정밀도(`q4_k_m` 기본, `q8_0`)를 고를 수 있습니다.
//!
//! # 갱신 방법
//!
//! 1. 로그인 없이 받을 수 있는(gated가 아닌) 저장소만 씁니다. 공식 저장소가 gated면 `ggml-org`,
//!    `unsloth`, `bartowski`, `lmstudio-community` 같은 신뢰할 만한 미러를 고르고, 나눠진(split)
//!    GGUF는 피합니다. `curl -sI https://huggingface.co/<저장소>/resolve/<커밋>/<파일>`이 401/403 없이
//!    302로 열리는지 확인합니다.
//! 2. `https://huggingface.co/api/models/<저장소>`의 `sha`를 [`LlmModel::revision`]에 적어 고정합니다
//!    (`main`은 내용이 바뀔 수 있음).
//! 3. 크기와 SHA-256은 `https://huggingface.co/api/models/<저장소>/tree/<커밋>`에서 그 파일의 `size`와
//!    `lfs.oid`를 그대로 옮겨 적습니다. 수 GB 파일을 직접 내려받아 확인할 필요는 없습니다.
//! 4. 변형 ID에는 점을 쓰지 않습니다(번역 키 경로가 깨짐). 새 계열·변형·단계 값을 넣었다면
//!    `locales/*.yml`의 `processor.llm_local.families.<계열 ID>.*`, `processor.llm_local.models.<변형 ID>.*`,
//!    `processor.llm_local.choices.<size|precision>.<값 ID>` 문구도 추가합니다.
//!
//! 마지막 확인: 2026-11-16.

use std::path::PathBuf;

use sulsul_core::AppPaths;
use sulsul_core::stt::{ModelAsset, ModelAxis, ModelFile, ModelVariant, VariantChoice};

use crate::LOCAL_PROCESSOR_ID;

/// 기본 정밀도 (4비트 K 양자화)
pub const Q4_K_M: &str = "q4_k_m";

/// 높은 정밀도 (8비트 양자화)
pub const Q8_0: &str = "q8_0";

/// 내려받을 수 있는 로컬 LLM 계열 하나
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmFamily {
    /// 계열 ID (예: `"qwen3_5"`). 점을 쓰지 않습니다.
    pub id: String,
    /// `processor.llm_local.families.<id>.name`
    pub name_key: String,
    /// `processor.llm_local.families.<id>.description`
    pub description_key: String,
    /// 화면에 그대로 보일 라이선스 이름 (예: `"Apache-2.0"`)
    pub license: &'static str,
    /// 모델 카드 URL
    pub homepage: &'static str,
    /// 내려받을 수 있는 변형. `choices`는 [크기, 정밀도] 순서이고 목록은 작은 크기부터입니다.
    pub variants: Vec<ModelVariant>,
}

/// GGUF 파일 하나로 된 변형 정보
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LlmModel {
    /// 변형 ID. 모든 계열에서 겹치지 않고 점이 없습니다 (예: `"qwen3_5-4b-q4_k_m"`).
    pub id: &'static str,
    /// 크기 단계 값 (예: `"4b"`, `"2_4b"`)
    pub size: &'static str,
    /// 정밀도 단계 값 ([`Q4_K_M`] 또는 [`Q8_0`])
    pub precision: &'static str,
    /// Hugging Face 저장소
    pub repo: &'static str,
    /// 크기·해시를 확인한 저장소 커밋
    pub revision: &'static str,
    pub file_name: &'static str,
    pub size_bytes: u64,
    /// 파일 전체의 SHA-256 (소문자 16진수 64자)
    pub sha256: &'static str,
    pub recommended: bool,
}

impl LlmModel {
    /// 커밋에 고정한 내려받기 주소
    pub fn download_url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, self.file_name
        )
    }

    /// 내려받기에 쓰는 공통 형식으로 바꿉니다.
    pub fn asset(&self) -> ModelAsset {
        ModelAsset {
            id: self.id.to_owned(),
            file_name: self.file_name.to_owned(),
            url: self.download_url(),
            size_bytes: self.size_bytes,
            sha256: self.sha256.to_owned(),
        }
    }
}

/// 계열 하나의 고정 정보
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FamilySpec {
    pub id: &'static str,
    pub license: &'static str,
    pub homepage: &'static str,
    /// 작은 크기부터, 같은 크기 안에서는 기본 정밀도([`Q4_K_M`])부터
    pub models: &'static [LlmModel],
}

/// 로컬 LLM 카탈로그. 계열 순서가 화면 순서입니다.
pub const FAMILIES: &[FamilySpec] = &[
    // Qwen3.5 (2026-02). 201개 언어, 작은 크기 중 가장 최신 세대. 기본으로 생각(thinking) 모드라
    // 실행할 때 `enable_thinking=false`로 꺼야 바로 답합니다. 공식 GGUF가 크기별로 다 있지 않아
    // `lmstudio-community`(bartowski 양자화) 것을 씁니다.
    FamilySpec {
        id: "qwen3_5",
        license: "Apache-2.0",
        homepage: "https://huggingface.co/Qwen/Qwen3.5-4B",
        models: &[
            LlmModel {
                id: "qwen3_5-2b-q4_k_m",
                size: "2b",
                precision: Q4_K_M,
                repo: "lmstudio-community/Qwen3.5-2B-GGUF",
                revision: "bb84e11355a036e28f080c7793fa6d22b7c4e344",
                file_name: "Qwen3.5-2B-Q4_K_M.gguf",
                size_bytes: 1_270_808_032,
                sha256: "0bfe35afc9f05b7fac3fa04925e051ac7939a42a8a17ea11afc99701bea826cc",
                recommended: false,
            },
            LlmModel {
                id: "qwen3_5-2b-q8_0",
                size: "2b",
                precision: Q8_0,
                repo: "lmstudio-community/Qwen3.5-2B-GGUF",
                revision: "bb84e11355a036e28f080c7793fa6d22b7c4e344",
                file_name: "Qwen3.5-2B-Q8_0.gguf",
                size_bytes: 2_012_012_000,
                sha256: "30d5d309e77d48b44325fe8eee08f4201f99687752f9a4a77fa66498457b45f4",
                recommended: false,
            },
            LlmModel {
                id: "qwen3_5-4b-q4_k_m",
                size: "4b",
                precision: Q4_K_M,
                repo: "lmstudio-community/Qwen3.5-4B-GGUF",
                revision: "f9f88ac3e234be915e23811a6d28ea287bdb927e",
                file_name: "Qwen3.5-4B-Q4_K_M.gguf",
                size_bytes: 2_707_513_696,
                sha256: "25082a7dd3776cc3c741c6347d3bd04523f05796607b3fbc32fa3a25dfa1418c",
                recommended: true,
            },
            LlmModel {
                id: "qwen3_5-4b-q8_0",
                size: "4b",
                precision: Q8_0,
                repo: "lmstudio-community/Qwen3.5-4B-GGUF",
                revision: "f9f88ac3e234be915e23811a6d28ea287bdb927e",
                file_name: "Qwen3.5-4B-Q8_0.gguf",
                size_bytes: 4_482_402_656,
                sha256: "c3fc7bcaf6f75b8f7ceeead9a769f5a7a9f86a8180af1cfb2b72958dcad8e028",
                recommended: false,
            },
            LlmModel {
                id: "qwen3_5-9b-q4_k_m",
                size: "9b",
                precision: Q4_K_M,
                repo: "lmstudio-community/Qwen3.5-9B-GGUF",
                revision: "1379f25c6b505a3fc737bd7818cb09389cf807c1",
                file_name: "Qwen3.5-9B-Q4_K_M.gguf",
                size_bytes: 5_627_044_256,
                sha256: "cd76ec205963b3b33350093e6904d9de16c4e666fd104e1f632d25c7f15f2a13",
                recommended: false,
            },
            LlmModel {
                id: "qwen3_5-9b-q8_0",
                size: "9b",
                precision: Q8_0,
                repo: "lmstudio-community/Qwen3.5-9B-GGUF",
                revision: "1379f25c6b505a3fc737bd7818cb09389cf807c1",
                file_name: "Qwen3.5-9B-Q8_0.gguf",
                size_bytes: 9_527_501_216,
                sha256: "096a84703a334662086e4b46b6a4dd896bad10a46f67cd84a53cfe1420b24717",
                recommended: false,
            },
        ],
    },
    // Gemma 4 (2026-03, Apache-2.0). E2B·E4B는 층별 임베딩 때문에 매개변수 수에 비해 파일이 큽니다.
    // 생각 모드는 기본으로 꺼져 있습니다. Google 공식 GGUF는 QAT `q4_0`뿐이라
    // `lmstudio-community` 것을 씁니다.
    FamilySpec {
        id: "gemma4",
        license: "Apache-2.0",
        homepage: "https://huggingface.co/google/gemma-4-E4B-it",
        models: &[
            LlmModel {
                id: "gemma4-e2b-q4_k_m",
                size: "e2b",
                precision: Q4_K_M,
                repo: "lmstudio-community/gemma-4-E2B-it-GGUF",
                revision: "eed445ccfb8aef2e279df1a97e133baf06e443b4",
                file_name: "gemma-4-E2B-it-Q4_K_M.gguf",
                size_bytes: 3_427_880_384,
                sha256: "71e6e8cb64a76da1a734fb8f6ba389d2784950d9902f5e87244fca0a15190c94",
                recommended: false,
            },
            LlmModel {
                id: "gemma4-e2b-q8_0",
                size: "e2b",
                precision: Q8_0,
                repo: "lmstudio-community/gemma-4-E2B-it-GGUF",
                revision: "eed445ccfb8aef2e279df1a97e133baf06e443b4",
                file_name: "gemma-4-E2B-it-Q8_0.gguf",
                size_bytes: 4_967_497_152,
                sha256: "1265ad3b888ca0b8b312c4f51358514fd49d4f7ad2f608f141ea9596b641d576",
                recommended: false,
            },
            LlmModel {
                id: "gemma4-e4b-q4_k_m",
                size: "e4b",
                precision: Q4_K_M,
                repo: "lmstudio-community/gemma-4-E4B-it-GGUF",
                revision: "99210a71094c6aa8559f764e0cadedd7d89de94d",
                file_name: "gemma-4-E4B-it-Q4_K_M.gguf",
                size_bytes: 5_335_291_936,
                sha256: "0ffb122c8b6921f13cbc34186e052524d0b5803b17f4867b7197a561400b3770",
                recommended: false,
            },
            LlmModel {
                id: "gemma4-e4b-q8_0",
                size: "e4b",
                precision: Q8_0,
                repo: "lmstudio-community/gemma-4-E4B-it-GGUF",
                revision: "99210a71094c6aa8559f764e0cadedd7d89de94d",
                file_name: "gemma-4-E4B-it-Q8_0.gguf",
                size_bytes: 8_031_240_000,
                sha256: "6f6904e2925d5389ed51390d6c731f752914025fd2d7ad7df9d45f9c24164397",
                recommended: false,
            },
        ],
    },
    // LG AI연구원 EXAONE 3.5 (한국어·영어 특화, 공식 GGUF). EXAONE 4.0은 1.2B·32B, 4.5는 33B뿐이라
    // 1B~9B에서 크기를 둘 이상 고를 수 있는 가장 최신 세대가 3.5입니다. 상업적 사용 불가(NC) 라이선스.
    FamilySpec {
        id: "exaone3_5",
        license: "EXAONE AI Model License Agreement 1.1 - NC",
        homepage: "https://huggingface.co/LGAI-EXAONE/EXAONE-3.5-7.8B-Instruct",
        models: &[
            LlmModel {
                id: "exaone3_5-2_4b-q4_k_m",
                size: "2_4b",
                precision: Q4_K_M,
                repo: "LGAI-EXAONE/EXAONE-3.5-2.4B-Instruct-GGUF",
                revision: "142acae803a41c206e8d0fa978c6102c748911bb",
                file_name: "EXAONE-3.5-2.4B-Instruct-Q4_K_M.gguf",
                size_bytes: 1_644_918_272,
                sha256: "1660a3794acc33437137faab5cb3fb2b2c4191d0a4ec991d4c31a7197cb2cdbf",
                recommended: false,
            },
            LlmModel {
                id: "exaone3_5-2_4b-q8_0",
                size: "2_4b",
                precision: Q8_0,
                repo: "LGAI-EXAONE/EXAONE-3.5-2.4B-Instruct-GGUF",
                revision: "142acae803a41c206e8d0fa978c6102c748911bb",
                file_name: "EXAONE-3.5-2.4B-Instruct-Q8_0.gguf",
                size_bytes: 2_838_845_952,
                sha256: "464d3b40dabdc0fb0d1c05c84d51372bc7da44e038708e6924dd2bd4c9128a35",
                recommended: false,
            },
            LlmModel {
                id: "exaone3_5-7_8b-q4_k_m",
                size: "7_8b",
                precision: Q4_K_M,
                repo: "LGAI-EXAONE/EXAONE-3.5-7.8B-Instruct-GGUF",
                revision: "c618bf67338171760c72c3f109f2900cb7d79855",
                file_name: "EXAONE-3.5-7.8B-Instruct-Q4_K_M.gguf",
                size_bytes: 4_770_649_728,
                sha256: "2f4aff8d555b82500d4de2723810586e83e358003388fdd08bd52e6091e6ddd1",
                recommended: false,
            },
            LlmModel {
                id: "exaone3_5-7_8b-q8_0",
                size: "7_8b",
                precision: Q8_0,
                repo: "LGAI-EXAONE/EXAONE-3.5-7.8B-Instruct-GGUF",
                revision: "c618bf67338171760c72c3f109f2900cb7d79855",
                file_name: "EXAONE-3.5-7.8B-Instruct-Q8_0.gguf",
                size_bytes: 8_312_084_096,
                sha256: "afbdbe36262d563a9d32313a24770feee91239019b425c0c3fa952f78485d486",
                recommended: false,
            },
        ],
    },
];

/// 로컬 LLM 파일을 두는 폴더 (`delete_variant`의 `root`로도 씁니다).
/// 음성 인식 모델(`models/` 바로 아래, `models/sense_voice/`)과 겹치지 않게 `models/llm/`을 씁니다.
pub fn models_root(paths: &AppPaths) -> PathBuf {
    paths.models_dir().join("llm")
}

/// 내려받을 수 있는 로컬 LLM 계열 목록 (화면 순서)
pub fn families(paths: &AppPaths) -> Vec<LlmFamily> {
    let root = models_root(paths);
    FAMILIES
        .iter()
        .map(|spec| LlmFamily {
            id: spec.id.to_owned(),
            name_key: format!("processor.{LOCAL_PROCESSOR_ID}.families.{}.name", spec.id),
            description_key: format!(
                "processor.{LOCAL_PROCESSOR_ID}.families.{}.description",
                spec.id
            ),
            license: spec.license,
            homepage: spec.homepage,
            variants: spec
                .models
                .iter()
                .map(|model| variant(&root, spec.id, model))
                .collect(),
        })
        .collect()
}

/// 변형 ID로 계열과 변형을 찾습니다.
pub fn find_variant(paths: &AppPaths, variant_id: &str) -> Option<(LlmFamily, ModelVariant)> {
    families(paths).into_iter().find_map(|family| {
        let variant = family.variants.iter().find(|v| v.id == variant_id)?.clone();
        Some((family, variant))
    })
}

/// 정해진 변형 정보 (파일 위치 없이)
pub fn find_model(variant_id: &str) -> Option<&'static LlmModel> {
    FAMILIES
        .iter()
        .flat_map(|spec| spec.models)
        .find(|model| model.id == variant_id)
}

fn variant(root: &std::path::Path, family_id: &str, model: &LlmModel) -> ModelVariant {
    ModelVariant {
        id: model.id.to_owned(),
        name_key: format!("processor.{LOCAL_PROCESSOR_ID}.models.{}.name", model.id),
        description_key: format!(
            "processor.{LOCAL_PROCESSOR_ID}.models.{}.description",
            model.id
        ),
        recommended: model.recommended,
        files: vec![ModelFile {
            path: root.join(family_id).join(model.file_name),
            asset: model.asset(),
        }],
        choices: vec![
            choice(ModelAxis::Size, model.size),
            choice(ModelAxis::Precision, model.precision),
        ],
    }
}

/// `processor.llm_local.choices.<축 ID>.<값 ID>` 번역 키를 가진 단계 값
fn choice(axis: ModelAxis, value: &str) -> VariantChoice {
    VariantChoice {
        axis,
        value: value.to_owned(),
        label_key: format!(
            "processor.{LOCAL_PROCESSOR_ID}.choices.{}.{value}",
            axis.id()
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use sulsul_core::stt::{axis_options, catalog_axes, find_by_choices};

    use super::*;

    fn paths() -> AppPaths {
        AppPaths::under("unused-root")
    }

    fn all_models() -> impl Iterator<Item = &'static LlmModel> {
        FAMILIES.iter().flat_map(|spec| spec.models)
    }

    #[test]
    fn catalog_size_is_within_plan() {
        assert!((3..=4).contains(&FAMILIES.len()), "계열은 3~4개여야 합니다");
        let total = all_models().count();
        assert!(
            (10..=16).contains(&total),
            "변형은 모두 10~16개여야 합니다: {total}"
        );
        for spec in FAMILIES {
            let sizes: HashSet<_> = spec.models.iter().map(|m| m.size).collect();
            assert!(
                (2..=3).contains(&sizes.len()),
                "{}: 크기는 2~3개여야 합니다",
                spec.id
            );
        }
    }

    #[test]
    fn ids_are_unique_and_have_no_dots() {
        let mut seen = HashSet::new();
        for model in all_models() {
            assert!(seen.insert(model.id), "{}: 변형 ID가 겹칩니다", model.id);
            for value in [model.id, model.size, model.precision] {
                assert!(
                    !value.contains('.'),
                    "{value}: 점이 있으면 번역 키 경로가 깨집니다"
                );
            }
        }
        for spec in FAMILIES {
            assert!(
                !spec.id.contains('.'),
                "{}: 계열 ID에 점이 있습니다",
                spec.id
            );
            for model in spec.models {
                assert!(
                    model.id.starts_with(&format!("{}-", spec.id)),
                    "{}: 변형 ID는 계열 ID로 시작해야 합니다",
                    model.id
                );
            }
        }
    }

    #[test]
    fn entries_are_well_formed_and_pinned() {
        for model in all_models() {
            assert!(model.size_bytes > 0, "{}", model.id);
            assert_eq!(model.sha256.len(), 64, "{}", model.id);
            assert!(
                model
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{}: SHA-256은 소문자 16진수여야 합니다",
                model.id
            );
            assert_eq!(model.revision.len(), 40, "{}", model.id);
            assert!(model.revision.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(model.file_name.ends_with(".gguf"), "{}", model.id);
            assert!(
                [Q4_K_M, Q8_0].contains(&model.precision),
                "{}: 알 수 없는 정밀도",
                model.id
            );
            let url = model.download_url();
            assert_eq!(
                url,
                format!(
                    "https://huggingface.co/{}/resolve/{}/{}",
                    model.repo, model.revision, model.file_name
                )
            );
            assert!(
                !url.contains("/resolve/main/"),
                "{}: main에 고정하면 안 됩니다",
                model.id
            );
        }
        for spec in FAMILIES {
            assert!(!spec.license.is_empty(), "{}", spec.id);
            assert!(spec.homepage.starts_with("https://"), "{}", spec.id);
        }
    }

    #[test]
    fn exactly_one_recommended_small_default_precision_variant() {
        let recommended: Vec<_> = all_models().filter(|m| m.recommended).collect();
        assert_eq!(recommended.len(), 1, "추천 변형은 하나여야 합니다");
        assert_eq!(recommended[0].precision, Q4_K_M);
        assert_eq!(recommended[0].id, "qwen3_5-4b-q4_k_m");
    }

    #[test]
    fn families_build_variants_with_keys_paths_and_choices() {
        let paths = paths();
        let families = families(&paths);
        assert_eq!(families.len(), FAMILIES.len());
        for family in &families {
            assert_eq!(
                family.name_key,
                format!("processor.llm_local.families.{}.name", family.id)
            );
            assert_eq!(
                family.description_key,
                format!("processor.llm_local.families.{}.description", family.id)
            );
            assert_eq!(
                catalog_axes(&family.variants),
                [ModelAxis::Size, ModelAxis::Precision],
                "{}: 단계 순서는 크기 → 정밀도여야 합니다",
                family.id
            );
            for variant in &family.variants {
                assert_eq!(
                    variant.name_key,
                    format!("processor.llm_local.models.{}.name", variant.id)
                );
                assert_eq!(
                    variant.description_key,
                    format!("processor.llm_local.models.{}.description", variant.id)
                );
                for choice in &variant.choices {
                    assert_eq!(
                        choice.label_key,
                        format!(
                            "processor.llm_local.choices.{}.{}",
                            choice.axis.id(),
                            choice.value
                        )
                    );
                }
                assert_eq!(variant.files.len(), 1, "GGUF 한 파일이어야 합니다");
                let path = variant.primary_path().unwrap();
                assert!(
                    path.starts_with(models_root(&paths)),
                    "{}: 파일은 models_root 아래에 있어야 합니다",
                    variant.id
                );
                assert!(!variant.is_installed());
            }
            // 기본 정밀도가 먼저 나와야 합니다.
            let precisions = axis_options(&family.variants, &[], ModelAxis::Precision);
            assert_eq!(precisions[0].value, Q4_K_M, "{}", family.id);
        }
    }

    #[test]
    fn models_root_is_separate_from_speech_models() {
        let paths = paths();
        let root = models_root(&paths);
        assert_eq!(root, paths.models_dir().join("llm"));
        assert_ne!(root, paths.models_dir());
    }

    #[test]
    fn find_variant_returns_family_and_variant() {
        let paths = paths();
        let (family, variant) = find_variant(&paths, "gemma4-e4b-q8_0").unwrap();
        assert_eq!(family.id, "gemma4");
        assert_eq!(variant.id, "gemma4-e4b-q8_0");
        assert_eq!(
            variant.choice(ModelAxis::Size).unwrap().value,
            "e4b",
            "크기 단계 값이 맞아야 합니다"
        );
        let picked = find_by_choices(
            &family.variants,
            &[(ModelAxis::Size, "e4b"), (ModelAxis::Precision, Q8_0)],
        )
        .unwrap();
        assert_eq!(picked.id, variant.id);
        assert!(find_variant(&paths, "nope").is_none());
        assert_eq!(find_model("exaone3_5-7_8b-q4_k_m").unwrap().size, "7_8b");
    }
}
