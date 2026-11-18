# 개발 안내

술술을 소스에서 빌드하고 고치려는 사람을 위한 문서입니다. 앱 소개는 [README](../README.md), 쓰는 법은 [사용 안내](guide.md)에 있고,
에이전트(Claude, Codex 등)가 따를 규칙은 [AGENTS.md](../AGENTS.md)에 있습니다.

## 구조

```
crates/
  sulsul-core/              설정(TOML), 오디오, STT·후처리 추상화, 모델 내려받기
  sulsul-stt-whisper/       로컬 Whisper 엔진(whisper.cpp)
  sulsul-stt-whisper-onnx/  로컬 Whisper ONNX 엔진, Windows는 인코더만 DirectML 가속
  sulsul-stt-sensevoice/    로컬 SenseVoice(ONNX) 엔진, Windows는 DirectML 가속
  sulsul-stt-cloud/         OpenAI 호환 클라우드 엔진
  sulsul-llm/               글 다듬기(LLM) 모델 카탈로그, 외부 제공자 요청, 다듬기 실행(Refiner)
  sulsul-llm-runner/        글 다듬기 로컬 모델 실행기(llama.cpp). 앱이 띄우는 별도 실행 파일
  sulsul-vad/               말소리 찾기(Silero VAD): 녹음 앞뒤 무음 자르기, 말소리 없는 녹음 가려내기
  sulsul-app/               데스크톱 앱(eframe): 녹음·받아쓰기·글 다듬기 테스트 화면, 기록 화면, 설정 화면
  sulsul-cli/               개발용 명령줄 도구 (장치 확인, 녹음 테스트)
locales/                    화면 문구 번역 파일 (ko.yml이 기본)
```

## 빌드

NPU 지원 범위와 현재 PC의 장치 진단 결과는 [AMD NPU 확인 기록](npu-validation.md)을 참고하세요.

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
- 글 다듬기 로컬 모델 실행기(`sulsul-llm-runner`)도 같은 기능 플래그(`vulkan` 기본, `cuda`)를 따르고,
  쓸 수 있는 GPU가 없거나 모델을 GPU에 올리지 못하면 CPU로 돌립니다.
- 기본 빌드 실행 파일은 `vulkan-1.dll`(Vulkan 로더)을 실행할 때 불러옵니다. 그래픽 드라이버가 설치된
  컴퓨터에는 거의 항상 있지만, 드라이버가 전혀 없는 컴퓨터(가상 머신 등)에서는 프로그램이 뜨지 않으므로
  배포할 때 로더를 함께 넣거나 CPU 전용 빌드를 쓰세요.

### 명령

```sh
cargo build --workspace
cargo test --workspace

# 앱 실행 (개발 빌드에는 화면 확인용 가짜 엔진 "mock"이 함께 들어갑니다)
cargo run -p sulsul-app
# 로컬 모델로 글을 다듬으려면 실행기도 빌드해 앱과 같은 폴더(target/debug)에 둡니다.
# `cargo run -p sulsul-app`은 실행기를 빌드하지 않습니다.
cargo build -p sulsul-llm-runner

# 입력 장치 목록
cargo run -p sulsul-cli -- devices
# 5초 녹음해서 16kHz 모노 WAV로 저장 (--native-rate: 장치 샘플레이트 그대로)
# 앱처럼 음량을 맞추고 앞뒤 조용한 부분을 자름 (--no-auto-gain, --no-trim-silence: 하지 않음)
cargo run -p sulsul-cli -- record -s 5 -o test.wav
```

## 설치 파일 만들기

