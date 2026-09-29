#!/usr/bin/env bash
# Experiment: the gematik:pki Java core (gemLibPki) as a native library for
# Android (arm64) or iOS (arm64) with Gluon Substrate + Gluon's GraalVM build.
#
#   ./build-mobile.sh android     on Linux  (Android SDK/NDK: ANDROID_SDK / ANDROID_NDK, else downloaded)
#   ./build-mobile.sh ios         on macOS  (Xcode)
#
# Needs GRAALVM_HOME pointing to a Gluon GraalVM (github.com/gluonhq/graal/releases).
# Output, same two-library layout as build-native.sh:
#   build/pki-mobile/<target>/libgematik_pki_image.{so,dylib}   the native image (core + gemLibPki + JDK)
#   build/pki-mobile/<target>/libgematik_pki.{so,dylib}         the C shim with the gematik_pki_* ABI symbols
set -euo pipefail
TARGET="${1:?usage: build-mobile.sh android|ios}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
OUT="${OUT:-$ROOT/build/pki-mobile/$TARGET}"
PREFIX=gematik_pki
[ -x "${GRAALVM_HOME:-}/bin/native-image" ] || { echo "set GRAALVM_HOME to a Gluon GraalVM" >&2; exit 2; }
export JAVA_HOME="$GRAALVM_HOME"
mkdir -p "$OUT"

(cd "$HERE" && mvn -B -Pmobile package -DskipTests gluonfx:sharedlib -Dgluonfx.target="$TARGET" -Dgluonfx.verbose=true)

GVM="$HERE/target/gluonfx"
case "$TARGET" in
  android)
    IMG=$(find "$GVM" -name "*.so" -path "*android*" ! -name "libsubstrate.so" | head -1)
    [ -n "$IMG" ] || { echo "no shared library produced under $GVM" >&2; find "$GVM" -maxdepth 4 >&2; exit 1; }
    cp "$IMG" "$OUT/lib${PREFIX}_image.so"
    NDK="${ANDROID_NDK:-$(ls -d "$HOME"/.gluon/substrate/Android/ndk/* | tail -1)}"
    CC="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android28-clang"
    "$CC" -std=c11 -O2 -Wall -Wextra -Werror -fPIC -shared -DACME_PREFIX=$PREFIX -I"$ROOT/abi" \
      -o "$OUT/lib$PREFIX.so" "$ROOT/runtime/java-core/native/acme_graal_shim.c" \
      -L"$OUT" -l${PREFIX}_image
    NM="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-nm"
    SYMS=$("$NM" -D --defined-only "$OUT/lib${PREFIX}_image.so")
    ABI=$("$NM" -D --defined-only "$OUT/lib$PREFIX.so")
    file "$OUT"/*.so || true
    ;;
  ios)
    IMG=$(find "$GVM" \( -name "*.dylib" -o -name "*.so" \) -path "*ios*" | head -1)
    [ -n "$IMG" ] || { echo "no shared library produced under $GVM" >&2; find "$GVM" -maxdepth 4 >&2; exit 1; }
    cp "$IMG" "$OUT/lib${PREFIX}_image.dylib"
    install_name_tool -id "@rpath/lib${PREFIX}_image.dylib" "$OUT/lib${PREFIX}_image.dylib"
    xcrun -sdk iphoneos clang -std=c11 -O2 -Wall -Wextra -Werror -fPIC -dynamiclib -arch arm64 \
      -mios-version-min=12.0 -DACME_PREFIX=$PREFIX -I"$ROOT/abi" \
      -install_name "@rpath/lib$PREFIX.dylib" -Wl,-rpath,@loader_path \
      -o "$OUT/lib$PREFIX.dylib" "$ROOT/runtime/java-core/native/acme_graal_shim.c" \
      -L"$OUT" -l${PREFIX}_image
    SYMS=$(nm -gU "$OUT/lib${PREFIX}_image.dylib")
    ABI=$(nm -gU "$OUT/lib$PREFIX.dylib")
    file "$OUT"/*.dylib; otool -L "$OUT/lib$PREFIX.dylib"
    ;;
  *) echo "unknown target $TARGET" >&2; exit 2 ;;
esac

# The image must export the core's entry points, the shim the public ABI.
echo "$SYMS" | grep -q "acme_jvm_call_sync" || { echo "image does not export acme_jvm_* entry points" >&2; exit 1; }
echo "$SYMS" | grep -q "graal_create_isolate" || { echo "image does not export graal_create_isolate" >&2; exit 1; }
echo "$ABI" | grep -q "${PREFIX}_call_sync" || { echo "shim does not export ${PREFIX}_call_sync" >&2; exit 1; }
ls -la "$OUT"
echo "built $TARGET: $(echo "$SYMS" | grep -c acme_jvm_) acme_jvm_* entry points, $(echo "$ABI" | grep -c " _\?${PREFIX}_") ${PREFIX}_* ABI symbols"
