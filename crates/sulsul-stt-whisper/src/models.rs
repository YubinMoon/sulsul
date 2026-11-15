//! 내려받을 수 있는 whisper.cpp GGML 모델 목록.
//!
//! 모델을 추가하려면 [`MODELS`]에 한 줄을 더합니다. 크기와 SHA-256은 Hugging Face API
//! (`https://huggingface.co/api/models/ggerganov/whisper.cpp/tree/main`)의 `size`와
//! `lfs.oid` 값을 그대로 옮겨 적습니다.
//!
//! 목록 순서가 내려받기 화면의 단계별 선택지 순서가 되므로 작은 모델부터 적고, 같은 크기 안에서는
//! 작은 양자화 파일을 먼저 둡니다. 새 단계 값을 넣었다면 `locales/*.yml`의
//! `engine.whisper_local.choices.<축 ID>.<값 ID>` 문구도 추가합니다.

use sulsul_core::stt::{LanguageSupport, ModelAsset, ModelAxis, VariantChoice, WHISPER_LANGUAGES};

use crate::ENGINE_ID;

pub const DEFAULT_MODEL: &str = "large-v3";

/// 모델 파일을 받는 Hugging Face 저장소
const REPO: &str = "ggerganov/whisper.cpp";

/// 내려받는 저장소 리비전(커밋). `main`은 내용이 바뀔 수 있어, 크기·해시를 확인한
/// 시점의 커밋에 고정합니다.
const REPO_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

/// 내려받을 수 있는 whisper.cpp GGML 모델
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperModel {
    pub id: &'static str,
    /// 내려받기 화면의 크기 단계 값 (예: `large-v3-turbo`)
    pub size: &'static str,
    /// 내려받기 화면의 언어 단계 값. 공식 다국어 모델은 `multi`입니다.
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
        language: "multi",
        precision: "f16",
        file_name: "ggml-tiny.bin",
        size_bytes: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        recommended: false,
    },
    WhisperModel {
        id: "base",
        size: "base",
        language: "multi",
        precision: "f16",
        file_name: "ggml-base.bin",
        size_bytes: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        recommended: false,
    },
    WhisperModel {
        id: "small",
        size: "small",
        language: "multi",
        precision: "f16",
        file_name: "ggml-small.bin",
        size_bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        recommended: false,
    },
    WhisperModel {
        id: "medium",
        size: "medium",
        language: "multi",
        precision: "f16",
        file_name: "ggml-medium.bin",
        size_bytes: 1_533_763_059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        recommended: false,
    },
    WhisperModel {
        id: "large-v3-turbo-q5_0",
        size: "large-v3-turbo",
        language: "multi",
        precision: "q5_0",
        file_name: "ggml-large-v3-turbo-q5_0.bin",
        size_bytes: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        recommended: false,
    },
    WhisperModel {
        id: "large-v3-turbo",
        size: "large-v3-turbo",
        language: "multi",
        precision: "f16",
        file_name: "ggml-large-v3-turbo.bin",
        size_bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        recommended: true,
    },
    WhisperModel {
        id: "large-v3",
        size: "large-v3",
        language: "multi",
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
    fn size_labels() {
        assert_eq!(format_size(3_095_033_483), "2.9 GB");
        assert_eq!(format_size(147_951_465), "141 MB");
        assert_eq!(format_size(2048), "2 KB");
        assert_eq!(format_size(512), "512 B");
    }
}
