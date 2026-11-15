//! 내려받을 수 있는 whisper.cpp GGML 모델 목록.
//!
//! 모델을 추가하려면 [`MODELS`]에 한 줄을 더합니다. 크기와 SHA-256은 고정한 리비전의
//! Hugging Face API (`https://huggingface.co/api/models/ggerganov/whisper.cpp/tree/<REPO_REVISION>`)에서
//! 그 파일의 `size`와 `lfs.oid` 값을 그대로 옮겨 적습니다.
//!
//! 목록 순서가 내려받기 화면의 단계별 선택지 순서가 되므로 작은 모델부터 적고, 같은 크기 안에서는
//! 다국어 모델 바로 뒤에 영어 전용 모델을, 작은 양자화 파일을 먼저 둡니다. 모델 ID에는 점을 쓰지
//! 않습니다(번역 키 경로가 깨짐). 새 단계 값을 넣었다면 `locales/*.yml`의
//! `engine.whisper_local.choices.<축 ID>.<값 ID>` 문구도 추가합니다.

use sulsul_core::stt::{LanguageSupport, ModelAsset, ModelAxis, VariantChoice, WHISPER_LANGUAGES};

use crate::ENGINE_ID;

pub const DEFAULT_MODEL: &str = "large-v3";

/// 모델 파일을 받는 Hugging Face 저장소
const REPO: &str = "ggerganov/whisper.cpp";

/// 내려받는 저장소 리비전(커밋). `main`은 내용이 바뀔 수 있어, 크기·해시를 확인한
/// 시점의 커밋에 고정합니다.
const REPO_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

/// 다국어 모델의 언어 단계 값
pub const MULTILINGUAL: &str = "multi";

/// 영어 전용(`*.en`) 모델의 언어 단계 값
pub const ENGLISH_ONLY: &str = "en";

/// 내려받을 수 있는 whisper.cpp GGML 모델
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperModel {
    pub id: &'static str,
    /// 내려받기 화면의 크기 단계 값 (예: `large-v3-turbo`)
    pub size: &'static str,
    /// 내려받기 화면의 언어 단계 값. 공식 다국어 모델은 [`MULTILINGUAL`], 영어 전용 모델은
    /// [`ENGLISH_ONLY`]입니다.
    pub language: &'static str,
    /// 내려받기 화면의 정밀도 단계 값 (예: 원본 `f16`, 양자화 `q5_0`)
    pub precision: &'static str,
    pub file_name: &'static str,
    pub size_bytes: u64,
    /// 파일 전체의 SHA-256 (소문자 16진수 64자)
    pub sha256: &'static str,
    pub recommended: bool,
}

/// whisper.cpp 공식 GGML 모델 카탈로그 (`ggerganov/whisper.cpp`).
pub const MODELS: &[WhisperModel] = &[
    WhisperModel {
        id: "tiny",
        size: "tiny",
        language: MULTILINGUAL,
        precision: "f16",
        file_name: "ggml-tiny.bin",
        size_bytes: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        recommended: false,
    },
    WhisperModel {
        id: "tiny-en",
        size: "tiny",
        language: ENGLISH_ONLY,
        precision: "f16",
        file_name: "ggml-tiny.en.bin",
        size_bytes: 77_704_715,
        sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
        recommended: false,
    },
    WhisperModel {
        id: "base",
        size: "base",
        language: MULTILINGUAL,
        precision: "f16",
        file_name: "ggml-base.bin",
        size_bytes: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        recommended: false,
    },
    WhisperModel {
        id: "base-en",
        size: "base",
        language: ENGLISH_ONLY,
        precision: "f16",
        file_name: "ggml-base.en.bin",
        size_bytes: 147_964_211,
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
        recommended: false,
    },
    WhisperModel {
        id: "small",
        size: "small",
        language: MULTILINGUAL,
        precision: "f16",
        file_name: "ggml-small.bin",
        size_bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        recommended: false,
    },
    WhisperModel {
        id: "small-en",
        size: "small",
        language: ENGLISH_ONLY,
        precision: "f16",
        file_name: "ggml-small.en.bin",
        size_bytes: 487_614_201,
        sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
        recommended: false,
    },
    WhisperModel {
        id: "medium",
        size: "medium",
        language: MULTILINGUAL,
        precision: "f16",
        file_name: "ggml-medium.bin",
        size_bytes: 1_533_763_059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        recommended: false,
    },
    WhisperModel {
        id: "medium-en",
        size: "medium",
        language: ENGLISH_ONLY,
        precision: "f16",
        file_name: "ggml-medium.en.bin",
        size_bytes: 1_533_774_781,
        sha256: "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
        recommended: false,
    },
    WhisperModel {
        id: "large-v3-turbo-q5_0",
        size: "large-v3-turbo",
        language: MULTILINGUAL,
        precision: "q5_0",
        file_name: "ggml-large-v3-turbo-q5_0.bin",
        size_bytes: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        recommended: false,
    },
    WhisperModel {
        id: "large-v3-turbo",
        size: "large-v3-turbo",
        language: MULTILINGUAL,
        precision: "f16",
        file_name: "ggml-large-v3-turbo.bin",
        size_bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        recommended: true,
    },
    WhisperModel {
        id: "large-v3",
        size: "large-v3",
        language: MULTILINGUAL,
        precision: "f16",
        file_name: "ggml-large-v3.bin",
        size_bytes: 3_095_033_483,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
        recommended: false,
    },
];

