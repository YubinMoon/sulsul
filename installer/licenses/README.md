# 네이티브 라이브러리 라이선스

설치 파일(`installer\build.ps1`)이 `licenses` 폴더에 함께 넣는 원문입니다. Rust 크레이트 고지는 빌드할 때
`cargo about`으로 만들고, Vulkan 런타임 라이선스는 내려받은 묶음에서 꺼내므로 여기에 없습니다.
의존성 판이 바뀌면 같은 곳에서 다시 받아 바꿉니다.

| 파일 | 출처 |
|---|---|
| `whisper.cpp-LICENSE.txt` | whisper-rs-sys 0.15.0 크레이트의 `whisper.cpp/LICENSE` |
| `llama.cpp-LICENSE.txt` | llama-cpp-sys-2 0.1.161 크레이트의 `llama.cpp/LICENSE` |
| `onnxruntime-LICENSE.txt`, `onnxruntime-ThirdPartyNotices.txt` | NuGet `Microsoft.ML.OnnxRuntime.Managed` 1.31.0 (ort 2.0.0-rc.15가 쓰는 ONNX Runtime 판) |
| `DirectML-LICENSE.txt`, `DirectML-ThirdPartyNotices.txt` | NuGet `Microsoft.AI.DirectML` 1.15.4 (설치 파일에 든 `DirectML.dll` 판) |
