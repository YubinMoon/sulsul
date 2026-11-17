# whisper.cpp·llama.cpp(ggml)를 CMake로 빌드할 때 project()마다 불러오는 파일입니다 (`.cargo/config.toml`의
# CMAKE_PROJECT_INCLUDE). Vulkan 백엔드는 셰이더 생성기(vulkan-shaders-gen)를 ExternalProject로
# 따로 빌드하는데, 기본 위치(`ggml/src/ggml-vulkan/vulkan-shaders-gen-prefix/src/vulkan-shaders-gen-build`)가
# 너무 깊어서 Windows의 MSBuild가 경로 길이 제한(260자)에 걸려 FTK1011 오류로 실패합니다.
# ExternalProject의 기본 폴더를 빌드 폴더 바로 아래 `ep`로 옮겨 경로를 줄입니다.
set_property(DIRECTORY PROPERTY EP_BASE "${CMAKE_BINARY_DIR}/ep")

# ggml-vulkan은 CMake 정책 CMP0147을 켜서 Visual Studio가 사용자 지정 명령을 한꺼번에 돌리게 합니다.
# 셰이더 생성기의 ExternalProject 단계(구성 → 빌드 → 설치)까지 한꺼번에 돌면, 처음 빌드할 때 설치 단계가
# 빌드를 기다리지 않아 "file INSTALL cannot find ... vulkan-shaders-gen.exe"로 실패합니다.
# CMake 함수는 정의될 때의 정책으로 돌기 때문에, ExternalProject 모듈을 여기서 CMP0147이 꺼진 상태로
# 먼저 불러 두면 그 단계만 차례로 돌고 셰이더 컴파일은 그대로 한꺼번에 돕니다. 모듈은 한 번만 불러지므로
# ggml-vulkan의 include(ExternalProject)는 아무것도 바꾸지 않습니다. 3.19~3.26 정책은 ggml-vulkan이
# 켜는 CMP0114·CMP0116을 포함하고, 3.27에 생긴 CMP0147은 꺼진(설정하지 않은) 상태로 둡니다.
cmake_policy(PUSH)
cmake_policy(VERSION 3.19...3.26)
include(ExternalProject)
cmake_policy(POP)
