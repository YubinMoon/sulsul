use std::fmt;
use std::sync::Arc;

use super::{EngineContext, EngineDescriptor, SttEngine, SttEngineFactory, SttError};
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

    /// 설정 파일에 저장된 엔진 설정을 검사합니다.
    pub fn validate(&self, config: &SttConfig, id: &str) -> Result<Vec<SettingIssue>, SttError> {
        let descriptor = self
            .descriptor(id)
            .ok_or_else(|| SttError::UnknownEngine(id.to_owned()))?;
        Ok(config
            .engines
            .get(id)
            .map(|table| settings::validate(&descriptor.settings, table))
            .unwrap_or_default())
    }

    /// 기본값을 채운 엔진 설정
    pub fn resolve_settings(
        &self,
        config: &SttConfig,
        id: &str,
    ) -> Result<ResolvedSettings, SttError> {
        let descriptor = self
            .descriptor(id)
            .ok_or_else(|| SttError::UnknownEngine(id.to_owned()))?;
        Ok(settings::resolve(
            &descriptor.settings,
            config.engines.get(id),
        ))
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
        let resolved = settings::resolve(&descriptor.settings, config.engines.get(id));
        factory.create(ctx, &resolved)
    }
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
        };
        let transcript = engine.transcribe(&clip, &request).unwrap();
        assert!(!transcript.text.is_empty());
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
}
