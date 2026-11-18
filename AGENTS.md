# AGENTS.md

이 저장소에서 일하는 에이전트(Claude, Codex 등)를 위한 안내입니다.
사람이 읽는 소개, 빌드 종류 표, 설정 파일 위치는 [README.md](README.md)에 있습니다.

## 프로젝트

술술은 단축키(기본: **오른쪽 Ctrl**, 토글)를 누르고 말하면 음성을 텍스트로 바꿔 커서 위치에 입력하거나
클립보드에 복사해 주는 데스크톱 앱입니다. Rust 워크스페이스이고, Windows를 먼저 만들되 구조는 크로스플랫폼으로 둡니다.

마일스톤: M0 녹음 CLI → M1 STT → M2 단축키·클립보드 → M3 커서 입력 → M4 트레이 앱 →
M5 VAD·환각 필터·오버레이 → M6 배포. 지금 어디까지 왔는지는 `git log`로 확인하세요.

| 크레이트 | 역할 |
|---|---|
| `sulsul-core` | 설정(TOML), 오디오(녹음·재생·리샘플·WAV), STT·후처리 추상화, 모델 내려받기, 취소 신호 |
| `sulsul-stt-whisper` | 로컬 Whisper(whisper.cpp, `whisper-rs`) 엔진 |
| `sulsul-stt-whisper-onnx` | 로컬 Whisper ONNX(sherpa-onnx 내보내기, `ort`) 엔진, Windows는 인코더만 DirectML 가속 |
| `sulsul-stt-sensevoice` | 로컬 SenseVoice(ONNX, `ort`) 엔진, Windows는 DirectML 가속 |
| `sulsul-stt-cloud` | OpenAI 호환 `/audio/transcriptions` 클라우드 엔진 |
| `sulsul-llm` | 글 다듬기(LLM) 로컬 모델 카탈로그, 외부 제공자(OpenAI 호환 `/chat/completions`) 프리셋·요청, 모델 선택 규칙, 다듬기 실행(`Refiner`)과 로컬 실행기 프로토콜 |
| `sulsul-llm-runner` | 글 다듬기 로컬 모델 실행기(llama.cpp, `llama-cpp-2`). 앱이 띄우고 표준 입출력 JSON으로 주고받는 별도 실행 파일 |
| `sulsul-vad` | 말소리 찾기(Silero VAD, `ort`). 녹음 앞뒤 무음 자르기와 말소리 없는 녹음 가려내기. 모델은 실행 파일에 넣음 |
| `sulsul-app` | 데스크톱 앱(eframe/egui), 실행 파일 이름은 `sulsul` |
| `sulsul-cli` | 개발용 명령줄 도구 (`devices`, `record`, `transcribe`, `speech`, `models`) |
| `locales/` | 화면 문구 번역 파일 (`ko.yml`이 기본) |

## 빌드와 테스트

필요한 도구: Rust stable(MSVC), CMake, LLVM/Clang, Vulkan SDK(`VULKAN_SDK` 환경 변수).

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all

cargo run -p sulsul-app                  # 앱 (개발 빌드에는 가짜 엔진 "mock"이 들어감)
cargo build -p sulsul-llm-runner         # 글 다듬기 로컬 모델 실행기 (cargo run -p sulsul-app은 빌드하지 않음)
cargo run -p sulsul-cli -- devices       # 입력 장치 목록
cargo run -p sulsul-cli -- transcribe test.wav --engine whisper_local
```

- 빌드·clippy·테스트·실행 확인은 기본 빌드(Vulkan 포함)로만 합니다. CPU 전용 빌드(`--no-default-features`)로
  검증하지 마세요. GPU 사용 여부는 앱 안의 GPU 토글로 조절하고, CPU 동작은 토글을 끄거나
  `VK_LOADER_DRIVERS_DISABLE=*`로 GPU를 숨겨서 확인합니다.
- `VULKAN_SDK`가 없으면 기본 빌드가 whisper-rs-sys 빌드 스크립트에서 실패합니다. 셸에 값이 없으면
  설치 경로(보통 `C:\VulkanSDK\<버전>`)를 직접 지정하고, 그래도 안 되면 CPU 빌드로 우회하지 말고 보고하세요.
- `.cargo/config.toml`의 `GGML_NATIVE = "OFF"`는 다른 컴퓨터에서도 실행되게 하려는 설정이니 바꾸지 마세요.
  ggml 관련 환경 변수를 바꿨다면 `cargo clean -p whisper-rs-sys` 뒤에 다시 빌드해야 적용됩니다.
- 실제 Whisper 모델 테스트는 `SULSUL_TEST_WHISPER_MODEL`, `SULSUL_TEST_WHISPER_WAV`가 있을 때만 돌고,
  네트워크가 필요한 테스트는 `#[ignore]`입니다. 이 테스트들이 건너뛰어졌다면 결과에 그렇게 적으세요.
