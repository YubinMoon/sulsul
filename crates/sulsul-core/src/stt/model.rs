//! 엔진이 쓰는 모델 파일 정보.
//!
//! 로컬 엔진은 [`SttEngineFactory::model`](super::SttEngineFactory::model)로 지금 설정에
//! 필요한 모델 파일을 알려 줍니다. 화면과 명령줄 도구는 이 정보만 보고 파일이 있는지
//! 확인하고 내려받으므로, 엔진마다 따로 코드를 둘 필요가 없습니다.

use std::path::PathBuf;

/// 내려받을 수 있는 모델 파일 하나
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelAsset {
    /// 엔진 설정에서 쓰는 모델 ID (예: `large-v3`)
    pub id: String,
    /// 모델 폴더에 저장할 파일 이름
    pub file_name: String,
    pub url: String,
    pub size_bytes: u64,
    /// 파일 전체의 SHA-256 (소문자 16진수 64자)
    pub sha256: String,
}

/// 지금 설정으로 엔진을 만들 때 필요한 모델 파일
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequirement {
    /// 엔진이 읽을 파일 위치
    pub path: PathBuf,
    /// 내려받을 수 있는 모델이면 그 정보. 사용자가 파일을 직접 지정했으면 `None`입니다.
    pub asset: Option<ModelAsset>,
}

impl ModelRequirement {
    pub fn is_installed(&self) -> bool {
        self.path.is_file()
    }
}
