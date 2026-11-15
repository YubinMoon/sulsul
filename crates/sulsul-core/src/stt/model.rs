use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::download::part_path;

/// 내려받을 수 있는 모델 아티팩트 정보.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelAsset {
    pub id: String,
    pub file_name: String,
    pub url: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// 로컬 경로가 정해진 다운로드 가능한 개별 파일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    pub path: PathBuf,
    pub asset: ModelAsset,
}

/// 한 엔진이 제공하는 다운로드 가능 모델 크기·변형(variant) 정보.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelVariant {
    /// 엔진 설정(`model` 키 등)에 기록될 식별자 (예: `"tiny"`, `"large-v3"`, `"int8"`)
    pub id: String,
    /// 화면 표시 이름 번역 키 (번역이 없으면 `id`가 표시됩니다)
    pub name_key: String,
    /// 특징·권장 용도 번역 키
    pub description_key: String,
    /// 권장 모델 여부
    pub recommended: bool,
    /// 이 변형을 실행하는 데 필요한 모든 파일 (첫 번째가 주 모델 파일)
    pub files: Vec<ModelFile>,
}

impl ModelVariant {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.asset.size_bytes).sum()
    }

    pub fn is_installed(&self) -> bool {
        !self.files.is_empty() && self.files.iter().all(|f| f.path.is_file())
    }

    pub fn missing_files(&self) -> Vec<&ModelFile> {
        self.files.iter().filter(|f| !f.path.is_file()).collect()
    }

    pub fn primary_path(&self) -> Option<&Path> {
        self.files.first().map(|f| f.path.as_path())
    }
}

/// `variant`의 파일을 지우되, 같은 엔진의 다른 설치된 변형과 공유하는 파일(예: `tokens.txt`)은 남깁니다.
pub fn delete_variant(variant: &ModelVariant, catalog: &[ModelVariant]) -> io::Result<()> {
    let shared: HashSet<&Path> = catalog
        .iter()
        .filter(|other| other.id != variant.id && other.is_installed())
        .flat_map(|other| other.files.iter().map(|f| f.path.as_path()))
        .collect();

    for file in &variant.files {
        if shared.contains(file.path.as_path()) {
            continue;
        }
        remove_if_exists(&file.path)?;
        remove_if_exists(&part_path(&file.path))?;
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// 엔진이 동작하는 데 필요한 모델 파일 정보.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequirement {
    /// 주 모델 파일 위치
    pub path: PathBuf,
    /// 자동 다운로드 정보 (`None`이면 사용자가 직접 파일을 준비해야 합니다)
    pub asset: Option<ModelAsset>,
    /// 주 모델 외에 함께 필요한 추가 다운로드 파일 (예: `tokens.txt`)
    pub extra_files: Vec<ModelFile>,
}

impl ModelRequirement {
    pub fn single(path: PathBuf, asset: ModelAsset) -> Self {
        Self {
            path,
            asset: Some(asset),
            extra_files: Vec::new(),
        }
    }

    pub fn custom(path: PathBuf) -> Self {
        Self {
            path,
            asset: None,
            extra_files: Vec::new(),
        }
    }

    pub fn from_variant(variant: &ModelVariant) -> Option<Self> {
        let mut iter = variant.files.iter();
        let primary = iter.next()?;
        Some(Self {
            path: primary.path.clone(),
            asset: Some(primary.asset.clone()),
            extra_files: iter.cloned().collect(),
        })
    }

    /// 필요한 모든 모델 파일이 이미 준비되어 있는지 확인합니다.
    pub fn is_installed(&self) -> bool {
        self.path.is_file() && self.extra_files.iter().all(|f| f.path.is_file())
    }

    /// 자동 다운로드할 수 있는 모든 파일 목록 (주 파일 + 추가 파일)
    pub fn downloadable_files(&self) -> Vec<ModelFile> {
        let mut out = Vec::new();
        if let Some(asset) = &self.asset {
            out.push(ModelFile {
                path: self.path.clone(),
                asset: asset.clone(),
            });
        }
        out.extend(self.extra_files.iter().cloned());
        out
    }

    /// 총 내려받아야 하는 크기 (다운로드 가능한 파일 기준)
    pub fn total_bytes(&self) -> Option<u64> {
        let files = self.downloadable_files();
        (!files.is_empty()).then(|| files.iter().map(|f| f.asset.size_bytes).sum())
    }
}

/// 바이트 크기를 읽기 쉬운 문자열(`"77.7 MB"`, `"3.1 GB"`)로 바꿉니다.
pub fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    const MB: f64 = 1_000_000.0;
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / GB)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / MB)
    } else {
        format!("{:.0} KB", (bytes as f64 / 1_000.0).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: &str, name: &str, size: u64) -> ModelAsset {
        ModelAsset {
            id: id.into(),
            file_name: name.into(),
            url: format!("https://example.com/{name}"),
            size_bytes: size,
            sha256: "a".repeat(64),
        }
    }

    #[test]
    fn delete_variant_preserves_shared_files_until_last_variant_removed() {
        let dir = tempfile::tempdir().unwrap();
        let tokens = dir.path().join("tokens.txt");
        let int8 = dir.path().join("model.int8.onnx");
        let fp32 = dir.path().join("model.onnx");
        fs::write(&tokens, "tok").unwrap();
        fs::write(&int8, "int8").unwrap();
        fs::write(&fp32, "fp32").unwrap();

        let v_int8 = ModelVariant {
            id: "int8".into(),
            name_key: "k1".into(),
            description_key: "d1".into(),
            recommended: true,
            files: vec![
                ModelFile {
                    path: int8.clone(),
                    asset: asset("int8", "model.int8.onnx", 4),
                },
                ModelFile {
                    path: tokens.clone(),
                    asset: asset("tokens", "tokens.txt", 3),
                },
            ],
        };
        let v_fp32 = ModelVariant {
            id: "fp32".into(),
            name_key: "k2".into(),
            description_key: "d2".into(),
            recommended: false,
            files: vec![
                ModelFile {
                    path: fp32.clone(),
                    asset: asset("fp32", "model.onnx", 4),
                },
                ModelFile {
                    path: tokens.clone(),
                    asset: asset("tokens", "tokens.txt", 3),
                },
            ],
        };
        let catalog = vec![v_int8.clone(), v_fp32.clone()];

        // fp32를 지워도 int8이 설치돼 있으므로 공유 파일 tokens.txt는 남습니다.
        delete_variant(&v_fp32, &catalog).unwrap();
        assert!(!fp32.exists());
        assert!(int8.exists());
        assert!(tokens.exists());

        // 마지막으로 int8까지 지우면 tokens.txt도 함께 삭제됩니다.
        delete_variant(&v_int8, &catalog).unwrap();
        assert!(!int8.exists());
        assert!(!tokens.exists());
    }
}