Windows x64 설치 파일은 [Inno Setup 6](https://jrsoftware.org/isinfo.php)으로 만듭니다.
위 빌드 도구와 함께 아래 도구가 있어야 합니다.

- Visual Studio C++ 도구 (VC++ 런타임 DLL을 재배포 폴더에서 가져옴)
- Inno Setup 6: `winget install --id JRSoftware.InnoSetup -e`
- cargo-about (Rust 크레이트 라이선스 고지): `cargo install cargo-about --locked --features cli`
- PowerShell 7 (`pwsh`)

```powershell
pwsh -File installer\build.ps1   # 이미 빌드했으면 -SkipBuild
```

`installer\build.ps1`이 릴리스 빌드(기본 기능 = Vulkan)를 하고, 넣을 파일을 `target\installer\stage`에 모은 뒤
`installer\sulsul.iss`를 컴파일해 `target\installer\sulsul-<버전>-windows-x64-setup.exe`를 만듭니다.
버전은 루트 `Cargo.toml`의 `[workspace.package] version`을 씁니다.

- 넣는 파일: `sulsul.exe`, `sulsul-llm-runner.exe`, `DirectML.dll`, VC++ 런타임 DLL(Visual Studio 재배포 폴더),
  Vulkan 로더 `vulkan-1.dll`, `licenses\` 폴더
- Vulkan 로더는 LunarG의 Vulkan 런타임 묶음(`VulkanRT-X64-<버전>-Components.zip`)을 내려받아 꺼냅니다.
  버전과 SHA-256은 `build.ps1`에 고정해 두었고, 내려받은 파일은 `target\installer\cache`에 둡니다.
  그래픽 드라이버가 없는 컴퓨터에서도 앱이 뜨고, 그때는 CPU로 인식합니다.
- `licenses\`에는 술술 라이선스, 글꼴·말소리 찾기 모델 라이선스, `cargo about`으로 만든 Rust 크레이트 고지
  (`THIRD-PARTY-NOTICES.txt`, 설정은 `installer\about.toml`), 네이티브 라이브러리 고지(`installer\licenses\`), Vulkan 런타임 라이선스가 들어갑니다.
- 관리자 권한 없이 사용자별로 `%LOCALAPPDATA%\Programs\sulsul`에 설치합니다.
- 설치·지우기 전에 설치 폴더에서 실행 중인 술술을 끝내고, 지울 때 자동 실행 등록을 지운 뒤 설정·기록·모델도 지울지 묻습니다.
- 실행 파일 아이콘(`crates/sulsul-app/assets/icon/sulsul.ico`)은 트레이 아이콘과 같은 그림이고, 빌드 스크립트(`build.rs`)가 `winresource`로 넣습니다.

## 설정과 데이터 폴더

설정 파일과 데이터 폴더 위치는 [사용 안내](guide.md#설정-파일과-기록)에 있습니다.

환경 변수 `SULSUL_HOME`을 지정하면 모든 파일을 그 폴더 아래에 둡니다(개발·테스트용).
앱이나 CLI를 직접 실행해 볼 때는 저장소의 `.sulsul-home/`(git 무시됨)으로 지정해 실제 설정·모델 폴더를 건드리지 않습니다.
설정 폴더가 다르면 이미 떠 있는 술술과 따로 실행됩니다.

```powershell
$env:SULSUL_HOME = "$PWD\.sulsul-home"; cargo run -p sulsul-app
```

앱 로그의 기록 수준은 `RUST_LOG`로 바꿉니다.

## STT 엔진 추가

1. `SttEngineFactory`, `SttEngine` 구현 (`sulsul-core/src/stt/mod.rs` 참고)
2. 설명자에 설정 스키마(`SettingField`) 선언: 설정 화면이 자동으로 그려지고 값은 `[stt.engines.<ID>]`에 저장.
   인식 언어(`language_field`)와, 모델을 메모리에 올리는 로컬 엔진이면 모델 메모리 유지 시간(`keep_loaded_field`)도 넣습니다.
   인식 언어는 엔진마다 따로 저장됩니다(예전 `[stt] language`는 엔진 설정에 언어가 없을 때만 이어받음).
   고를 수 있는 언어는 한국어·영어·중국어·일본어 가운데 모델이 지원하는 것입니다(자동 감지는 모델이 지원하면 맨 앞).
3. 앱 시작 시 `SttRegistry`에 등록
4. `locales/*.yml`의 `engine.<ID>.*`에 문구 추가
