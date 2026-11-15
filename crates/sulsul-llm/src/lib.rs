//! 글 다듬기(LLM 후처리)의 모델 카탈로그와 외부 제공자.
//!
//! - [`catalog`]: 이 PC에 내려받아 쓰는 로컬 LLM(GGUF) 목록
//! - [`presets`]: OpenAI 호환 `/chat/completions`를 제공하는 외부 제공자 프리셋
//! - [`client`]: 외부 제공자 연결 테스트
//! - [`entries`]: "사용 가능 모델" 목록과 고르기·지우기 규칙
//!
//! 화면 문구는 갖지 않습니다. 이름·설명은 번역 키(`*_key`)로 내보내고, 로그와 오류 메시지는 영어입니다.
//! 받아쓰기 결과를 실제로 다듬는 실행(로컬 추론, 후처리기 등록)은 아직 여기에 없습니다.

pub mod catalog;
pub mod client;
pub mod entries;
pub mod presets;

/// 이 PC에 내려받은 모델로 다듬는 후처리기 ID (`[postprocess] processor`)
pub const LOCAL_PROCESSOR_ID: &str = "llm_local";

/// 외부 제공자로 다듬는 후처리기 ID (`[postprocess] processor`)
pub const CLOUD_PROCESSOR_ID: &str = "llm_cloud";

/// `[postprocess.processors.llm_local]`에서 고른 로컬 모델(변형 ID)을 담는 키
pub const LOCAL_MODEL_KEY: &str = "model";
