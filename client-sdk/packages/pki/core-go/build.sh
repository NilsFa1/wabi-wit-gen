#!/usr/bin/env bash
# Cross-compiles the gematik:pki Go core (gempki) into the native library the
# SDKs load. One target per call; output in build/pki-go/<target>/.
#
#   linux-x64 | linux-arm64 | windows-x64     on Linux, with zig as C cross compiler
#   android                                    on Linux, with the Android NDK (ANDROID_NDK)
#                                              -> android/jniLibs/{arm64-v8a,x86_64}/libgematik_pki.so
#   macos                                      on macOS: universal (arm64 + x86_64) libgematik_pki.dylib
#   ios                                        on macOS: GematikPki.xcframework (device + simulator,
#                                              static library, for Swift/Xcode) plus dynamic libraries
#                                              ios/ and ios-simulator/ for Dart native assets (Flutter)
#
# Every library is checked for the ABI entry points (gematik_pki_*).
set -euo pipefail
TARGET="${1:?usage: build.sh linux-x64|linux-arm64|windows-x64|android|macos|ios}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
OUT="${OUT:-$ROOT/build/pki-go/$TARGET}"
PREFIX=gematik_pki
mkdir -p "$OUT"
export CGO_ENABLED=1

gobuild() { # mode output, env from the caller
  (cd "$HERE" && go build -trimpath -ldflags="-s -w" -buildmode="$1" -o "$2" .)
  echo "   built $2"
}
check() { # nm-command file
  local syms; syms=$("${@:1:$#-1}" "${@: -1}")
  for s in call_sync call_async future_subscribe object_drop core_info; do
    grep -q "${PREFIX}_$s" <<<"$syms" || { echo "missing ${PREFIX}_$s in ${*: -1}" >&2; exit 1; }
  done
  echo "ok: ${*: -1} ($(grep -c "[ _]${PREFIX}_" <<<"$syms") ABI symbols)"
}
zigcc() { # zig target -> CC wrapper
  export ZIG_TARGET="$1" CC="$ROOT/scripts/zig-cc.sh"
}

