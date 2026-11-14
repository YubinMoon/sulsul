//! 술술 핵심 로직.
//!
//! - [`config`]: TOML 설정 파일(설정의 기준)과 저장소
//! - [`audio`]: 오디오 데이터 타입과 마이크 녹음
//! - [`stt`]: 음성 인식 엔진 추상화와 레지스트리
//! - [`postprocess`]: 인식 결과 후처리(예: LLM 다듬기) 확장 지점
//! - [`settings`]: 엔진·후처리기가 공통으로 쓰는 설정 스키마
//! - [`cancel`]: 오래 걸리는 작업을 멈추는 신호
//! - [`download`]: 큰 파일(모델) 내려받기: 이어받기, 크기·해시 검사
//!
//! 이 크레이트는 UI 문자열을 직접 갖지 않습니다. 화면에 보일 이름·설명은
//! 번역 키(`*_key`)로 노출하고, 실제 문구는 저장소 루트의 `locales/`에서 관리합니다.

pub mod audio;
pub mod cancel;
pub mod config;
pub mod download;
pub mod paths;
pub mod postprocess;
pub mod settings;
pub mod stt;

/// 엔진 설정 값 타입을 맞추기 위해 다시 내보냅니다.
pub use toml;

pub use cancel::CancelToken;
pub use config::{Config, ConfigStore};
pub use paths::AppPaths;
