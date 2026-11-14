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
2. 설명자에 설정 스키마(`SettingField`) 선언: 설정 화면이 자동으로 그려지고 값은 `[stt.engines.<ID>]`에 저장
3. 앱 시작 시 `SttRegistry`에 등록
4. `locales/*.yml`의 `engine.<ID>.*`에 문구 추가

## 개발

필요한 도구: Rust(stable, MSVC), CMake, LLVM/Clang(whisper.cpp 바인딩 생성용)

```sh
cargo build --workspace
cargo test --workspace

# 입력 장치 목록
cargo run -p sulsul-cli -- devices
# 5초 녹음해서 16kHz 모노 WAV로 저장 (--native-rate: 장치 샘플레이트 그대로)
cargo run -p sulsul-cli -- record -s 5 -o test.wav
```
