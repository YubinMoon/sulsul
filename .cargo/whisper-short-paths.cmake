# whisper.cpp(ggml)를 CMake로 빌드할 때 project()마다 불러오는 파일입니다 (`.cargo/config.toml`의
# CMAKE_PROJECT_INCLUDE). Vulkan 백엔드는 셰이더 생성기(vulkan-shaders-gen)를 ExternalProject로
# 따로 빌드하는데, 기본 위치(`ggml/src/ggml-vulkan/vulkan-shaders-gen-prefix/src/vulkan-shaders-gen-build`)가
# 너무 깊어서 Windows의 MSBuild가 경로 길이 제한(260자)에 걸려 FTK1011 오류로 실패합니다.
# ExternalProject의 기본 폴더를 빌드 폴더 바로 아래 `ep`로 옮겨 경로를 줄입니다.
set_property(DIRECTORY PROPERTY EP_BASE "${CMAKE_BINARY_DIR}/ep")
