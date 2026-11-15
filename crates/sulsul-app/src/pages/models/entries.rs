//! "사용 가능 엔진" 목록의 항목과, 항목을 켜거나 지울 때 설정을 바꾸는 규칙.
//!
//! 화면을 그리지 않습니다. 목록에 무엇이 들어가고 어느 것이 켜져 있는지는 설정과 모델 파일
//! 상태만으로 정해지므로, 여기서 따로 계산해 화면 코드와 테스트가 함께 씁니다.

use rust_i18n::t;
use sulsul_core::config::{ApiProfile, SttConfig};
use sulsul_core::stt::{EngineContext, EngineKind, ModelVariant, SttRegistry};
use sulsul_core::toml::{Table, Value};
use sulsul_stt_cloud::{
    ENGINE_ID as CLOUD_ENGINE_ID, find_preset, normalize_transcriptions_url, profile_from_preset,
};

use crate::i18n::{tr, tr_opt};

/// 모델 파일·폴더를 직접 지정하는 엔진 설정 키. 로컬 엔진들이 같은 이름을 씁니다.
pub const MODEL_PATH_KEY: &str = "model_path";

/// 클라우드 엔진 설정 테이블에서 프로필 값을 덮어쓰는 키 (CLI의 `--set`용)
const CLOUD_OVERRIDE_KEYS: [&str; 4] = ["provider", "endpoint", "api_key", "model"];

/// 목록의 항목 하나를 가리키는 키
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntryKey {
    /// 설치된 카탈로그 모델
    Variant {
        engine_id: String,
        variant_id: String,
    },
    /// 직접 지정한 모델 파일·폴더 (`model_path`)
    Custom { engine_id: String },
    /// 모델 파일이 필요 없는 로컬 엔진
    Engine { engine_id: String },
    /// 등록한 외부 제공자 (API 프로필)
    Provider { profile_id: String },
}

impl EntryKey {
    pub fn variant(engine_id: &str, variant_id: &str) -> Self {
        Self::Variant {
            engine_id: engine_id.to_owned(),
            variant_id: variant_id.to_owned(),
        }
    }

    pub fn provider(profile_id: &str) -> Self {
        Self::Provider {
            profile_id: profile_id.to_owned(),
        }
    }

    /// 이 항목을 실행하는 엔진의 ID
    pub fn engine_id(&self) -> &str {
        match self {
            Self::Variant { engine_id, .. }
            | Self::Custom { engine_id }
            | Self::Engine { engine_id } => engine_id,
            Self::Provider { .. } => CLOUD_ENGINE_ID,
        }
    }

    /// 이 컴퓨터에서 실행하는 항목인지 (외부 제공자가 아닌지)
    pub fn is_local(&self) -> bool {
        !matches!(self, Self::Provider { .. })
    }
}

/// 항목의 종류별 내용
#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind {
    Variant(ModelVariant),
    /// `path`는 설정에 적힌 그대로의 경로입니다.
    Custom {
        path: String,
    },
    Engine,
    Provider(ApiProfile),
}

/// 목록의 항목 하나
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub key: EntryKey,
    pub kind: EntryKind,
    /// 지금 설정이 이 항목을 가리키는지
    pub selected: bool,
    /// 지금 바로 받아쓰기에 쓸 수 있는지 (파일이 있고, 꼭 필요한 값이 채워져 있음)
    pub ready: bool,
}

impl Entry {
    /// 스위치로 켤 수 있는 항목인지. 직접 지정한 파일이 없으면 켤 수 없습니다.
    /// 값이 덜 채워진 제공자는 켠 뒤에 고칠 수 있으므로 켤 수 있습니다.
    pub fn selectable(&self) -> bool {
        self.ready || matches!(self.kind, EntryKind::Provider(_))
    }

    /// 스위치를 켜진 모습으로 보여 줄지
    pub fn shown_on(&self) -> bool {
        self.selected && self.selectable()
    }
}

