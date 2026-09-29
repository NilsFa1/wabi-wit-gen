#!/usr/bin/env bash
# Builds the Rust core for every shipping target and lays the artifacts out the
# way the SDK packages expect them:
#
#   build/native/
#     linux-x64/libacme_client.so        linux-arm64/libacme_client.so
#     win-x64/acme_client.dll            (MSVC on Windows, mingw when cross-compiling)
#     osx/libacme_client.dylib           (universal: arm64 + x86_64, needs macOS)
#     apple/AcmeClient.xcframework       (iOS device + simulator + macOS, needs Xcode)
#     android/jniLibs/<abi>/libacme_client.so   (arm64-v8a armeabi-v7a x86_64 x86, needs NDK)
#
# Targets whose toolchain is missing are reported and skipped. Any other core
# (Go, Zig, ...) just has to produce files with the same names.
#
#   scripts/build-native.sh [linux|windows|apple|android|all]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/build/native"
CORE="$ROOT/core-rust"
WHAT="${1:-all}"
mkdir -p "$OUT"
have() { command -v "$1" >/dev/null 2>&1; }
skip() { echo "SKIP $1: $2"; }
cargo_build() { (cd "$CORE" && cargo build --release --lib --target "$1" "${@:2}"); }

build_linux() {
  cargo_build x86_64-unknown-linux-gnu
  mkdir -p "$OUT/linux-x64" && cp "$CORE/target/x86_64-unknown-linux-gnu/release/libacme_client.so" "$OUT/linux-x64/"
  if rustup target add aarch64-unknown-linux-gnu >/dev/null 2>&1; then
    ZIG_TARGET=aarch64-linux-gnu CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$ROOT/scripts/zig-cc.sh" \
      cargo_build aarch64-unknown-linux-gnu
    mkdir -p "$OUT/linux-arm64" && cp "$CORE/target/aarch64-unknown-linux-gnu/release/libacme_client.so" "$OUT/linux-arm64/"
  else skip linux-arm64 "rustup target unavailable"; fi
}

build_windows() {
  if [[ "${OS:-}" == "Windows_NT" ]]; then
    cargo_build x86_64-pc-windows-msvc
    mkdir -p "$OUT/win-x64" && cp "$CORE/target/x86_64-pc-windows-msvc/release/acme_client.dll" "$OUT/win-x64/"
  elif have x86_64-w64-mingw32-gcc && rustup target add x86_64-pc-windows-gnu >/dev/null 2>&1; then
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc cargo_build x86_64-pc-windows-gnu
    mkdir -p "$OUT/win-x64" && cp "$CORE/target/x86_64-pc-windows-gnu/release/acme_client.dll" "$OUT/win-x64/"
  else skip windows "needs Windows (MSVC) or mingw-w64"; fi
}

build_apple() {
  if [[ "$(uname)" != "Darwin" ]] || ! have xcodebuild; then skip apple "needs macOS + Xcode"; return; fi
  local T=(aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
  for t in "${T[@]}"; do rustup target add "$t" >/dev/null; cargo_build "$t"; done
  local R="$CORE/target"
  mkdir -p "$OUT/osx" "$OUT/apple/tmp"
  lipo -create "$R"/{aarch64,x86_64}-apple-darwin/release/libacme_client.dylib -output "$OUT/osx/libacme_client.dylib"
  lipo -create "$R"/{aarch64,x86_64}-apple-darwin/release/libacme_client.a -output "$OUT/apple/tmp/macos.a"
  lipo -create "$R"/{aarch64-apple-ios-sim,x86_64-apple-ios}/release/libacme_client.a -output "$OUT/apple/tmp/sim.a"
  # Headers + module map so Swift can `import CAcmeClient` from the binary target:
  # generic ABI types + the package's prefixed prototypes (generated/c).
  local H="$OUT/apple/tmp/include"
  mkdir -p "$H" && cp "$ROOT/abi/acme_abi.h" "$ROOT/abi/acme_abi_functions.h" "$ROOT/generated/c/acme_client.h" "$H/"
  printf 'module CAcmeClient {\n    header "acme_abi.h"\n    header "acme_client.h"\n    export *\n}\n' > "$H/module.modulemap"
  rm -rf "$OUT/apple/AcmeClient.xcframework"
  xcodebuild -create-xcframework \
    -library "$R/aarch64-apple-ios/release/libacme_client.a" -headers "$H" \
    -library "$OUT/apple/tmp/sim.a" -headers "$H" \
    -library "$OUT/apple/tmp/macos.a" -headers "$H" \
    -output "$OUT/apple/AcmeClient.xcframework"
  rm -rf "$OUT/apple/tmp"
}

build_android() {
  if ! have cargo-ndk || [[ -z "${ANDROID_NDK_HOME:-}" ]]; then skip android "needs ANDROID_NDK_HOME + cargo-ndk"; return; fi
  rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android >/dev/null
  (cd "$CORE" && cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 -o "$OUT/android/jniLibs" build --release --lib)
}

case "$WHAT" in
  linux) build_linux ;;
  windows) build_windows ;;
  apple) build_apple ;;
  android) build_android ;;
  all) build_linux; build_windows; build_apple; build_android ;;
  *) echo "usage: $0 [linux|windows|apple|android|all]"; exit 2 ;;
esac
echo "artifacts:"; (cd "$OUT" && find . -name '*.so' -o -name '*.dll' -o -name '*.dylib' -o -name '*.xcframework' -prune | sort)
