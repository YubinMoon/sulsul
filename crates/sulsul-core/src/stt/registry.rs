use std::fmt;
use std::sync::Arc;

use toml::Value;

use super::fields::{effective_language, legacy_language};
use super::{
    EngineContext, EngineDescriptor, LANGUAGE_KEY, Language, LanguageSupport, ModelRequirement,
    ModelVariant, SttEngine, SttEngineFactory, SttError,
};
use crate::config::SttConfig;
use crate::settings::{self, ResolvedSettings, SettingIssue};

/// 사용 가능한 STT 엔진 목록
#[derive(Clone, Default)]
pub struct SttRegistry {
    factories: Vec<Arc<dyn SttEngineFactory>>,
}

impl SttRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 엔진을 등록합니다. 같은 ID가 있으면 교체합니다.
    pub fn register(&mut self, factory: Arc<dyn SttEngineFactory>) {
        let id = factory.descriptor().id.clone();
        match self.factories.iter_mut().find(|f| f.descriptor().id == id) {
            Some(slot) => *slot = factory,
            None => self.factories.push(factory),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn SttEngineFactory>> {
        self.factories.iter().find(|f| f.descriptor().id == id)
    }

    pub fn descriptor(&self, id: &str) -> Option<&EngineDescriptor> {
        self.get(id).map(|f| f.descriptor())
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &EngineDescriptor> {
        self.factories.iter().map(|f| f.descriptor())
    }

    fn factory(&self, id: &str) -> Result<&Arc<dyn SttEngineFactory>, SttError> {
        self.get(id)
            .ok_or_else(|| SttError::UnknownEngine(id.to_owned()))
    }

    /// 설정 파일에 저장된 엔진 설정을 검사합니다.
    ///
    /// 더 이상 쓰지 않는 키([`EngineDescriptor::retired_keys`])는 문제로 보지 않습니다.
    pub fn validate(&self, config: &SttConfig, id: &str) -> Result<Vec<SettingIssue>, SttError> {
        let descriptor = self.factory(id)?.descriptor();
        Ok(config
            .engines
            .get(id)
            .map(|table| settings::validate(&descriptor.settings, table))
            .unwrap_or_default()
            .into_iter()
            .filter(|issue| !is_retired(descriptor, &issue.key))
            .collect())
    }

    /// 엔진 설정 표에 있지만 스키마에 없는 키 (오타일 가능성). 설정 화면의 경고에 씁니다.
    ///
    /// 더 이상 쓰지 않는 키([`EngineDescriptor::retired_keys`])는 빼고, 모르는 엔진이면 빈 목록입니다.
    pub fn unknown_keys(&self, config: &SttConfig, id: &str) -> Vec<String> {
        let (Some(descriptor), Some(table)) = (self.descriptor(id), config.engines.get(id)) else {
            return Vec::new();
        };
        table
            .keys()
            .filter(|key| {
                !descriptor.settings.iter().any(|field| &field.key == *key)
                    && !is_retired(descriptor, key)
            })
            .cloned()
            .collect()
    }

    /// 기본값을 채운 엔진 설정.
    ///
    /// 엔진 표에 `language`가 없으면 예전 전역 인식 언어(`[stt] language`)를 그 엔진에서 고를 수
    /// 있을 때 이어받습니다.
    pub fn resolve_settings(
        &self,
        config: &SttConfig,
        id: &str,
    ) -> Result<ResolvedSettings, SttError> {
        Ok(resolve(self.factory(id)?, config))
    }

    /// 지금 설정(고른 모델)으로 인식할 수 있는 언어
    pub fn languages(&self, config: &SttConfig, id: &str) -> Result<LanguageSupport, SttError> {
        let factory = self.factory(id)?;
        Ok(factory.languages(&resolve(factory, config)))
    }

    /// 인식 한 번에 실제로 쓸 언어. [`super::Transcriber::transcribe`]와 같은 규칙입니다.
    ///
    /// `requested`(그 작업만의 덮어쓰기)가 없으면 엔진 설정의 `language`를 씁니다. 그 언어를 지금
    /// 모델이 지원하지 않을 때 모델이 언어 하나만 지원하면(예: 영어 전용) 그 언어를, 아니면
    /// [`SttError::UnsupportedLanguage`]를 돌려줍니다.
    pub fn resolve_language(
        &self,
        config: &SttConfig,
        id: &str,
        requested: Option<&str>,
    ) -> Result<Language, SttError> {
        let factory = self.factory(id)?;
        effective_language(
            factory.as_ref(),
            &resolve(factory, config),
            config,
            requested,
        )
    }

    /// 카탈로그 모델 하나(`variant_id`)를 고른다고 했을 때 인식할 수 있는 언어.
    /// 내려받기 화면이나 아직 켜지 않은 모델 카드처럼 설정과 상관없이 모델을 보여 줄 때 씁니다.
    pub fn variant_languages(
        &self,
        id: &str,
        variant_id: &str,
    ) -> Result<LanguageSupport, SttError> {
        let factory = self.factory(id)?;
        // 스키마 기본값에서는 직접 지정 경로(`model_path`)가 비어 있습니다.
        let mut resolved = settings::resolve(&factory.descriptor().settings, None);
        if let Some(key) = factory.model_setting_key() {
            resolved.set(key, Value::String(variant_id.to_owned()));
        }
        Ok(factory.languages(&resolved))
    }

    /// 지금 설정으로 이 엔진이 쓸 모델 파일 (모델 파일을 쓰지 않는 엔진이면 `None`)
    pub fn model(
        &self,
        ctx: &EngineContext,
        config: &SttConfig,
        id: &str,
    ) -> Result<Option<ModelRequirement>, SttError> {
        let factory = self.factory(id)?;
        Ok(factory.model(ctx, &resolve(factory, config)))
    }

    /// 엔진이 제공하는 모델 크기·변형 카탈로그
    pub fn model_variants(
        &self,
        ctx: &EngineContext,
        id: &str,
    ) -> Result<Vec<ModelVariant>, SttError> {
        Ok(self.factory(id)?.model_variants(ctx))
    }

    /// 카탈로그 모델 ID를 기록할 엔진 설정 키 (예: `"model"`)
    pub fn model_setting_key(&self, id: &str) -> Result<Option<&'static str>, SttError> {
        Ok(self.factory(id)?.model_setting_key())
    }

    /// 설정 파일에 빠진 엔진 설정 키를 기본값으로 채웁니다.
    /// 사용자가 TOML을 직접 열었을 때 바꿀 수 있는 항목을 볼 수 있게 하려는 목적입니다.
    /// 언어가 빠졌으면 [`SttRegistry::resolve_settings`]처럼 예전 전역 인식 언어를 이어받습니다.
    /// 무언가 추가했으면 `true`를 돌려줍니다.
    pub fn ensure_engine_defaults(&self, config: &mut SttConfig, id: &str) -> bool {
        let Some(descriptor) = self.descriptor(id) else {
            return false;
        };
        let legacy = legacy_language(descriptor, config);
        let table = config.engines.entry(id.to_owned()).or_default();
        let mut changed = false;
        for field in &descriptor.settings {
            if !table.contains_key(&field.key) {
                let value = match &legacy {
                    Some(legacy) if field.key == LANGUAGE_KEY => legacy.clone(),
                    _ => field.default.clone(),
                };
                table.insert(field.key.clone(), value);
                changed = true;
            }
        }
        changed
    }

    /// 설정에서 선택된 엔진(`stt.engine`)을 만듭니다.
    pub fn create_active(
        &self,
        ctx: &EngineContext,
        config: &SttConfig,
    ) -> Result<Box<dyn SttEngine>, SttError> {
        self.create(ctx, config, &config.engine)
    }

    pub fn create(
        &self,
        ctx: &EngineContext,
        config: &SttConfig,
        id: &str,
    ) -> Result<Box<dyn SttEngine>, SttError> {
        let factory = self.factory(id)?;
        let descriptor = factory.descriptor();
        let issues: Vec<_> = self
            .validate(config, id)?
            .into_iter()
            .filter(|issue| issue.problem != settings::SettingProblem::UnknownKey)
            .collect();
        if !issues.is_empty() {
            return Err(SttError::InvalidSettings {
                engine: descriptor.id.clone(),
                issues,
            });
        }
        factory.create(ctx, &resolve(factory, config))
    }
}

/// 엔진이 해석한 설정에 예전 전역 인식 언어를 이어받습니다 ([`SttRegistry::resolve_settings`]).
fn resolve(factory: &Arc<dyn SttEngineFactory>, config: &SttConfig) -> ResolvedSettings {
    let mut resolved = factory.resolve_settings(config);
    if let Some(legacy) = legacy_language(factory.descriptor(), config) {
        resolved.set(LANGUAGE_KEY, legacy);
    }
    resolved
}

fn is_retired(descriptor: &EngineDescriptor, key: &str) -> bool {
    descriptor.retired_keys.iter().any(|retired| retired == key)
}

impl fmt::Debug for SttRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.descriptors().map(|d| &d.id))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioClip;
    use crate::cancel::CancelToken;
    use crate::paths::AppPaths;
    use crate::stt::mock::{MOCK_ENGINE_ID, MockEngineFactory};
    use crate::stt::{Language, TranscribeRequest};

    fn registry() -> SttRegistry {
        let mut registry = SttRegistry::new();
        registry.register(Arc::new(MockEngineFactory::new()));
        registry
    }

    fn ctx() -> EngineContext {
        EngineContext {
            paths: AppPaths::under("unused"),
        }
    }

    #[test]
    fn unknown_engine() {
        let config = SttConfig {
            engine: "nope".into(),
            ..SttConfig::default()
        };
        let err = registry().create_active(&ctx(), &config).err().unwrap();
        assert!(matches!(err, SttError::UnknownEngine(id) if id == "nope"));
    }

    #[test]
    fn ensure_defaults_then_create() {
        let registry = registry();
        let mut config = SttConfig {
            engine: MOCK_ENGINE_ID.into(),
            ..SttConfig::default()
        };
        assert!(registry.ensure_engine_defaults(&mut config, MOCK_ENGINE_ID));
        assert!(!registry.ensure_engine_defaults(&mut config, MOCK_ENGINE_ID));
        config
            .engines
            .get_mut(MOCK_ENGINE_ID)
            .unwrap()
            .insert("delay_ms".into(), toml::Value::Integer(0));

        let mut engine = registry.create_active(&ctx(), &config).unwrap();
        let clip = AudioClip::new(vec![0.0; 16_000], 16_000);
        let request = TranscribeRequest {
            language: Language::Code("ko".into()),
            prompt: None,
            cancel: CancelToken::new(),
        };
        let transcript = engine.transcribe(&clip, &request).unwrap();
        assert!(!transcript.text.is_empty());
    }

    #[test]
    fn engine_without_model_files_reports_none() {
        let registry = registry();
        let config = SttConfig::default();
        assert_eq!(
            registry.model(&ctx(), &config, MOCK_ENGINE_ID).unwrap(),
            None
        );
        assert!(matches!(
            registry.model(&ctx(), &config, "nope"),
            Err(SttError::UnknownEngine(_))
        ));
    }

    #[test]
    fn invalid_settings_are_rejected() {
        let registry = registry();
        let mut config = SttConfig {
            engine: MOCK_ENGINE_ID.into(),
            ..SttConfig::default()
        };
        let mut table = toml::Table::new();
        table.insert("delay_ms".into(), toml::Value::Integer(-1));
        config.engines.insert(MOCK_ENGINE_ID.into(), table);

        let err = registry.create_active(&ctx(), &config).err().unwrap();
        assert!(matches!(err, SttError::InvalidSettings { .. }));
    }

    /// 가짜 엔진 설명자에 `threads`를 은퇴한 키로 적은 엔진
    struct Retiring(EngineDescriptor);

    impl SttEngineFactory for Retiring {
        fn descriptor(&self) -> &EngineDescriptor {
            &self.0
        }

        fn create(
            &self,
            ctx: &EngineContext,
            settings: &ResolvedSettings,
        ) -> Result<Box<dyn SttEngine>, SttError> {
            MockEngineFactory::new().create(ctx, settings)
        }
    }

    fn retiring_registry() -> SttRegistry {
        let mut descriptor = MockEngineFactory::new().descriptor().clone();
        descriptor.retired_keys = vec!["threads".into()];
        let mut registry = SttRegistry::new();
        registry.register(Arc::new(Retiring(descriptor)));
        registry
    }

    #[test]
    fn retired_keys_are_neither_problems_nor_unknown() {
        let registry = retiring_registry();
        let mut config = SttConfig::default();
        let table = config.engines.entry(MOCK_ENGINE_ID.into()).or_default();
        table.insert("threads".into(), toml::Value::Integer(0));
        table.insert("typo".into(), toml::Value::Integer(1));

        let issues = registry.validate(&config, MOCK_ENGINE_ID).unwrap();
        let keys: Vec<_> = issues.iter().map(|issue| issue.key.as_str()).collect();
        assert_eq!(keys, ["typo"], "은퇴한 키가 문제로 보고됐습니다");
        assert_eq!(registry.unknown_keys(&config, MOCK_ENGINE_ID), ["typo"]);
        assert!(registry.unknown_keys(&config, "nope").is_empty());
        // 은퇴한 키가 남아 있어도 엔진을 만들 수 있습니다.
        assert!(registry.create(&ctx(), &config, MOCK_ENGINE_ID).is_ok());
    }

    #[test]
    fn legacy_global_language_is_inherited_only_when_engine_has_none() {
        let registry = registry();
        let language = |config: &SttConfig| {
            registry
                .resolve_settings(config, MOCK_ENGINE_ID)
                .unwrap()
                .str(LANGUAGE_KEY)
                .to_owned()
        };

        // 예전 값이 없으면 스키마 기본값(한국어)
        let mut config = SttConfig::default();
        assert_eq!(language(&config), "ko");

        // 엔진 표에 언어가 없으면 예전 전역 값을 이어받습니다 (대소문자·공백은 정리).
        config.language = " EN ".into();
        assert_eq!(language(&config), "en");

        // 그 엔진에서 고를 수 없는 값이면 기본값
        config.language = "xx".into();
        assert_eq!(language(&config), "ko");

        // 엔진 표의 언어가 있으면 그것이 우선입니다.
        config.language = "en".into();
        config
            .engines
            .entry(MOCK_ENGINE_ID.into())
            .or_default()
            .insert(LANGUAGE_KEY.into(), "ja".into());
        assert_eq!(language(&config), "ja");
    }

    #[test]
    fn ensure_defaults_fills_language_from_legacy_value() {
        let registry = registry();
        let mut config = SttConfig {
            language: "auto".into(),
            ..SttConfig::default()
        };
        assert!(registry.ensure_engine_defaults(&mut config, MOCK_ENGINE_ID));
        let table = &config.engines[MOCK_ENGINE_ID];
        assert_eq!(table[LANGUAGE_KEY].as_str(), Some("auto"));
        assert_eq!(table[crate::stt::KEEP_LOADED_KEY].as_integer(), Some(5));

        // 이미 있는 언어는 건드리지 않습니다.
        config.language = "en".into();
        assert!(!registry.ensure_engine_defaults(&mut config, MOCK_ENGINE_ID));
        assert_eq!(
            config.engines[MOCK_ENGINE_ID][LANGUAGE_KEY].as_str(),
            Some("auto")
        );
    }

    /// 고를 수 있는 언어(한국어·영어·중국어·일본어)가 아닌 값은 모델이 아는 언어라도 잘못된 값입니다.
    #[test]
    fn language_that_is_not_offered_reads_as_default_with_a_warning() {
        let registry = registry();
        let mut config = SttConfig::default();
        config
            .engines
            .entry(MOCK_ENGINE_ID.into())
            .or_default()
            .insert(LANGUAGE_KEY.into(), "fr".into());

        let resolved = registry.resolve_settings(&config, MOCK_ENGINE_ID).unwrap();
        assert_eq!(resolved.str(LANGUAGE_KEY), "ko");
        let issues = registry.validate(&config, MOCK_ENGINE_ID).unwrap();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].key, LANGUAGE_KEY);
        assert_eq!(
            registry
                .resolve_language(&config, MOCK_ENGINE_ID, None)
                .unwrap(),
            Language::Code("ko".into())
        );
        assert!(matches!(
            registry.resolve_language(&config, MOCK_ENGINE_ID, Some("fr")),
            Err(SttError::UnsupportedLanguage(code)) if code == "fr"
        ));
    }

    /// 예전 `[stt] language = "fr"`은 이어받지 않고 기본값(한국어)이 됩니다.
    #[test]
    fn legacy_language_that_is_not_offered_is_not_inherited() {
        let registry = registry();
        let mut config = SttConfig {
            language: "fr".into(),
            ..SttConfig::default()
        };
        assert_eq!(
            registry
                .resolve_settings(&config, MOCK_ENGINE_ID)
                .unwrap()
                .str(LANGUAGE_KEY),
            "ko"
        );
        assert!(registry.ensure_engine_defaults(&mut config, MOCK_ENGINE_ID));
        assert_eq!(
            config.engines[MOCK_ENGINE_ID][LANGUAGE_KEY].as_str(),
            Some("ko")
        );
        assert!(
            registry
                .validate(&config, MOCK_ENGINE_ID)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn language_support_follows_factory() {
        let registry = registry();
        let config = SttConfig::default();
        assert_eq!(
            registry.languages(&config, MOCK_ENGINE_ID).unwrap(),
            LanguageSupport::Any
        );
        assert_eq!(
            registry.variant_languages(MOCK_ENGINE_ID, "any").unwrap(),
            LanguageSupport::Any
        );
        assert!(matches!(
            registry.languages(&config, "nope"),
            Err(SttError::UnknownEngine(_))
        ));
        assert!(matches!(
            registry.variant_languages("nope", "any"),
            Err(SttError::UnknownEngine(_))
        ));
    }

    #[test]
    fn resolve_language_prefers_request_then_engine_setting() {
        let registry = registry();
        let mut config = SttConfig::default();
        config
            .engines
            .entry(MOCK_ENGINE_ID.into())
            .or_default()
            .insert(LANGUAGE_KEY.into(), "ja".into());
        assert_eq!(
            registry
                .resolve_language(&config, MOCK_ENGINE_ID, None)
                .unwrap(),
            Language::Code("ja".into())
        );
        assert_eq!(
            registry
                .resolve_language(&config, MOCK_ENGINE_ID, Some("auto"))
                .unwrap(),
            Language::Auto
        );
    }

    /// 예전 설정 파일(전역 `[stt] language`, 엔진 표의 `threads = 0`, 엔진 표에 언어 없음)을
    /// 그대로 읽어도 오류·경고 없이 예전 언어를 이어받고, 저장하면 엔진 표에 언어가 채워집니다.
    #[test]
    fn old_config_file_is_carried_over() {
        use crate::config::ConfigStore;

        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        let old = "\
[stt]
engine = \"mock\"
# 인식 언어: ko, en, auto(자동 감지)
language = \"en\"
initial_prompt = \"\"

[stt.engines.mock]
delay_ms = 0
text = \"\"
threads = 0
";
        std::fs::write(paths.config_file(), old).unwrap();

        let registry = retiring_registry();
        let store = ConfigStore::new(paths.config_file());
        let loaded = store.load().unwrap();
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        let mut config = loaded.config;
        assert!(
            registry
                .validate(&config.stt, MOCK_ENGINE_ID)
                .unwrap()
                .is_empty()
        );
        assert!(
            registry
                .unknown_keys(&config.stt, MOCK_ENGINE_ID)
                .is_empty()
        );
        let resolved = registry
            .resolve_settings(&config.stt, MOCK_ENGINE_ID)
            .unwrap();
        assert_eq!(resolved.str(LANGUAGE_KEY), "en");

        assert!(registry.ensure_engine_defaults(&mut config.stt, MOCK_ENGINE_ID));
        store.save(&config).unwrap();

        let text = std::fs::read_to_string(paths.config_file()).unwrap();
        // 사용자 파일의 예전 키는 지우지 않습니다.
        assert!(text.contains("threads = 0"), "{text}");
        assert!(text.contains("language = \"en\""), "{text}");
        let reloaded = store.load().unwrap();
        assert!(reloaded.issues.is_empty(), "{:?}", reloaded.issues);
        assert_eq!(
            reloaded.config.stt.engines[MOCK_ENGINE_ID][LANGUAGE_KEY].as_str(),
            Some("en")
        );
    }
}