/// 실제로 쓸 수 있는 항목 목록: 설치된 로컬 모델, 직접 지정한 모델, 모델이 필요 없는 로컬 엔진,
/// 등록한 외부 제공자 순서입니다. 같은 묶음 안에서는 엔진 등록 순서와 카탈로그 순서를 따릅니다.
pub fn available_entries(
    registry: &SttRegistry,
    ctx: &EngineContext,
    stt: &SttConfig,
) -> Vec<Entry> {
    let mut variants = Vec::new();
    let mut customs = Vec::new();
    let mut engines = Vec::new();

    for descriptor in registry
        .descriptors()
        .filter(|descriptor| descriptor.kind == EngineKind::Local)
    {
        let engine_id = descriptor.id.as_str();
        let active = stt.engine == engine_id;
        let catalog = registry.model_variants(ctx, engine_id).unwrap_or_default();
        let custom_path = custom_model_path(registry, stt, engine_id);
        let chosen = chosen_variant_id(registry, stt, engine_id);

        for variant in catalog.iter().filter(|variant| variant.is_installed()) {
            variants.push(Entry {
                key: EntryKey::variant(engine_id, &variant.id),
                selected: active
                    && custom_path.is_empty()
                    && chosen.as_deref() == Some(variant.id.as_str()),
                ready: true,
                kind: EntryKind::Variant(variant.clone()),
            });
        }

        let requirement = registry.model(ctx, stt, engine_id).ok().flatten();
        if !custom_path.is_empty() {
            customs.push(Entry {
                key: EntryKey::Custom {
                    engine_id: engine_id.to_owned(),
                },
                selected: active,
                ready: requirement.is_some_and(|model| model.is_installed()),
                kind: EntryKind::Custom { path: custom_path },
            });
        } else if catalog.is_empty() && requirement.is_none() {
            engines.push(Entry {
                key: EntryKey::Engine {
                    engine_id: engine_id.to_owned(),
                },
                selected: active,
                ready: true,
                kind: EntryKind::Engine,
            });
        }
    }

    let mut entries = variants;
    entries.append(&mut customs);
    entries.append(&mut engines);

    if registry.descriptor(CLOUD_ENGINE_ID).is_some() {
        let cloud_active = stt.engine == CLOUD_ENGINE_ID;
        let active_profile = stt.active_api_profile().map(|profile| profile.id.as_str());
        let mut seen: Vec<&str> = Vec::new();
        for profile in &stt.api_profiles {
            // ID가 겹치는 프로필은 앞의 것만 쓰이므로(설정 파일을 직접 고친 경우) 뒤의 것은 뺍니다.
            if seen.contains(&profile.id.as_str()) {
                continue;
            }
            seen.push(&profile.id);
            entries.push(Entry {
                key: EntryKey::provider(&profile.id),
                selected: cloud_active && active_profile == Some(profile.id.as_str()),
                ready: cloud_profile_ready(profile),
                kind: EntryKind::Provider(profile.clone()),
            });
        }
    }
    entries
}

/// 지금 설정이 가리키는 항목을 바로 쓸 수 있는지
pub fn selection_ready(entries: &[Entry]) -> bool {
    entries.iter().any(|entry| entry.selected && entry.ready)
}

/// 엔진 설정에 직접 지정한 모델 경로. 없으면 빈 문자열입니다.
pub fn custom_model_path(registry: &SttRegistry, stt: &SttConfig, engine_id: &str) -> String {
    registry
        .resolve_settings(stt, engine_id)
        .map(|resolved| resolved.str(MODEL_PATH_KEY).trim().to_owned())
        .unwrap_or_default()
}

/// 엔진 설정이 고른 카탈로그 모델 ID. 모델을 고르는 설정이 없는 엔진이면 `None`입니다.
pub fn chosen_variant_id(
    registry: &SttRegistry,
    stt: &SttConfig,
    engine_id: &str,
) -> Option<String> {
    let key = registry.model_setting_key(engine_id).ok().flatten()?;
    let resolved = registry.resolve_settings(stt, engine_id).ok()?;
    Some(resolved.str(key).to_owned())
}

/// 항목을 켭니다 (받아쓰기에 쓸 엔진·모델·제공자로 고릅니다).
///
/// 카탈로그 모델을 켜면 같은 엔진의 직접 지정 경로(`model_path`)는 비워집니다.
pub fn activate(registry: &SttRegistry, stt: &mut SttConfig, key: &EntryKey) {
    let engine_id = key.engine_id().to_owned();
    stt.engine.clone_from(&engine_id);
    registry.ensure_engine_defaults(stt, &engine_id);
    match key {
        EntryKey::Variant { variant_id, .. } => {
            let model_key = registry.model_setting_key(&engine_id).ok().flatten();
            let table = stt.engines.entry(engine_id).or_default();
            if let Some(model_key) = model_key {
                table.insert(model_key.to_owned(), Value::String(variant_id.clone()));
            }
            blank_model_path(table);
        }
        EntryKey::Custom { .. } | EntryKey::Engine { .. } => {}
        EntryKey::Provider { profile_id } => stt.api_profile.clone_from(profile_id),
    }
}