impl WhisperModel {
    pub fn download_url(&self) -> String {
        format!(
            "https://huggingface.co/{REPO}/resolve/{REPO_REVISION}/{}",
            self.file_name
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

    /// 화면에 보일 파일 크기 (예: `2.9 GB`)
    pub fn size_label(&self) -> String {
        format_size(self.size_bytes)
    }

    /// 영어 전용(`*.en`) 모델인지
    pub fn is_english_only(&self) -> bool {
        self.language == ENGLISH_ONLY
    }

    /// 이 모델로 인식할 수 있는 언어. 영어 전용 모델은 영어만, 다국어 모델은 자동 감지와
    /// whisper.cpp의 모든 언어입니다.
    pub fn languages(&self) -> LanguageSupport {
        if self.is_english_only() {
            LanguageSupport::Only(vec![ENGLISH_ONLY.to_owned()])
        } else {
            multilingual()
        }
    }

    /// 내려받기 화면의 단계별 선택 값 (크기 → 언어 → 정밀도 순서)
    pub fn choices(&self) -> Vec<VariantChoice> {
        [
            (ModelAxis::Size, self.size),
            (ModelAxis::Language, self.language),
            (ModelAxis::Precision, self.precision),
        ]
        .into_iter()
        .map(|(axis, value)| VariantChoice::new(ENGINE_ID, axis, value))
        .collect()
    }
}

pub fn find_model(id: &str) -> Option<&'static WhisperModel> {
    MODELS.iter().find(|m| m.id == id)
}

/// 다국어 모델(과 직접 지정한 모델 파일)이 인식할 수 있는 언어
pub fn multilingual() -> LanguageSupport {
    LanguageSupport::AutoAnd(WHISPER_LANGUAGES.iter().map(|&c| c.to_owned()).collect())
}

/// 바이트 수를 사람이 읽기 쉬운 단위로 바꿉니다 (1024 단위, Windows 탐색기와 같은 기준).
fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value >= KIB * KIB * KIB {
        format!("{:.1} GB", value / (KIB * KIB * KIB))
    } else if value >= KIB * KIB {
        format!("{:.0} MB", value / (KIB * KIB))
    } else if value >= KIB {
        format!("{:.0} KB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_is_listed() {
        assert!(find_model(DEFAULT_MODEL).is_some());
    }

    #[test]
    fn entries_are_well_formed() {
        for model in MODELS {
            assert!(model.size_bytes > 0, "{}", model.id);
            assert_eq!(model.sha256.len(), 64, "{}", model.id);
            assert!(
                model
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{}",
                model.id
            );
        }
    }

    #[test]
    fn url_is_pinned_to_revision() {
        let model = find_model(DEFAULT_MODEL).unwrap();
        let url = model.download_url();
        assert_eq!(
            url,
            format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/{REPO_REVISION}/ggml-large-v3.bin"
            )
        );
        assert!(!url.contains("/resolve/main/"));
        assert_eq!(REPO_REVISION.len(), 40);
        assert!(REPO_REVISION.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn asset_copies_entry() {
        let model = find_model(DEFAULT_MODEL).unwrap();
        let asset = model.asset();
        assert_eq!(asset.id, model.id);
        assert_eq!(asset.file_name, model.file_name);
        assert_eq!(asset.url, model.download_url());
        assert_eq!(asset.size_bytes, model.size_bytes);
        assert_eq!(asset.sha256, model.sha256);
    }

    #[test]
    fn english_only_models_support_only_english() {
        let english: Vec<_> = MODELS.iter().filter(|m| m.is_english_only()).collect();
        let ids: Vec<_> = english.iter().map(|m| m.id).collect();
        assert_eq!(ids, ["tiny-en", "base-en", "small-en", "medium-en"]);
        for model in english {
            assert_eq!(
                model.languages(),
                LanguageSupport::Only(vec!["en".into()]),
                "{}",
                model.id
            );
            assert!(
                model.file_name.ends_with(".en.bin"),
                "{}: 영어 전용 원본 파일이 아닙니다",
                model.id
            );
        }
        assert_eq!(find_model("large-v3").unwrap().languages(), multilingual());
    }

    #[test]
    fn model_ids_have_no_dots() {
        for model in MODELS {
            assert!(
                !model.id.contains('.'),
                "{}: 모델 ID에 점이 있으면 번역 키 경로가 깨집니다",
                model.id
            );
        }
    }

    #[test]
    fn size_labels() {
        assert_eq!(format_size(3_095_033_483), "2.9 GB");
        assert_eq!(format_size(147_951_465), "141 MB");
        assert_eq!(format_size(2048), "2 KB");
        assert_eq!(format_size(512), "512 B");
    }
}
