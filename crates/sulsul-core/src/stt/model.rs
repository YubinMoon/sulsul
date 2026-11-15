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

/// 내려받기 화면에서 모델을 단계별로 고르는 기준
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelAxis {
    /// 모델 크기 (예: tiny, large-v3)
    Size,
    /// 인식할 수 있는 언어 (예: 다국어)
    Language,
    /// 가중치 정밀도·양자화 (예: 원본, q5_0, int8)
    Precision,
}

impl ModelAxis {
    /// 번역 키 등에 쓰는 ID: "size" / "language" / "precision"
    pub fn id(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Language => "language",
            Self::Precision => "precision",
        }
    }
}

/// 변형 하나가 한 단계(축)에서 갖는 값
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantChoice {
    pub axis: ModelAxis,
    /// 같은 엔진 안에서 같은 값끼리 묶는 ID (예: "large-v3-turbo", "multi", "q5_0")
    pub value: String,
    /// 화면에 보일 값 이름 번역 키: `engine.<엔진 ID>.choices.<축 ID>.<값 ID>`
    pub label_key: String,
}

impl VariantChoice {
    /// `label_key`를 `engine.<엔진 ID>.choices.<축 ID>.<값 ID>` 규칙대로 채워 만듭니다.
    pub fn new(engine_id: &str, axis: ModelAxis, value: &str) -> Self {
        Self {
            axis,
            value: value.to_owned(),
            label_key: format!("engine.{engine_id}.choices.{}.{value}", axis.id()),
        }
    }
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
    /// 내려받기 화면의 단계별 선택 값. 같은 엔진의 모든 변형이 같은 축을 같은 순서로 가집니다.
    /// 순서가 곧 화면의 드롭다운 순서(왼쪽부터)입니다. 비어 있으면 단계 선택 없이 변형 하나로 봅니다.
    pub choices: Vec<VariantChoice>,
}

impl ModelVariant {
    /// 이 변형이 `axis` 단계에서 갖는 값
    pub fn choice(&self, axis: ModelAxis) -> Option<&VariantChoice> {
        self.choices.iter().find(|c| c.axis == axis)
    }

    /// `picked`의 단계 값과 모두 맞는지. `skip` 축의 항목은 보지 않습니다.
    fn matches_choices(&self, picked: &[(ModelAxis, &str)], skip: Option<ModelAxis>) -> bool {
        picked
            .iter()
            .filter(|(axis, _)| Some(*axis) != skip)
            .all(|(axis, value)| self.choice(*axis).is_some_and(|c| c.value == *value))
    }

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

/// 카탈로그의 단계 순서 (첫 변형의 `choices` 순서). 카탈로그가 비었으면 빈 목록.
pub fn catalog_axes(catalog: &[ModelVariant]) -> Vec<ModelAxis> {
    catalog
        .first()
        .map(|v| v.choices.iter().map(|c| c.axis).collect())
        .unwrap_or_default()
}

/// `picked`(이미 고른 단계 값)와 모두 맞는 변형만 남긴 뒤, `axis` 단계에서 고를 수 있는 값 목록.
/// `picked` 안에 `axis` 자신의 값이 있으면 그 항목은 무시합니다.
/// 결과는 카탈로그에서 처음 나온 순서이며 같은 `value`는 한 번만 나옵니다.
pub fn axis_options<'a>(
    catalog: &'a [ModelVariant],
    picked: &[(ModelAxis, &str)],
    axis: ModelAxis,
) -> Vec<&'a VariantChoice> {
    let mut seen = HashSet::new();
    catalog
        .iter()
        .filter(|v| v.matches_choices(picked, Some(axis)))
        .filter_map(|v| v.choice(axis))
        .filter(|c| seen.insert(c.value.as_str()))
        .collect()
}

/// `picked`의 단계 값과 모두 맞는 첫 변형. `picked`가 비었으면 첫 변형.
pub fn find_by_choices<'a>(
    catalog: &'a [ModelVariant],
    picked: &[(ModelAxis, &str)],
) -> Option<&'a ModelVariant> {
    catalog.iter().find(|v| v.matches_choices(picked, None))
}