- 앱이나 CLI를 직접 실행해 볼 때는 `SULSUL_HOME`을 저장소의 `.sulsul-home/`(git 무시됨)으로 지정해
  사용자의 실제 설정·모델 폴더를 건드리지 않습니다.
  PowerShell: `$env:SULSUL_HOME = "$PWD\.sulsul-home"; cargo run -p sulsul-app`

작업을 끝냈다고 하기 전에 고친 크레이트의 `cargo fmt`, `cargo clippy`, `cargo test`를 돌리고,
실패가 있으면 숨기지 말고 출력과 함께 보고합니다.

## 확정된 결정

아래와 충돌하는 변경이 필요하면 구현하기 전에 사용자에게 먼저 묻습니다.

- 단축키는 오른쪽 Ctrl 토글이 기본이고, 누르는 동안만 녹음하는 방식도 설정으로 지원합니다.
- STT 엔진은 바꿔 끼울 수 있어야 합니다. 인식을 실행할 때는 엔진을 직접 다루지 말고 `sulsul_core::stt::Transcriber`를 씁니다.
- 설정의 기준은 TOML 파일입니다. 설정 화면은 파일과 동기화되고, 저장할 때 사용자의 주석·순서·직접 넣은 키를 지킵니다.
- 화면 언어는 한국어가 기본이고 영어는 나중에 추가합니다. 화면 문구는 `locales/*.yml`에만 둡니다.
- GUI는 다크 모드가 기본입니다.
- GPU 가속: 실행 파일(`sulsul-app`, `sulsul-cli`)의 기본 기능은 Vulkan, CUDA는 `--features cuda`,
  CPU 전용은 `--no-default-features`입니다. 엔진 라이브러리 크레이트의 `default`는 비워 둡니다
  (여기서 켜면 실행 파일에서 끌 수 없음). 실행 중에 쓸 수 있는 GPU가 없으면 반드시 CPU로 동작해야 합니다.
- SenseVoice는 Windows에서 기능 플래그 없이 항상 DirectML을 넣고, 장치가 없으면 CPU로 돌립니다.
- 글 다듬기(LLM 후처리)는 `[postprocess] enabled`를 켜면 단축키 받아쓰기 결과를 고른 프롬프트(system)와
  받아쓴 글(user)로 고른 모델에 보내, 다듬은 글을 받아쓴 글을 보낼 곳에 넣습니다. 다듬지 못하면 받아쓴 원문 대신
  `[술술 오류]`로 시작하는 실패 원인 안내를 같은 곳(클립보드 또는 커서 위치)에 넣습니다.
- 받아쓴 글을 보낼 곳(`[output] target`)은 클립보드에 복사(기본)·커서 위치에 붙여넣기·자동 중에서 고릅니다.
  커서 위치에는 클립보드를 잠깐 빌려 붙여넣기 키로 넣고 늘 원래 내용으로 되돌립니다(`restore_clipboard`는
  클립보드에 복사할 때만 씀). 커서 위치에 붙여넣기인데 입력할 곳이 없으면 기록에만 남깁니다.
- llama.cpp와 whisper.cpp는 각자 ggml을 정적 링크해 한 실행 파일에 넣으면 기호가 겹칩니다. 그래서 로컬 LLM은
  앱이 아닌 `sulsul-llm-runner` 실행 파일에서 돌리고, `sulsul-app`·`sulsul-llm`에는 `llama-cpp-2`를 넣지 않습니다.

## 코드 규칙

- 에디션 2024. 의존성은 루트 `Cargo.toml`의 `[workspace.dependencies]`에 버전을 적고 각 크레이트에서는
  `이름.workspace = true`로 씁니다. 새 크레이트에는 `[lints] workspace = true`를 넣습니다.
