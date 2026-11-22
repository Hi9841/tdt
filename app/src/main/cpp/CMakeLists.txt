cmake_minimum_required(VERSION 3.24)
project(tdt_engine C CXX)

# The engine is unusable unoptimized, so debug APKs get a release engine too.
set(CMAKE_BUILD_TYPE Release)

# The speech library (Rust): the take path of TDT desktop on Trit. Trit itself is in third_party/trit, and
# engine/Cargo.toml names that path.
cmake_path(SET ENGINE_DIR NORMALIZE "${CMAKE_CURRENT_SOURCE_DIR}/../../../../engine")
if(NOT EXISTS ${ENGINE_DIR}/../third_party/trit/Cargo.toml)
  message(FATAL_ERROR "Trit not found. Put an export of https://github.com/Hi9841/trit in third_party/trit.")
endif()

if(ANDROID_ABI STREQUAL "arm64-v8a")
  set(RUST_TARGET aarch64-linux-android)
elseif(ANDROID_ABI STREQUAL "x86_64")
  set(RUST_TARGET x86_64-linux-android)
else()
  message(FATAL_ERROR "Trit has no Rust target for ${ANDROID_ABI}.")
endif()
string(REPLACE "-" "_" RUST_TARGET_VAR ${RUST_TARGET})

find_program(CARGO cargo HINTS "$ENV{USERPROFILE}/.cargo/bin" "$ENV{HOME}/.cargo/bin" REQUIRED)

# Cargo builds the static library with the NDK compiler of this ABI for the C parts (mimalloc). Trit builds
# without its GPU feature: TDT runs speech on the CPU.
# The target directory is the library's own, so every ABI and variant shares one build cache.
set(SPEECH_LIB ${ENGINE_DIR}/target/${RUST_TARGET}/release/libtdt_speech.a)
add_custom_target(tdt_speech_build
  COMMAND ${CMAKE_COMMAND} -E env
    CC_${RUST_TARGET_VAR}=${CMAKE_C_COMPILER}
    CFLAGS_${RUST_TARGET_VAR}=--target=${CMAKE_C_COMPILER_TARGET}
    AR_${RUST_TARGET_VAR}=${CMAKE_AR}
    ${CARGO} rustc --release --locked --lib --crate-type staticlib
      --target ${RUST_TARGET}
      --manifest-path ${ENGINE_DIR}/Cargo.toml
  BYPRODUCTS ${SPEECH_LIB}
  USES_TERMINAL
  VERBATIM)

add_library(tdt_speech STATIC IMPORTED)
set_target_properties(tdt_speech PROPERTIES IMPORTED_LOCATION ${SPEECH_LIB})
add_dependencies(tdt_speech tdt_speech_build)

add_library(tdt_engine SHARED engine_jni.cc)
target_include_directories(tdt_engine PRIVATE ${ENGINE_DIR}/include)
target_link_libraries(tdt_engine PRIVATE tdt_speech log dl m)
# Keep only what the bridge calls: with the library's symbols hidden, the Trit families it does not reach drop out.
target_link_options(tdt_engine PRIVATE -Wl,--exclude-libs,ALL -Wl,--gc-sections)