/// 지금 바로 쓸 수 있는 첫 항목을 켭니다. 켠 항목이 있으면 `true`를 돌려줍니다.
/// 쓸 수 있는 항목이 없으면 설정을 그대로 둡니다.
pub fn activate_first_ready(
    registry: &SttRegistry,
    ctx: &EngineContext,
    stt: &mut SttConfig,
) -> bool {
    let first = available_entries(registry, ctx, stt)
        .into_iter()
        .find(|entry| entry.ready);
    match first {
        Some(entry) => {
            activate(registry, stt, &entry.key);
            true
        }
        None => false,
    }
}

/// 직접 지정한 모델 경로를 설정에서 비웁니다. 사용자의 파일은 건드리지 않습니다.
pub fn clear_custom_path(stt: &mut SttConfig, engine_id: &str) {
    if let Some(table) = stt.engines.get_mut(engine_id) {
        blank_model_path(table);
    }
}

/// 엔진 설정 테이블의 `model_path`를 스키마 기본값인 빈 문자열로 바꿉니다.
///
/// 키를 지우면 안 됩니다. 설정을 저장할 때 파일에만 있는 키는 사용자가 직접 넣은 것으로 보고
/// 남겨 두므로(`sulsul_core::config`의 병합 규칙), 지운 키는 파일에 옛 값 그대로 남았다가 다시
/// 읽을 때 되살아납니다. 이 설정이 없는 엔진의 테이블에는 키를 새로 넣지 않습니다.
fn blank_model_path(table: &mut Table) {
    if let Some(path) = table.get_mut(MODEL_PATH_KEY) {
        *path = Value::String(String::new());
    }
}

/// 제공자를 설정에서 지웁니다. 고른 제공자(`stt.api_profile`)가 그것이었으면 남은 첫 제공자로
/// 바꾸고, 남은 것이 없으면 비웁니다. 클라우드 엔진 테이블의 CLI 재정의 키는 건드리지 않습니다.
pub fn remove_profile(stt: &mut SttConfig, profile_id: &str) {
    stt.api_profiles.retain(|profile| profile.id != profile_id);
    if stt.api_profile == profile_id {
        stt.api_profile = stt
            .api_profiles
            .first()
            .map(|profile| profile.id.clone())
            .unwrap_or_default();
    }
}

/// 지금 설정이 가리키는 것을 쓸 수 없는 이유
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionIssue {
    /// 고른 카탈로그 모델이 아직 설치되지 않았습니다.
    NotInstalled { engine: String, model: String },
    /// 직접 지정한 모델 파일·폴더를 찾을 수 없습니다.
    CustomMissing { engine: String },
    /// 외부 제공자를 쓰도록 되어 있는데 고른 제공자가 없습니다.
    NoProvider,
    /// 고른 제공자의 값이 덜 채워졌습니다.
    ProviderIncomplete { name: String },
    /// 이 앱에 등록되지 않은 엔진입니다.
    UnknownEngine { id: String },
}

impl SelectionIssue {
    /// 사용자에게 보일 안내 문장
    pub fn text(&self) -> String {
        match self {
            Self::NotInstalled { engine, model } => t!(
                "models.available.issue.not_installed",
                engine = engine,
                model = model
            ),
            Self::CustomMissing { engine } => {
                t!("models.available.issue.custom_missing", engine = engine)
            }
            Self::NoProvider => t!("models.available.issue.no_provider"),
            Self::ProviderIncomplete { name } => {
                t!("models.available.issue.provider_incomplete", name = name)
            }
            Self::UnknownEngine { id } => t!("models.available.issue.unknown_engine", id = id),
        }
        .into_owned()
    }
}

