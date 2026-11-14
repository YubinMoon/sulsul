//! 인식 결과 후처리 확장 지점 (예: LLM으로 문장 다듬기).
//!
//! 아직 구현체는 없습니다. STT 엔진과 같은 방식으로 설명자(번역 키, 설정 스키마)와
//! 팩토리를 제공하고, 설정 값은 TOML의 `[postprocess.processors.<ID>]`에 저장합니다.

use crate::paths::AppPaths;
use crate::settings::{ResolvedSettings, SettingField};
use crate::stt::Language;

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorDescriptor {
    pub id: String,
    pub name_key: String,
    pub description_key: String,
    pub settings: Vec<SettingField>,
}

/// 후처리에 함께 넘기는 정보
#[derive(Debug, Clone)]
pub struct PostProcessContext {
    /// 인식할 때 사용한 언어
    pub language: Language,
}

pub trait PostProcessor: Send {
    fn process(&mut self, text: &str, ctx: &PostProcessContext)
    -> Result<String, PostProcessError>;
}

pub trait PostProcessorFactory: Send + Sync {
    fn descriptor(&self) -> &ProcessorDescriptor;

    fn create(
        &self,
        paths: &AppPaths,
        settings: &ResolvedSettings,
    ) -> Result<Box<dyn PostProcessor>, PostProcessError>;
}

#[derive(Debug, thiserror::Error)]
pub enum PostProcessError {
    #[error("unknown post-processor '{0}'")]
    UnknownProcessor(String),
    #[error("{0}")]
    Backend(String),
}
