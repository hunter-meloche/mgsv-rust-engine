cmake_minimum_required(VERSION 3.21)
project(SnakeSKSE LANGUAGES CXX)

# SKSE64 source tree (the tarball shipped in Skyrim's src/ folder, unpacked)
# and ianpatt/common installed to an `extern` prefix; see skyrim/README.md.
set(SKSE_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../../.tools/skse/skse64-2.3.1" CACHE PATH "SKSE64 source")
set(SKSE_EXTERN "${CMAKE_CURRENT_SOURCE_DIR}/../../.tools/skse/extern" CACHE PATH "ianpatt/common install prefix")

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")

add_library(SnakeSKSE SHARED src/main.cpp)
target_include_directories(SnakeSKSE PRIVATE
    "${SKSE_ROOT}"
    "${SKSE_EXTERN}/include"
    "${CMAKE_CURRENT_SOURCE_DIR}/../../crates/snake-ffi/include")
target_compile_definitions(SnakeSKSE PRIVATE NOMINMAX WIN32_LEAN_AND_MEAN)
target_compile_options(SnakeSKSE PRIVATE /W3 /utf-8 /FI"${SKSE_EXTERN}/include/common/IPrefix.h")
target_link_libraries(SnakeSKSE PRIVATE shell32 ole32 xaudio2)
