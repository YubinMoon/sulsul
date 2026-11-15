use std::fmt;
use std::sync::Arc;

use super::{
    EngineContext, EngineDescriptor, ModelRequirement, ModelVariant, SttEngine, SttEngineFactory,
    SttError,
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

    /// 기본값을 채운 엔진 설정
    pub fn resolve_settings(
        &self,
        config: &SttConfig,
        id: &str,
    ) -> Result<ResolvedSettings, SttError> {
        let factory = self
            .get(id)
            .ok_or_else(|| SttError::UnknownEngine(id.to_owned()))?;
        Ok(factory.resolve_settings(config))
    }

    /// 지금 설정으로 이 엔진이 쓸 모델 파일 (모델 파일을 쓰지 않는 엔진이면 `None`)
    pub fn model(
        &self,
        ctx: &EngineContext,
        config: &SttConfig,
        id: &str,
    ) -> Result<Option<ModelRequirement>, SttError> {
        let factory = self
            .get(id)
            .ok_or_else(|| SttError::UnknownEngine(id.to_owned()))?;
        let resolved = factory.resolve_settings(config);
        Ok(factory.model(ctx, &resolved))
    }

    /// 엔진이 제공하는 모델 크기·변형 카탈로그
    pub fn model_variants(
        &self,
        ctx: &EngineContext,
        id: &str,
    ) -> Result<Vec<ModelVariant>, SttError> {
        let factory = self
            .get(id)
            .ok_or_else(|| SttError::UnknownEngine(id.to_owned()))?;
        Ok(factory.model_variants(ctx))
    }

    /// 카탈로그 모델 ID를 기록할 엔진 설정 키 (예: `"model"`)
    pub fn model_setting_key(&self, id: &str) -> Result<Option<&'static str>, SttError> {
        let factory = self
            .get(id)
            .ok_or_else(|| SttError::UnknownEngine(id.to_owned()))?;
        Ok(factory.model_setting_key())
    }

    /// 설정 파일에 빠진 엔진 설정 키를 기본값으로 채웁니다.
    /// 사용자가 TOML을 직접 열었을 때 바꿀 수 있는 항목을 볼 수 있게 하려는 목적입니다.
    /// 무언가 추가했으면 `true`를 돌려줍니다.
    pub fn ensure_engine_defaults(&self, config: &mut SttConfig, id: &str) -> bool {
        let Some(descriptor) = self.descriptor(id) else {
            return false;
        };
        let table = config.engines.entry(id.to_owned()).or_default();
        let mut changed = false;
        for field in &descriptor.settings {
            if !table.contains_key(&field.key) {
                table.insert(field.key.clone(), field.default.clone());
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
        let factory = self
            .get(id)
            .ok_or_else(|| SttError::UnknownEngine(id.to_owned()))?;
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
        let resolved = factory.resolve_settings(config);
        factory.create(ctx, &resolved)
    }
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
}