- `dbg!`, `todo!`를 남기지 않습니다 (워크스페이스 clippy 경고).
- 주석·문서 주석·테스트 실패 메시지는 한국어로 씁니다.
- 코어·엔진·앱의 로그(`tracing`)와 내부 오류 메시지는 영어입니다. 화면에는 `sulsul-app/src/error_text.rs`처럼
  오류 변형마다 번역 문구로 바꿔 보여 줍니다. 사람이 직접 읽는 CLI 출력은 한국어로 씁니다.
- 문구는 쉬운 우리말로 씁니다 (다운로드 → 내려받기, 트랜스크립션 → 받아쓰기·인식). 기존 문구의 말투를 따르세요.
- 화면 문구는 코드에 직접 쓰지 않고 `t!("키")`로 참조하며 `locales/ko.yml`에 추가합니다. 엔진 문구는
  `engine.<엔진 ID>.*`, 후처리기 문구는 `processor.<ID>.*` 아래에 둡니다. 빠진 키는 `sulsul-app`의 테스트가 잡습니다.
- 설정 항목을 추가할 때는 `sulsul-core/src/config/mod.rs`의 기본값과 `config/default.toml`(주석 포함)을
  함께 고칩니다. 둘이 다르면 `default_template_matches_default_config` 테스트가 실패합니다.
- 엔진별 설정은 `SettingField` 스키마로 선언하면 설정 화면이 자동으로 그려지고 값은 `[stt.engines.<ID>]`에 저장됩니다.
  새 엔진을 추가하는 절차는 `sulsul-core/src/stt/mod.rs` 머리 주석에 있습니다.
- 모델 파일은 크기·SHA-256·리비전을 고정한 URL로 기록하고, 내려받기는 `sulsul_core::download`를 씁니다.
  예외: 말소리 찾기 모델(Silero VAD, 2.3MB)은 기본으로 켜진 기능에 쓰여 `sulsul-vad/model/`에 넣어 둡니다.
- 가짜 엔진(`mock`)은 화면 흐름 확인용이라 개발 빌드(`debug_assertions`)에만 등록합니다.
- 테스트에서 실제 설정·데이터 폴더를 쓰지 않습니다. `AppPaths::under(임시 폴더)`나 `tempfile`을 씁니다.

## GUI 규칙 (`sulsul-app`)

- 색·글자 크기·모서리는 `theme.rs`의 `Palette`, `size`, `radius`만 씁니다. 화면 코드에서 `Color32`를 새로 만들지 않습니다.
  강조에는 포인트 색 하나만 쓰고, 경고·오류·성공 색은 상태 표시에만 씁니다.
- 여러 화면에서 쓰는 요소는 `widgets/`에, 화면은 `pages/`에 둡니다. 새 위젯을 만들기 전에 `widgets/`에 비슷한 것이 있는지 봅니다.
- 글꼴은 Pretendard(`fonts.rs`), 아이콘은 Lucide 아이콘 글꼴(`icons.rs`)을 씁니다.
- 창 최소 크기(780×540)에서도 화면이 잘리거나 겹치지 않아야 합니다.
- 모델 로딩·인식·내려받기처럼 오래 걸리는 일은 화면 스레드에서 돌리지 않고, 작업 스레드와 `CancelToken`으로 처리합니다
  (`pages/test/stt/job.rs` 참고).
- 가속 장치는 "예상 장치"(`accel.rs`)와 엔진이 보고한 실제 장치를 화면에서 섞지 않습니다.

## 커밋

- 사용자가 요청할 때만 커밋합니다. `master`에서 작업 중이면 먼저 브랜치를 만듭니다.
- 메시지는 한국어. 여러 기능을 한 번에 구현했어도 기능 단위로 나눠 커밋합니다.
- 제목은 짧은 명사구(예: `모델 내려받기 기능`, `CLI 받아쓰기 명령`), 본문은 `- `로 시작하는 변경점 목록입니다.

## 워커에게 맡길 때

Orca `/orchestration`으로 워커를 띄우는 감독자는 `.claude/skills/sulsul-orchestration` 스킬을 함께 씁니다
(작업 종류별 모델 배정, 나누는 법, 실행 테스트 방법). 워커로 이 저장소에 들어왔다면 이 파일의 규칙을 따르고,
spec에 적힌 범위 밖 파일은 고치지 않으며, 커밋하지 않고 결과를 보고합니다.