case "$TARGET" in
  linux-x64)   zigcc x86_64-linux-gnu.2.28;  GOOS=linux GOARCH=amd64 gobuild c-shared "$OUT/lib$PREFIX.so"
               check nm -D --defined-only "$OUT/lib$PREFIX.so" ;;
  linux-arm64) zigcc aarch64-linux-gnu.2.28; GOOS=linux GOARCH=arm64 gobuild c-shared "$OUT/lib$PREFIX.so"
               check nm -D --defined-only "$OUT/lib$PREFIX.so" ;;
  windows-x64) zigcc x86_64-windows-gnu;     GOOS=windows GOARCH=amd64 gobuild c-shared "$OUT/$PREFIX.dll"
               check objdump -p "$OUT/$PREFIX.dll" ;;
  android)
    NDK="${ANDROID_NDK:?set ANDROID_NDK to an Android NDK}"
    BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
    for abi in arm64-v8a:arm64:aarch64 x86_64:amd64:x86_64; do
      IFS=: read -r dir goarch triple <<<"$abi"
      mkdir -p "$OUT/jniLibs/$dir"
      CC="$BIN/$triple-linux-android24-clang" GOOS=android GOARCH=$goarch gobuild c-shared "$OUT/jniLibs/$dir/lib$PREFIX.so"
      check "$BIN/llvm-nm" -D --defined-only "$OUT/jniLibs/$dir/lib$PREFIX.so"
    done ;;
  macos)
    for arch in arm64:arm64 x86_64:amd64; do
      IFS=: read -r carch goarch <<<"$arch"
      echo "== macOS $carch ($(go version), $(clang --version | head -1))"
      CC=clang CGO_CFLAGS="-arch $carch -mmacosx-version-min=11.0" CGO_LDFLAGS="-arch $carch -mmacosx-version-min=11.0" \
        GOOS=darwin GOARCH=$goarch gobuild c-shared "$OUT/lib${PREFIX}_$carch.dylib"
    done
    lipo -create -output "$OUT/lib$PREFIX.dylib" "$OUT/lib${PREFIX}_arm64.dylib" "$OUT/lib${PREFIX}_x86_64.dylib"
    install_name_tool -id "@rpath/lib$PREFIX.dylib" "$OUT/lib$PREFIX.dylib"
    rm "$OUT/lib${PREFIX}_arm64.dylib" "$OUT/lib${PREFIX}_x86_64.dylib"
    lipo -info "$OUT/lib$PREFIX.dylib"
    check nm -gU "$OUT/lib$PREFIX.dylib" ;;
  ios)
    W="$OUT/work"; mkdir -p "$W/device" "$W/sim" "$W/headers"
    cp "$ROOT/abi/wabi_abi.h" "$HERE/../generated/c/$PREFIX.h" "$W/headers/"
    sdk() { xcrun -sdk "$1" --show-sdk-path; }
    echo "== iOS device arm64"
    CC=clang CGO_CFLAGS="-arch arm64 -miphoneos-version-min=13.0 -isysroot $(sdk iphoneos)" \
      CGO_LDFLAGS="-arch arm64 -miphoneos-version-min=13.0 -isysroot $(sdk iphoneos)" \
      GOOS=ios GOARCH=arm64 gobuild c-archive "$W/device/lib$PREFIX.a"
    for arch in arm64:arm64 x86_64:amd64; do # simulator slices, merged
      IFS=: read -r carch goarch <<<"$arch"
      echo "== iOS simulator $carch"
      flags="-arch $carch -mios-simulator-version-min=13.0 -isysroot $(sdk iphonesimulator)"
      CC=clang CGO_CFLAGS="$flags" CGO_LDFLAGS="$flags" \
        GOOS=ios GOARCH=$goarch gobuild c-archive "$W/sim/lib${PREFIX}_$carch.a"
    done
    lipo -create -output "$W/sim/lib$PREFIX.a" "$W/sim/lib${PREFIX}_arm64.a" "$W/sim/lib${PREFIX}_x86_64.a"
    rm -rf "$OUT/GematikPki.xcframework"
    xcodebuild -create-xcframework \
      -library "$W/device/lib$PREFIX.a" -headers "$W/headers" \
      -library "$W/sim/lib$PREFIX.a" -headers "$W/headers" \
      -output "$OUT/GematikPki.xcframework"
    check nm -gU "$W/device/lib$PREFIX.a"
    # Dart native assets (Flutter) bundle a dynamic library: link the same archives into
    # ios/lib$PREFIX.dylib (device) and ios-simulator/lib$PREFIX.dylib (arm64 + x86_64).
    SIM_OUT="$(dirname "$OUT")/ios-simulator"; mkdir -p "$SIM_OUT"
    dylib() { # output archive clang-flags...
      clang -dynamiclib "${@:3}" -Wl,-all_load "$2" -framework CoreFoundation -framework Security -lresolv \
        -install_name "@rpath/lib$PREFIX.dylib" -o "$1"
    }
    dylib "$OUT/lib$PREFIX.dylib" "$W/device/lib$PREFIX.a" -arch arm64 -miphoneos-version-min=13.0 -isysroot "$(sdk iphoneos)"
    dylib "$SIM_OUT/lib$PREFIX.dylib" "$W/sim/lib$PREFIX.a" -arch arm64 -arch x86_64 -mios-simulator-version-min=13.0 \
      -isysroot "$(sdk iphonesimulator)"
    check nm -gU "$OUT/lib$PREFIX.dylib"
    check nm -gU "$SIM_OUT/lib$PREFIX.dylib"
    rm -rf "$W" ;;
  *) echo "unknown target $TARGET" >&2; exit 2 ;;
esac
find "$OUT" -type f \( -name "*.so" -o -name "*.dll" -o -name "*.dylib" -o -name "*.a" \) -exec ls -la {} \;
