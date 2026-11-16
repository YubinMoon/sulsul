# 술술 (sulsul)

단축키를 누르고 말하면 음성을 텍스트로 바꿔 커서 위치에 입력하거나 클립보드에 복사해 주는 데스크톱 앱입니다.

- 기본 단축키: **오른쪽 Ctrl** (토글: 한 번 눌러 녹음 시작, 다시 눌러 종료)
- 음성 인식: 로컬 Whisper(whisper.cpp) 우선, 엔진을 바꿔 끼울 수 있는 구조
- 플랫폼: Windows 우선, 구조는 크로스플랫폼

## 구조

```
crates/
  sulsul-core/          설정(TOML), 오디오, STT·후처리 추상화
  sulsul-stt-whisper/   로컬 Whisper 엔진
  sulsul-llm/           글 다듬기(LLM) 모델 카탈로그, 외부 제공자 프리셋·연결 테스트
  sulsul-app/           데스크톱 앱(eframe): 녹음·받아쓰기 테스트, 설정 화면
  sulsul-cli/           개발용 명령줄 도구 (장치 확인, 녹음 테스트)
locales/                화면 문구 번역 파일 (ko.yml이 기본)
```

### 설정

설정의 기준은 TOML 파일입니다. 설정 화면에서 바꾼 값도 이 파일에 저장되고(주석 유지),
앱 실행 중 파일을 직접 고치면 자동으로 다시 불러옵니다.

| OS | 설정 파일 | 데이터(모델·녹음) |
|---|---|---|
| Windows | `%APPDATA%\sulsul\config\config.toml` | `%LOCALAPPDATA%\sulsul\data` |

환경 변수 `SULSUL_HOME`을 지정하면 모든 파일을 그 폴더 아래에 둡니다(개발·테스트용).

### STT 엔진 추가

1. `SttEngineFactory`, `SttEngine` 구현 (`sulsul-core/src/stt/mod.rs` 참고)
2. 설명자에 설정 스키마(`SettingField`) 선언: 설정 화면이 자동으로 그려지고 값은 `[stt.engines.<ID>]`에 저장.
   인식 언어(`language_field`)와, 모델을 메모리에 올리는 로컬 엔진이면 모델 메모리 유지 시간(`keep_loaded_field`)도 넣습니다.
   인식 언어는 엔진마다 따로 저장됩니다(예전 `[stt] language`는 엔진 설정에 언어가 없을 때만 이어받음).
   고를 수 있는 언어는 한국어·영어·중국어·일본어 가운데 모델이 지원하는 것입니다(자동 감지는 모델이 지원하면 맨 앞).
3. 앱 시작 시 `SttRegistry`에 등록
4. `locales/*.yml`의 `engine.<ID>.*`에 문구 추가

## 개발

NPU 지원 범위와 현재 PC의 장치 진단 결과는 [AMD NPU 확인 기록](docs/npu-validation.md)을 참고하세요.

필요한 도구: Rust(stable, MSVC), CMake, LLVM/Clang(whisper.cpp 바인딩 생성용),
Vulkan SDK(기본 빌드에 들어가는 Whisper GPU 가속용)

### 빌드 종류

| 빌드 | 빌드할 때 필요한 것 | Whisper 인식 장치 |
|---|---|---|
| 기본 (Vulkan) | Vulkan SDK (`VULKAN_SDK` 환경 변수) | 쓸 수 있는 GPU가 있으면 GPU, 없으면 CPU |
| `--no-default-features` | 없음 | CPU만 |
| `--no-default-features --features cuda` | CUDA Toolkit (`CUDA_PATH`) | CUDA GPU가 있으면 GPU, 없으면 CPU |

- Vulkan SDK는 `winget install --id KhronosGroup.VulkanSDK -e`로 설치할 수 있습니다. 설치 프로그램이
  `VULKAN_SDK`를 등록하므로, 설치한 뒤 터미널(과 편집기)을 다시 여세요.
- `VULKAN_SDK`가 없으면 기본 빌드가 `Please install Vulkan SDK and ensure that VULKAN_SDK env variable is set`
  (whisper-rs-sys 빌드 스크립트)로 실패합니다. SDK 없이 빌드하려면 `--no-default-features`를 붙이세요.
- GPU가 없거나 드라이버가 없는 컴퓨터에서는 설정의 GPU 가속 항목이 잠기고 CPU로 인식합니다.
- 기본 빌드 실행 파일은 `vulkan-1.dll`(Vulkan 로더)을 실행할 때 불러옵니다. 그래픽 드라이버가 설치된
  컴퓨터에는 거의 항상 있지만, 드라이버가 전혀 없는 컴퓨터(가상 머신 등)에서는 프로그램이 뜨지 않으므로
  배포할 때 로더를 함께 넣거나 CPU 전용 빌드를 쓰세요.

```sh
cargo build --workspace
cargo test --workspace

# 앱 실행 (개발 빌드에는 화면 확인용 가짜 엔진 "mock"이 함께 들어갑니다)
cargo run -p sulsul-app

# 입력 장치 목록
cargo run -p sulsul-cli -- devices
# 5초 녹음해서 16kHz 모노 WAV로 저장 (--native-rate: 장치 샘플레이트 그대로)
cargo run -p sulsul-cli -- record -s 5 -o test.wav
```
