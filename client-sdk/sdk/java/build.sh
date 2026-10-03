#!/usr/bin/env bash
# Builds the Java SDK of a WIT package. Everything comes from the package's
# generated/java (wabi-wit-gen writes the runtime there, in the package's own
# Java package <pkg>.runtime, plus the JNI shim in generated/java/jni):
#
#   sdk/java/build.sh [<package dir> [<jar name>]]    default: .  wabi-client
#
#   build/java/<jar>.jar                desktop/server: Panama (JDK 22+) and JNI bindings
#   build/java/<jar>-android.jar        Android: JNI binding only, Java 17 bytecode
#   build/java/lib<prefix>_jni.so       JNI shim of this package for this host
#                                       (Android: generated/java/jni/CMakeLists.txt)
#
# For another package the output goes to build/java-<jar>/ instead.
# Everything except the Panama binding is compiled with --release 17, so the
# same sources serve Android; the Panama classes (PanamaAbi, NativeAbi) are
# compiled with --release 22 and left out of the Android jar.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PKG="$(cd "$ROOT/${1:-.}" && pwd)"
NAME="${2:-wabi-client}"
OUT="$ROOT/build/java"
[ "$PKG" = "$ROOT" ] || OUT="$ROOT/build/java-$NAME"
GEN="$PKG/generated/java"
rm -rf "$OUT/classes" "$OUT/jni" && mkdir -p "$OUT/classes" "$OUT/jni"
PANAMA=($(find "$GEN" -name PanamaAbi.java -o -name NativeAbi.java))
COMMON=$(find "$GEN" -name '*.java' | grep -v -e PanamaAbi.java -e NativeAbi.java)
LINT="-Xlint:all,-serial,-restricted -Werror"
javac --release 17 $LINT -h "$OUT/jni" -d "$OUT/classes" $COMMON
javac --release 22 $LINT -cp "$OUT/classes" -d "$OUT/classes" "${PANAMA[@]}"
jar --create --file "$OUT/$NAME.jar" -C "$OUT/classes" .
jar --create --file "$OUT/$NAME-android.jar" \
  $(cd "$OUT/classes" && find . -name '*.class' ! -name 'PanamaAbi*' ! -name 'NativeAbi*' | sed 's|^|-C '"$OUT/classes"' |')
echo "built $OUT/$NAME.jar, $OUT/$NAME-android.jar"

# JNI shim of this package (host build). -include makes the compiler check every
# JNI function against the signatures javac generated from JniAbi.java.
SHIM=$(ls "$GEN"/jni/*_jni.c)
SHIM_LIB="lib$(basename "$SHIM" .c).so"
JAVA_HOME="${JAVA_HOME:-$(dirname "$(dirname "$(readlink -f "$(command -v javac)")")")}"
if [ -f "$JAVA_HOME/include/jni.h" ]; then
  cc -std=c11 -O2 -Wall -Wextra -Werror -fPIC -shared \
    -I"$JAVA_HOME/include" -I"$JAVA_HOME/include/linux" -I"$ROOT/abi" \
    -include "$(ls "$OUT"/jni/*_JniAbi.h)" \
    -o "$OUT/$SHIM_LIB" "$SHIM" -ldl -lpthread
  echo "built $OUT/$SHIM_LIB"
else
  echo "SKIP JNI shim: no jni.h under $JAVA_HOME"
fi