/// 지금 설정이 가리키는 것을 바로 쓸 수 없으면 그 이유를 돌려줍니다.
pub fn selection_issue(
    registry: &SttRegistry,
    ctx: &EngineContext,
    stt: &SttConfig,
    entries: &[Entry],
) -> Option<SelectionIssue> {
    if selection_ready(entries) {
        return None;
    }
    let engine_id = stt.engine.as_str();
    let Some(descriptor) = registry.descriptor(engine_id) else {
        return Some(SelectionIssue::UnknownEngine {
            id: engine_id.to_owned(),
        });
    };
    if engine_id == CLOUD_ENGINE_ID {
        return Some(match stt.active_api_profile() {
            Some(profile) => SelectionIssue::ProviderIncomplete {
                name: profile_display_name(profile),
            },
            None => SelectionIssue::NoProvider,
        });
    }
    let engine = tr(&descriptor.name_key);
    if !custom_model_path(registry, stt, engine_id).is_empty() {
        return Some(SelectionIssue::CustomMissing { engine });
    }
    let chosen = chosen_variant_id(registry, stt, engine_id)?;
    let model = registry
        .model_variants(ctx, engine_id)
        .ok()
        .and_then(|catalog| catalog.into_iter().find(|variant| variant.id == chosen))
        .map_or(chosen, |variant| variant_label(&variant));
    Some(SelectionIssue::NotInstalled { engine, model })
}

/// 제공자가 요청을 보낼 수 있을 만큼 채워졌는지 (주소·모델, 필요하면 API 키)
pub fn cloud_profile_ready(profile: &ApiProfile) -> bool {
    if normalize_transcriptions_url(&profile.endpoint).is_err() || profile.model.trim().is_empty() {
        return false;
    }
    let key_required = find_preset(&profile.provider).is_some_and(|preset| preset.requires_api_key);
    !key_required || !profile.api_key.trim().is_empty()
}

/// 카탈로그 모델의 이름. 번역이 없으면 ID를 보여 줍니다.
pub fn variant_label(variant: &ModelVariant) -> String {
    tr_opt(&variant.name_key).unwrap_or_else(|| variant.id.clone())
}

/// 제공자의 이름. 이름을 비워 두었으면 ID를 보여 줍니다.
pub fn profile_display_name(profile: &ApiProfile) -> String {
    if profile.name.trim().is_empty() {
        profile.id.clone()
    } else {
        profile.name.clone()
    }
}

/// 공급자 프리셋의 이름. 모르는 공급자면 ID를 그대로 보여 줍니다.
pub fn provider_label(provider: &str) -> String {
    find_preset(provider).map_or_else(|| provider.to_owned(), |preset| tr(preset.name_key))
}

/// 공급자 프리셋의 기본값으로 새 제공자를 만듭니다. ID와 이름은 이미 있는 것과 겹치지 않게 정합니다.
pub fn create_profile_from_preset(existing: &[ApiProfile], provider: &str) -> ApiProfile {
    let id = next_profile_id(existing, provider);
    let base_name = t!(
        "models.providers.default_name",
        provider = provider_label(provider)
    )
    .into_owned();
    let name = next_profile_name(existing, &base_name);
    profile_from_preset(provider, id, name)
}

