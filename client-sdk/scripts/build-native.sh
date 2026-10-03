#!/usr/bin/env bash
# Builds the Rust core for every shipping target and lays the artifacts out the
# way the SDK packages expect them:
#
#   build/native/
#     linux-x64/libwabi_client.so        linux-arm64/libwabi_client.so
#     win-x64/wabi_client.dll            (MSVC on Windows, mingw when cross-compiling)
#     osx/libwabi_client.dylib           (universal: arm64 + x86_64, needs macOS)
#     apple/WabiClient.xcframework       (iOS device + simulator + macOS, needs Xcode)
#     ios/libwabi_client.dylib           ios-simulator/libwabi_client.dylib (arm64 + x86_64):
#                                        dynamic libraries for Dart native assets (Flutter), needs Xcode
#     android/jniLibs/<abi>/libwabi_client.so   (arm64-v8a armeabi-v7a x86_64 x86, needs NDK)
#
# Targets whose toolchain is missing are reported and skipped. Any other core
# (Go, Zig, ...) just has to produce files with the same names.
#
#   scripts/build-native.sh [linux|windows|apple|android|all]
#
# Other WIT packages with a Rust core reuse this script via these variables
# (e.g. WABI_CORE_DIR=packages/zero/core-rust WABI_LIB=gematik_zero):
#   WABI_CORE_DIR  crate dir (default core-rust)      WABI_LIB   library name (default wabi_client)
#   WABI_HEADER    generated/c/<prefix>.h             WABI_XCFRAMEWORK  name (default WabiClient)
#   WABI_NATIVE_OUT output dir (default build/native)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${WABI_NATIVE_OUT:-$ROOT/build/native}"
CORE="${WABI_CORE_DIR:-$ROOT/core-rust}"
LIB="${WABI_LIB:-wabi_client}"
HEADER="${WABI_HEADER:-$ROOT/generated/c/wabi_client.h}"
XCF="${WABI_XCFRAMEWORK:-WabiClient}"
TGT="${CARGO_TARGET_DIR:-$CORE/target}"
WHAT="${1:-all}"
mkdir -p "$OUT"
have() { command -v "$1" >/dev/null 2>&1; }
skip() { echo "SKIP $1: $2"; }
cargo_build() { (cd "$CORE" && cargo build --release --lib --target "$1" "${@:2}"); }

build_linux() {
  cargo_build x86_64-unknown-linux-gnu
  mkdir -p "$OUT/linux-x64" && cp "$TGT/x86_64-unknown-linux-gnu/release/lib$LIB.so" "$OUT/linux-x64/"
  if rustup target add aarch64-unknown-linux-gnu >/dev/null 2>&1; then
    # zig as linker and as C compiler (crates with C parts, e.g. ring)
    ZIG_TARGET=aarch64-linux-gnu CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$ROOT/scripts/zig-cc.sh" \
      CC_aarch64_unknown_linux_gnu="$ROOT/scripts/zig-cc.sh" cargo_build aarch64-unknown-linux-gnu
    mkdir -p "$OUT/linux-arm64" && cp "$TGT/aarch64-unknown-linux-gnu/release/lib$LIB.so" "$OUT/linux-arm64/"
  else skip linux-arm64 "rustup target unavailable"; fi
}

build_windows() {
  if [[ "${OS:-}" == "Windows_NT" ]]; then
    cargo_build x86_64-pc-windows-msvc
    mkdir -p "$OUT/win-x64" && cp "$TGT/x86_64-pc-windows-msvc/release/$LIB.dll" "$OUT/win-x64/"
  elif have x86_64-w64-mingw32-gcc && rustup target add x86_64-pc-windows-gnu >/dev/null 2>&1; then
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc cargo_build x86_64-pc-windows-gnu
    mkdir -p "$OUT/win-x64" && cp "$TGT/x86_64-pc-windows-gnu/release/$LIB.dll" "$OUT/win-x64/"
  else skip windows "needs Windows (MSVC) or mingw-w64"; fi
}

build_apple() {
  if [[ "$(uname)" != "Darwin" ]] || ! have xcodebuild; then skip apple "needs macOS + Xcode"; return; fi
  local T=(aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
  for t in "${T[@]}"; do rustup target add "$t" >/dev/null; cargo_build "$t"; done
  local R="$TGT"
  mkdir -p "$OUT/osx" "$OUT/apple/tmp"
  lipo -create "$R"/{aarch64,x86_64}-apple-darwin/release/lib$LIB.dylib -output "$OUT/osx/lib$LIB.dylib"
  lipo -create "$R"/{aarch64,x86_64}-apple-darwin/release/lib$LIB.a -output "$OUT/apple/tmp/macos.a"
  lipo -create "$R"/{aarch64-apple-ios-sim,x86_64-apple-ios}/release/lib$LIB.a -output "$OUT/apple/tmp/sim.a"
  # Headers + module map so Swift can `import C$XCF` (the package's C module, as
  # in the generated Swift code) from the binary target: generic ABI types + the
  # package's prefixed prototypes (generated/c).
  local H="$OUT/apple/tmp/include"
  mkdir -p "$H" && cp "$ROOT/abi/wabi_abi.h" "$ROOT/abi/wabi_abi_functions.h" "$HEADER" "$H/"
  printf 'module C%s {\n    header "wabi_abi.h"\n    header "%s"\n    export *\n}\n' "$XCF" "$(basename "$HEADER")" > "$H/module.modulemap"
  rm -rf "$OUT/apple/$XCF.xcframework"
  xcodebuild -create-xcframework \
    -library "$R/aarch64-apple-ios/release/lib$LIB.a" -headers "$H" \
    -library "$OUT/apple/tmp/sim.a" -headers "$H" \
    -library "$OUT/apple/tmp/macos.a" -headers "$H" \
    -output "$OUT/apple/$XCF.xcframework"
  rm -rf "$OUT/apple/tmp"
  # Dart native assets (Flutter) bundle a dynamic library per target instead.
  mkdir -p "$OUT/ios" "$OUT/ios-simulator"
  cp "$R/aarch64-apple-ios/release/lib$LIB.dylib" "$OUT/ios/"
  lipo -create "$R"/{aarch64-apple-ios-sim,x86_64-apple-ios}/release/lib$LIB.dylib -output "$OUT/ios-simulator/lib$LIB.dylib"
  for d in osx ios ios-simulator; do install_name_tool -id "@rpath/lib$LIB.dylib" "$OUT/$d/lib$LIB.dylib"; done
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