/// `variant`의 파일(과 `.part` 조각)을 지우되, 같은 엔진의 다른 설치된 변형과 공유하는 파일(예: `tokens.txt`)은 남깁니다.
/// 지운 파일이 있던 폴더가 비면 `root` 바로 아래까지 거슬러 올라가며 빈 폴더를 지웁니다. `root` 자체와 `root` 밖은 건드리지 않습니다.
pub fn delete_variant(
    variant: &ModelVariant,
    catalog: &[ModelVariant],
    root: &Path,
) -> io::Result<()> {
    let shared: HashSet<&Path> = catalog
        .iter()
        .filter(|other| other.id != variant.id && other.is_installed())
        .flat_map(|other| other.files.iter().map(|f| f.path.as_path()))
        .collect();

    let mut dirs: Vec<&Path> = Vec::new();
    for file in &variant.files {
        if shared.contains(file.path.as_path()) {
            continue;
        }
        remove_if_exists(&file.path)?;
        remove_if_exists(&part_path(&file.path))?;
        if let Some(dir) = file.path.parent()
            && !dirs.contains(&dir)
        {
            dirs.push(dir);
        }
    }
    for dir in dirs {
        remove_empty_dirs(dir, root)?;
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

/// `dir`부터 위로 올라가며 빈 폴더를 지웁니다. 비어 있지 않은 폴더를 만나거나 `root`에 닿으면 멈추고,
/// `root` 밖의 폴더는 처음부터 건드리지 않습니다.
fn remove_empty_dirs(dir: &Path, root: &Path) -> io::Result<()> {
    let mut current = dir;
    while current != root && current.starts_with(root) {
        match fs::read_dir(current) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Ok(());
                }
                match fs::remove_dir(current) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    // 확인한 뒤에 다른 곳에서 파일이 생겼으면 그대로 둡니다.
                    Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            // 이미 없는 폴더면 그 위 폴더를 봅니다.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    Ok(())
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

    fn file(path: &Path, id: &str) -> ModelFile {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        ModelFile {
            path: path.to_path_buf(),
            asset: asset(id, &name, 4),
        }
    }

    fn variant(id: &str, files: Vec<ModelFile>, choices: &[(ModelAxis, &str)]) -> ModelVariant {
        ModelVariant {
            id: id.into(),
            name_key: format!("n.{id}"),
            description_key: format!("d.{id}"),
            recommended: false,
            files,
            choices: choices
                .iter()
                .map(|(axis, value)| VariantChoice::new("test", *axis, value))
                .collect(),
        }
    }

    /// 크기·언어·정밀도 세 단계를 가진 합성 카탈로그
    fn synthetic_catalog() -> Vec<ModelVariant> {
        use ModelAxis::{Language, Precision, Size};
        [
            ("small-multi-f16", "small", "multi", "f16"),
            ("small-en-f16", "small", "en", "f16"),
            ("large-multi-q5", "large", "multi", "q5_0"),
            ("large-multi-f16", "large", "multi", "f16"),
            ("large-en-q5", "large", "en", "q5_0"),
            ("small-multi-q5", "small", "multi", "q5_0"),
        ]
        .into_iter()
        .map(|(id, size, lang, precision)| {
            variant(
                id,
                Vec::new(),
                &[(Size, size), (Language, lang), (Precision, precision)],
            )
        })
        .collect()
    }

    fn values(options: &[&VariantChoice]) -> Vec<String> {
        options.iter().map(|c| c.value.clone()).collect()
    }

    #[test]
    fn axis_ids_are_stable() {
        assert_eq!(ModelAxis::Size.id(), "size");
        assert_eq!(ModelAxis::Language.id(), "language");
        assert_eq!(ModelAxis::Precision.id(), "precision");
    }

    #[test]
    fn variant_choice_new_builds_label_key() {
        let choice = VariantChoice::new("whisper_local", ModelAxis::Precision, "q5_0");
        assert_eq!(choice.axis, ModelAxis::Precision);
        assert_eq!(choice.value, "q5_0");
        assert_eq!(
            choice.label_key, "engine.whisper_local.choices.precision.q5_0",
            "번역 키가 engine.<엔진>.choices.<축>.<값> 규칙과 다릅니다"
        );
    }

    #[test]
    fn choice_finds_value_by_axis() {
        let catalog = synthetic_catalog();
        let v = &catalog[2];
        assert_eq!(v.choice(ModelAxis::Size).unwrap().value, "large");
        assert_eq!(v.choice(ModelAxis::Precision).unwrap().value, "q5_0");
        let bare = variant("bare", Vec::new(), &[]);
        assert_eq!(
            bare.choice(ModelAxis::Size),
            None,
            "축이 없으면 None이어야 합니다"
        );
    }

    #[test]
    fn catalog_axes_follow_first_variant() {
        use ModelAxis::{Language, Precision, Size};
        assert_eq!(
            catalog_axes(&synthetic_catalog()),
            [Size, Language, Precision]
        );
        assert!(
            catalog_axes(&[]).is_empty(),
            "빈 카탈로그는 단계가 없어야 합니다"
        );
        let no_choices = [variant("only", Vec::new(), &[])];
        assert!(
            catalog_axes(&no_choices).is_empty(),
            "choices가 비면 단계도 없어야 합니다"
        );
    }

    #[test]
    fn axis_options_dedupes_in_first_seen_order() {
        let catalog = synthetic_catalog();
        assert_eq!(
            values(&axis_options(&catalog, &[], ModelAxis::Size)),
            ["small", "large"],
            "크기 값은 처음 나온 순서로 한 번씩만 나와야 합니다"
        );
        assert_eq!(
            values(&axis_options(&catalog, &[], ModelAxis::Precision)),
            ["f16", "q5_0"]
        );
        // label_key도 함께 돌려줍니다.
        assert_eq!(
            axis_options(&catalog, &[], ModelAxis::Size)[1].label_key,
            "engine.test.choices.size.large"
        );
    }

    #[test]
    fn axis_options_filters_by_previous_picks() {
        use ModelAxis::{Language, Precision, Size};
        let catalog = synthetic_catalog();
        assert_eq!(
            values(&axis_options(&catalog, &[(Size, "small")], Language)),
            ["multi", "en"]
        );
        assert_eq!(
            values(&axis_options(
                &catalog,
                &[(Size, "small"), (Language, "en")],
                Precision
            )),
            ["f16"],
            "small·en 조합은 f16만 있어야 합니다"
        );
        assert_eq!(
            values(&axis_options(
                &catalog,
                &[(Size, "large"), (Language, "multi")],
                Precision
            )),
            ["q5_0", "f16"]
        );
        assert!(
            axis_options(&catalog, &[(Size, "huge")], Language).is_empty(),
            "맞는 변형이 없으면 선택지도 없어야 합니다"
        );
    }

    #[test]
    fn axis_options_ignores_own_axis_in_picked() {
        use ModelAxis::{Language, Precision, Size};
        let catalog = synthetic_catalog();
        // 이미 고른 자기 단계 값은 무시하므로, 다른 값으로 바꿀 수 있게 전체 선택지가 나옵니다.
        assert_eq!(
            values(&axis_options(
                &catalog,
                &[(Size, "small"), (Language, "en")],
                Size
            )),
            ["small", "large"],
            "자기 축의 picked 값은 무시해야 합니다"
        );
        assert_eq!(
            values(&axis_options(
                &catalog,
                &[(Size, "small"), (Precision, "f16")],
                Precision
            )),
            ["f16", "q5_0"]
        );
    }

    #[test]
    fn find_by_choices_matches_all_picks() {
        use ModelAxis::{Language, Precision, Size};
        let catalog = synthetic_catalog();
        let found = find_by_choices(
            &catalog,
            &[(Size, "large"), (Language, "multi"), (Precision, "f16")],
        )
        .unwrap();
        assert_eq!(found.id, "large-multi-f16");

        // 일부만 골랐으면 맞는 첫 변형
        let found = find_by_choices(&catalog, &[(Size, "large")]).unwrap();
        assert_eq!(found.id, "large-multi-q5");

        assert_eq!(
            find_by_choices(&catalog, &[(Size, "small"), (Precision, "int8")]),
            None,
            "맞는 변형이 없으면 None이어야 합니다"
        );
        assert_eq!(
            find_by_choices(&catalog, &[]).unwrap().id,
            "small-multi-f16",
            "picked가 비면 첫 변형이어야 합니다"
        );
        assert_eq!(find_by_choices(&[], &[]), None);
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

        let v_int8 = variant(
            "int8",
            vec![file(&int8, "int8"), file(&tokens, "tokens")],
            &[],
        );
        let v_fp32 = variant(
            "fp32",
            vec![file(&fp32, "fp32"), file(&tokens, "tokens")],
            &[],
        );
        let catalog = vec![v_int8.clone(), v_fp32.clone()];

        // fp32를 지워도 int8이 설치돼 있으므로 공유 파일 tokens.txt는 남습니다.
        delete_variant(&v_fp32, &catalog, dir.path()).unwrap();
        assert!(!fp32.exists());
        assert!(int8.exists());
        assert!(tokens.exists());

        // 마지막으로 int8까지 지우면 tokens.txt도 함께 삭제됩니다.
        delete_variant(&v_int8, &catalog, dir.path()).unwrap();
        assert!(!int8.exists());
        assert!(!tokens.exists());
        assert!(dir.path().is_dir(), "root 폴더는 비어도 남아야 합니다");
    }

    #[test]
    fn delete_variant_removes_empty_subdirs_but_keeps_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("models");
        let engine_dir = root.join("engine");
        let deep = engine_dir.join("v1");
        fs::create_dir_all(&deep).unwrap();
        let tokens = engine_dir.join("tokens.txt");
        let int8 = deep.join("model.int8.onnx");
        let fp32 = engine_dir.join("model.onnx");
        fs::write(&tokens, "tok").unwrap();
        fs::write(&int8, "int8").unwrap();
        fs::write(&fp32, "fp32").unwrap();
        // 내려받다 멈춘 조각도 함께 지워져야 합니다.
        fs::write(part_path(&int8), "partial").unwrap();

        let v_int8 = variant(
            "int8",
            vec![file(&int8, "int8"), file(&tokens, "tokens")],
            &[],
        );
        let v_fp32 = variant(
            "fp32",
            vec![file(&fp32, "fp32"), file(&tokens, "tokens")],
            &[],
        );
        let catalog = vec![v_int8.clone(), v_fp32.clone()];

        // int8만 지우면 v1 폴더는 비어서 사라지고, 공유 파일이 남은 engine 폴더는 남습니다.
        delete_variant(&v_int8, &catalog, &root).unwrap();
        assert!(!int8.exists());
        assert!(!part_path(&int8).exists(), ".part 조각이 남았습니다");
        assert!(!deep.exists(), "비게 된 하위 폴더가 남았습니다");
        assert!(tokens.exists(), "fp32가 쓰는 tokens.txt가 지워졌습니다");
        assert!(engine_dir.is_dir(), "파일이 남은 폴더가 지워졌습니다");

        // 마지막 변형까지 지우면 엔진 폴더도 사라지고 root는 남습니다.
        delete_variant(&v_fp32, &catalog, &root).unwrap();
        assert!(!engine_dir.exists(), "비게 된 엔진 폴더가 남았습니다");
        assert!(root.is_dir(), "root 폴더가 지워졌습니다");
        assert!(temp.path().is_dir());
    }

    #[test]
    fn delete_variant_keeps_non_empty_and_outside_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("models");
        let engine_dir = root.join("engine");
        let outside = temp.path().join("custom");
        fs::create_dir_all(&engine_dir).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let inside_file = engine_dir.join("model.bin");
        let unrelated = engine_dir.join("notes.txt");
        let outside_file = outside.join("model.bin");
        fs::write(&inside_file, "m").unwrap();
        fs::write(&unrelated, "keep").unwrap();
        fs::write(&outside_file, "m").unwrap();

        let v = variant(
            "v",
            vec![file(&inside_file, "inside"), file(&outside_file, "outside")],
            &[],
        );
        delete_variant(&v, std::slice::from_ref(&v), &root).unwrap();
        assert!(!inside_file.exists());
        assert!(!outside_file.exists());
        assert!(
            engine_dir.is_dir() && unrelated.exists(),
            "다른 파일이 있는 폴더는 남아야 합니다"
        );
        assert!(outside.is_dir(), "root 밖의 폴더는 건드리면 안 됩니다");
        assert!(root.is_dir());
    }
}