/// `prefix`로 시작하고 `existing`과 겹치지 않는 ID (`openai`, `openai-2`, …)
pub fn next_profile_id(existing: &[ApiProfile], prefix: &str) -> String {
    let clean_prefix: String = prefix
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    let base = if clean_prefix.is_empty() {
        "profile"
    } else {
        clean_prefix.as_str()
    };
    for n in 1usize.. {
        let candidate = if n == 1 {
            base.to_owned()
        } else {
            format!("{base}-{n}")
        };
        if !existing.iter().any(|p| p.id == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// `base_name`이 이미 있으면 뒤에 번호를 붙입니다 (`내 OpenAI 2`, …).
pub fn next_profile_name(existing: &[ApiProfile], base_name: &str) -> String {
    if !existing.iter().any(|p| p.name == base_name) {
        return base_name.to_owned();
    }
    for n in 2usize.. {
        let candidate = format!("{base_name} {n}");
        if !existing.iter().any(|p| p.name == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// 공급자를 바꿨을 때, 사용자가 손대지 않은 주소·모델만 새 공급자의 기본값으로 바꿉니다.
pub fn apply_provider_preset_switch(
    profile: &mut ApiProfile,
    old_provider: &str,
    new_provider: &str,
) {
    let Some(new_preset) = find_preset(new_provider) else {
        return;
    };
    let old_default_endpoint = find_preset(old_provider).map(|p| p.default_endpoint);
    let old_default_model = find_preset(old_provider).map(|p| p.default_model);

    if profile.endpoint.trim().is_empty()
        || old_default_endpoint.is_some_and(|d| profile.endpoint.trim() == d)
    {
        profile.endpoint = new_preset.default_endpoint.to_owned();
    }
    if profile.model.trim().is_empty()
        || old_default_model.is_some_and(|d| profile.model.trim() == d)
    {
        profile.model = new_preset.default_model.to_owned();
    }
}

/// `profile` 하나만으로 클라우드 엔진을 테스트할 설정을 만듭니다.
///
/// 같은 ID의 프로필이 있으면 바꿔 넣고 없으면 더한 뒤 그 프로필을 고릅니다. 엔진 테이블의
/// CLI 재정의 키는 테스트 결과를 덮지 않도록 비웁니다.
pub fn profile_test_config(base: &SttConfig, profile: &ApiProfile) -> SttConfig {
    let mut stt = base.clone();
    stt.engine = CLOUD_ENGINE_ID.to_owned();
    stt.api_profile.clone_from(&profile.id);
    match stt.api_profiles.iter_mut().find(|p| p.id == profile.id) {
        Some(existing) => *existing = profile.clone(),
        None => stt.api_profiles.push(profile.clone()),
    }
    if let Some(table) = stt.engines.get_mut(CLOUD_ENGINE_ID) {
        for key in CLOUD_OVERRIDE_KEYS {
            table.remove(key);
        }
    }
    stt
}

/// 항목 하나를 테스트할 설정을 만듭니다. 화면의 설정은 바꾸지 않습니다.
/// 목록에 없는 제공자면 `None`입니다.
pub fn entry_test_config(
    registry: &SttRegistry,
    base: &SttConfig,
    key: &EntryKey,
) -> Option<SttConfig> {
    match key {
        EntryKey::Provider { profile_id } => {
            let profile = base.api_profiles.iter().find(|p| &p.id == profile_id)?;
            Some(profile_test_config(base, profile))
        }
        EntryKey::Variant { .. } | EntryKey::Custom { .. } | EntryKey::Engine { .. } => {
            let mut stt = base.clone();
            // 카탈로그 모델은 그 모델을 고른 설정으로, 나머지는 엔진만 바꾼 설정으로 테스트합니다.
            if matches!(key, EntryKey::Variant { .. }) {
                activate(registry, &mut stt, key);
            } else {
                stt.engine = key.engine_id().to_owned();
            }
            Some(stt)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use sulsul_core::stt::mock::{MOCK_ENGINE_ID, MockEngineFactory};
    use sulsul_core::{AppPaths, Config, ConfigStore};

    use super::*;

    const WHISPER: &str = sulsul_stt_whisper::ENGINE_ID;

    /// Whisper(카탈로그 있음), 가짜 엔진(모델 파일 없음), 클라우드 엔진을 넣은 레지스트리
    fn registry() -> SttRegistry {
        let mut registry = SttRegistry::new();
        registry.register(sulsul_stt_whisper::factory());
        registry.register(Arc::new(MockEngineFactory::new()));
        registry.register(sulsul_stt_cloud::factory());
        registry
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        registry: SttRegistry,
        ctx: EngineContext,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let ctx = EngineContext {
                paths: AppPaths::under(dir.path()),
            };
            Self {
                _dir: dir,
                registry: registry(),
                ctx,
            }
        }

        fn entries(&self, stt: &SttConfig) -> Vec<Entry> {
            available_entries(&self.registry, &self.ctx, stt)
        }

        /// 카탈로그 모델의 파일을 빈 내용으로 만들어 설치된 것처럼 꾸밉니다.
        fn install(&self, variant_id: &str) {
            let catalog = self.registry.model_variants(&self.ctx, WHISPER).unwrap();
            let variant = catalog.iter().find(|v| v.id == variant_id).unwrap();
            for file in &variant.files {
                fs::create_dir_all(file.path.parent().unwrap()).unwrap();
                fs::write(&file.path, b"model").unwrap();
            }
        }

        /// Whisper에 직접 지정한 모델 파일을 만들고, 그 경로가 적힌 설정 파일을 임시 폴더에 둡니다.
        /// 사용자가 설정 파일에 `model_path`를 직접 적어 둔 상태와 같습니다. 파일에서 읽은 설정을 돌려줍니다.
        fn config_with_custom_path(&self) -> (ConfigStore, Config, String) {
            let model = self.ctx.paths.models_dir().join("my-model.bin");
            fs::create_dir_all(model.parent().unwrap()).unwrap();
            fs::write(&model, b"model").unwrap();
            let path = model.to_string_lossy().into_owned();

            let mut config = Config::default();
            config
                .stt
                .engines
                .entry(WHISPER.into())
                .or_default()
                .insert(MODEL_PATH_KEY.into(), Value::String(path.clone()));
            let store = ConfigStore::new(self.ctx.paths.config_file());
            let config = save_and_reload(&store, &config);
            assert_eq!(
                custom_model_path(&self.registry, &config.stt, WHISPER),
                path,
                "설정 파일에 직접 지정 경로가 적혀 있어야 합니다"
            );
            (store, config, path)
        }
    }

    /// 설정을 파일에 저장한 뒤 다시 읽습니다. 앱이 설정을 저장하고 파일과 맞추는 과정과 같습니다.
    fn save_and_reload(store: &ConfigStore, config: &Config) -> Config {
        store.save(config).unwrap();
        store.load().unwrap().config
    }

    fn profile(id: &str) -> ApiProfile {
        let mut profile = profile_from_preset("openai", id.into(), format!("이름 {id}"));
        profile.api_key = "sk-test".into();
        profile
    }

    fn selected_keys(entries: &[Entry]) -> Vec<&EntryKey> {
        entries
            .iter()
            .filter(|entry| entry.shown_on())
            .map(|entry| &entry.key)
            .collect()
    }

    #[test]
    fn lists_installed_variants_modelless_engines_and_providers_in_order() {
        let fixture = Fixture::new();
        fixture.install("tiny");
        fixture.install("small");
        let mut stt = SttConfig::default();
        stt.api_profiles.push(profile("openai"));

        let keys: Vec<_> = fixture.entries(&stt).into_iter().map(|e| e.key).collect();
        assert_eq!(
            keys,
            [
                EntryKey::variant(WHISPER, "tiny"),
                EntryKey::variant(WHISPER, "small"),
                EntryKey::Engine {
                    engine_id: MOCK_ENGINE_ID.into()
                },
                EntryKey::provider("openai"),
            ],
            "설치된 모델, 모델이 필요 없는 엔진, 제공자 순서여야 합니다"
        );
    }

    #[test]
    fn exactly_one_entry_is_selected_after_each_activation() {
        let fixture = Fixture::new();
        fixture.install("tiny");
        fixture.install("small");
        let mut stt = SttConfig::default();
        stt.api_profiles.push(profile("openai"));
        stt.api_profiles.push(profile("groq"));

        let keys: Vec<_> = fixture.entries(&stt).into_iter().map(|e| e.key).collect();
        for key in &keys {
            activate(&fixture.registry, &mut stt, key);
            let entries = fixture.entries(&stt);
            assert_eq!(
                selected_keys(&entries),
                [key],
                "{key:?}를 켠 뒤에는 그 항목만 켜져 있어야 합니다"
            );
            assert!(selection_ready(&entries));
        }
    }

    #[test]
    fn default_config_points_to_a_model_that_is_not_installed() {
        let fixture = Fixture::new();
        fixture.install("tiny");
        let stt = SttConfig::default();
        let entries = fixture.entries(&stt);

        assert!(selected_keys(&entries).is_empty());
        let issue = selection_issue(&fixture.registry, &fixture.ctx, &stt, &entries);
        assert!(
            matches!(issue, Some(SelectionIssue::NotInstalled { .. })),
            "기본값 모델이 설치되지 않았다고 알려야 합니다: {issue:?}"
        );
    }

    #[test]
    fn custom_path_entry_cannot_be_selected_while_its_file_is_missing() {
        let fixture = Fixture::new();
        fixture.install("tiny");
        let model = fixture.ctx.paths.models_dir().join("my-model.bin");
        let mut stt = SttConfig::default();
        stt.engines.entry(WHISPER.into()).or_default().insert(
            MODEL_PATH_KEY.into(),
            Value::String(model.to_string_lossy().into_owned()),
        );

        let custom_key = EntryKey::Custom {
            engine_id: WHISPER.into(),
        };
        let entries = fixture.entries(&stt);
        let custom = entries.iter().find(|e| e.key == custom_key).unwrap();
        assert!(custom.selected && !custom.ready && !custom.selectable());
        assert!(selected_keys(&entries).is_empty());
        assert_eq!(
            selection_issue(&fixture.registry, &fixture.ctx, &stt, &entries),
            Some(SelectionIssue::CustomMissing {
                engine: tr("engine.whisper_local.name")
            })
        );

        // 파일이 생기면 켜진 항목이 됩니다.
        fs::write(&model, b"model").unwrap();
        let entries = fixture.entries(&stt);
        assert_eq!(selected_keys(&entries), [&custom_key]);

        // 같은 엔진의 카탈로그 모델을 켜면 직접 지정 경로가 비워져 그 항목이 사라집니다.
        activate(
            &fixture.registry,
            &mut stt,
            &EntryKey::variant(WHISPER, "tiny"),
        );
        let entries = fixture.entries(&stt);
        assert!(entries.iter().all(|e| e.key != custom_key));
        assert_eq!(
            selected_keys(&entries),
            [&EntryKey::variant(WHISPER, "tiny")]
        );
    }

    /// 저장할 때 파일에만 있는 키는 사용자가 넣은 것으로 보고 남기므로, `model_path`를 설정에서
    /// 지우기만 하면 파일의 옛 경로가 되살아납니다. 빈 값으로 덮어써야 저장한 뒤에도 비어 있습니다.
    #[test]
    fn cleared_custom_path_stays_cleared_after_saving_and_reloading() {
        let fixture = Fixture::new();
        let (store, mut config, path) = fixture.config_with_custom_path();

        clear_custom_path(&mut config.stt, WHISPER);
        let reloaded = save_and_reload(&store, &config);

        assert_eq!(
            reloaded.stt.engines[WHISPER].get(MODEL_PATH_KEY),
            Some(&Value::String(String::new())),
            "저장한 뒤 다시 읽어도 직접 지정 경로가 비어 있어야 합니다"
        );
        assert_eq!(
            custom_model_path(&fixture.registry, &reloaded.stt, WHISPER),
            ""
        );
        let custom_key = EntryKey::Custom {
            engine_id: WHISPER.into(),
        };
        assert!(
            fixture
                .entries(&reloaded.stt)
                .iter()
                .all(|e| e.key != custom_key),
            "직접 지정한 모델 항목이 목록에 다시 나타나면 안 됩니다"
        );
        assert!(
            fs::metadata(&path).is_ok(),
            "사용자의 모델 파일은 지우지 않습니다"
        );
    }

    #[test]
    fn activating_a_catalog_model_clears_the_custom_path_in_the_saved_file() {
        let fixture = Fixture::new();
        fixture.install("tiny");
        let (store, mut config, _path) = fixture.config_with_custom_path();

        let tiny = EntryKey::variant(WHISPER, "tiny");
        activate(&fixture.registry, &mut config.stt, &tiny);
        let reloaded = save_and_reload(&store, &config);

        assert_eq!(reloaded.stt.engine, WHISPER);
        assert_eq!(
            custom_model_path(&fixture.registry, &reloaded.stt, WHISPER),
            "",
            "저장한 뒤 다시 읽어도 직접 지정 경로가 비어 있어야 합니다"
        );
        assert_eq!(
            chosen_variant_id(&fixture.registry, &reloaded.stt, WHISPER).as_deref(),
            Some("tiny"),
            "켠 카탈로그 모델이 설정에 남아야 합니다"
        );
        assert_eq!(
            selected_keys(&fixture.entries(&reloaded.stt)),
            [&tiny],
            "다시 읽은 뒤에도 켠 모델만 켜져 있어야 합니다"
        );
    }

    #[test]
    fn removing_the_selected_provider_falls_back_to_the_first_ready_entry() {
        let fixture = Fixture::new();
        let mut stt = SttConfig::default();
        stt.api_profiles.push(profile("openai"));
        stt.api_profiles.push(profile("groq"));
        activate(&fixture.registry, &mut stt, &EntryKey::provider("groq"));

        remove_profile(&mut stt, "groq");
        assert_eq!(
            stt.api_profile, "openai",
            "남은 첫 제공자를 가리켜야 합니다"
        );
        assert!(activate_first_ready(
            &fixture.registry,
            &fixture.ctx,
            &mut stt
        ));
        // 목록의 첫 항목은 모델이 필요 없는 가짜 엔진입니다.
        assert_eq!(stt.engine, MOCK_ENGINE_ID);

        remove_profile(&mut stt, "openai");
        assert!(stt.api_profiles.is_empty());
        assert_eq!(stt.api_profile, "");
    }

    #[test]
    fn nothing_is_activated_when_no_entry_is_ready() {
        let mut registry = SttRegistry::new();
        registry.register(sulsul_stt_whisper::factory());
        registry.register(sulsul_stt_cloud::factory());
        let dir = tempfile::tempdir().unwrap();
        let ctx = EngineContext {
            paths: AppPaths::under(dir.path()),
        };
        let mut stt = SttConfig::default();
        let before = stt.clone();
        assert!(available_entries(&registry, &ctx, &stt).is_empty());
        assert!(!activate_first_ready(&registry, &ctx, &mut stt));
        assert_eq!(stt, before, "쓸 수 있는 항목이 없으면 설정을 그대로 둡니다");
    }

    #[test]
    fn incomplete_provider_is_listed_but_not_ready() {
        let fixture = Fixture::new();
        let mut stt = SttConfig::default();
        let mut incomplete = profile("openai");
        incomplete.api_key.clear();
        stt.api_profiles.push(incomplete);
        activate(&fixture.registry, &mut stt, &EntryKey::provider("openai"));

        let entries = fixture.entries(&stt);
        let entry = entries
            .iter()
            .find(|e| e.key == EntryKey::provider("openai"))
            .unwrap();
        assert!(entry.selected && entry.selectable() && !entry.ready);
        assert!(!selection_ready(&entries));
        assert!(matches!(
            selection_issue(&fixture.registry, &fixture.ctx, &stt, &entries),
            Some(SelectionIssue::ProviderIncomplete { .. })
        ));
    }

    #[test]
    fn test_config_for_a_variant_selects_it_without_touching_the_base() {
        let fixture = Fixture::new();
        let mut base = SttConfig::default();
        base.engines
            .entry(WHISPER.into())
            .or_default()
            .insert(MODEL_PATH_KEY.into(), Value::String("D:/custom.bin".into()));
        let before = base.clone();

        let key = EntryKey::variant(WHISPER, "tiny");
        let stt = entry_test_config(&fixture.registry, &base, &key).unwrap();
        assert_eq!(base, before);
        assert_eq!(
            chosen_variant_id(&fixture.registry, &stt, WHISPER).as_deref(),
            Some("tiny")
        );
        assert_eq!(custom_model_path(&fixture.registry, &stt, WHISPER), "");
    }

    #[test]
    fn profile_test_config_uses_the_profile_and_drops_cli_overrides() {
        let mut base = SttConfig::default();
        base.api_profiles.push(profile("openai"));
        let table = base.engines.entry(CLOUD_ENGINE_ID.into()).or_default();
        table.insert("model".into(), Value::String("override".into()));
        table.insert("timeout_seconds".into(), Value::Integer(60));

        let mut draft = profile("draft");
        draft.model = "whisper-large-v3".into();
        let stt = profile_test_config(&base, &draft);

        assert_eq!(stt.engine, CLOUD_ENGINE_ID);
        assert_eq!(stt.active_api_profile(), Some(&draft));
        let table = &stt.engines[CLOUD_ENGINE_ID];
        assert!(!table.contains_key("model"));
        assert_eq!(table.get("timeout_seconds"), Some(&Value::Integer(60)));
        // 원래 설정에는 초안이 들어가지 않습니다.
        assert_eq!(base.api_profiles.len(), 1);
    }

    #[test]
    fn next_profile_id_generates_unique_ids() {
        let existing = vec![
            ApiProfile {
                id: "openai".into(),
                ..ApiProfile::default()
            },
            ApiProfile {
                id: "openai-2".into(),
                ..ApiProfile::default()
            },
        ];
        assert_eq!(next_profile_id(&existing, "openai"), "openai-3");
        assert_eq!(next_profile_id(&existing, "groq"), "groq");
    }

    #[test]
    fn provider_switch_updates_untouched_defaults_only() {
        let mut p = profile_from_preset("openai", "o1".into(), "OpenAI".into());
        apply_provider_preset_switch(&mut p, "openai", "groq");
        assert_eq!(p.endpoint, "https://api.groq.com/openai/v1");
        assert_eq!(p.model, "whisper-large-v3-turbo");

        // 사용자가 엔드포인트를 직접 바꾼 뒤에는 프리셋 전환이 덮어쓰지 않습니다.
        p.endpoint = "https://my-proxy.example.com/v1".into();
        apply_provider_preset_switch(&mut p, "groq", "custom");
        assert_eq!(p.endpoint, "https://my-proxy.example.com/v1");
        assert_eq!(p.model, "whisper-1");
    }

    #[test]
    fn cloud_profile_readiness_checks_endpoint_model_and_required_key() {
        let mut openai = profile_from_preset("openai", "o1".into(), "OpenAI".into());
        assert!(!cloud_profile_ready(&openai));
        openai.api_key = "sk-test".into();
        assert!(cloud_profile_ready(&openai));

        let custom = profile_from_preset("custom", "c1".into(), "Custom".into());
        assert!(cloud_profile_ready(&custom));
    }
}
