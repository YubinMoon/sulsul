# AMD NPU 확인 기록

2026-11-16 현재 PC에서 확인했습니다.

| 항목 | 결과 |
|---|---|
| CPU | AMD Ryzen 5 8640HS (Hawk Point) |
| Windows 빌드 | 26200.9974 |
| NPU | NPU Compute Accelerator Device, 상태 OK |
| PCI ID | VEN_1022 / DEV_1502 |
| 드라이버 | 32.0.20102.3930, oem143.inf |
| Ryzen AI SDK | 일반 설치 경로와 환경 변수에서 찾지 못함 |
| 실제 NPU 추론 | 미검증 |
| DirectML 장치 진단 | GPU: AMD Radeon 760M Graphics / NPU: 빈 목록 |

## 지원 판단

현재 Whisper ONNX 엔진은 DirectML GPU 인코더와 CPU 디코더를 사용합니다.
Windows의 NPU 존재 여부와 DirectML 사용 가능 장치는 서로 다릅니다.
세션 생성 성공도 NPU에 실제 연산이 배치되었다는 증거가 아닙니다.

AMD 문서에서 PHX/HPT는 Vitis AI의 X1 INT8 경로를 사용하며, BF16은 STX/KRK 이후를 대상으로 합니다.
해당 세대의 INT8 CNN 경로를 현재 sherpa-onnx Whisper 모델에 그대로 적용할 수 있다고 판단할 근거는 없습니다.
AMD의 Whisper.cpp 가속 안내 역시 Ryzen AI 300 계열을 대상으로 합니다.
따라서 이 PC에서 Whisper NPU 지원을 켜지 않습니다. 모든 NPU 작업의 불가능을 뜻하지 않습니다.

SDK 설치와 CNN 예제 실행은 수행하지 않았습니다. Whisper 지원 경로가 확인되지 않아 시스템 설치를 진행하지 않았으며,
NPU 자체의 연산 성공 여부와 CPU/GPU/NPU 실제 음성 비교도 미검증으로 남깁니다.
추후 지원 시에는 호환 모델, 런타임 버전과 NPU 배치 로그를 확보한 뒤 한국어 WAV로 결과와 시간을 비교해야 합니다.

## 다시 확인하기

```powershell
$env:SULSUL_HOME = "$PWD\.sulsul-home"
cargo run -p sulsul-stt-whisper-onnx --example acceleration_probe
```

이 명령은 DirectML 장치 목록과 현재 엔진의 지원 범위만 표시합니다. 실제 추론 검증은 아닙니다.
기존 `auto`의 GPU → CPU 순서와 CPU 선택은 유지합니다.

현재 PC에서 실행한 결과:

```text
DirectML 포함 여부: true
DirectML GPU 목록: ["AMD Radeon 760M Graphics"]
DirectML NPU 목록: []
```

즉, 현재 앱의 DirectML 경로에서는 Windows가 감지한 AMD NPU를 사용할 수 없습니다.

## 코드 검증

- `cargo fmt --all -- --check`: 통과
- `cargo test -p sulsul-stt-whisper-onnx -p sulsul-app`: 통과
- `cargo build -p sulsul-app`: 기본 Vulkan 구성 통과
- `cargo clippy -p sulsul-stt-whisper-onnx -p sulsul-app --all-targets`: 통과
  (첫 시도는 CMake 실행 권한 오류 `os error 5`로 실패, 승인된 권한으로 재실행)
- 실제 Whisper 모델 테스트: 모델·WAV 환경 변수가 없어 본문 실행을 건너뜀
- 네트워크 모델 내려받기 및 NPU 실제 추론: 수행하지 않음

## 근거

- [AMD 모델 실행과 세대별 설정](https://ryzenai.docs.amd.com/en/latest/modelrun.html)
- [AMD Whisper.cpp 지원 범위](https://ryzenai.docs.amd.com/en/latest/whisper_cpp.html)
- [ONNX Runtime Vitis AI 공급자](https://onnxruntime.ai/docs/execution-providers/Vitis-AI-ExecutionProvider.html)
