#!/usr/bin/env bash
# Builds the Java SDK of a WIT package (hand-written runtime + generated sources):
#
#   sdk/java/build.sh [<package dir> [<jar name>]]    default: .  acme-client
#
#   build/java/<jar>.jar                desktop/server: Panama (JDK 22+) and JNI bindings
#   build/java/<jar>-android.jar        Android: JNI binding only, Java 17 bytecode
#   build/java/libacme_jni.so           generic JNI shim for this host (Android: see runtime/java/jni/CMakeLists.txt)
#
# For another package the output goes to build/java-<jar>/ instead.
# Everything except the Panama binding is compiled with --release 17, so the
# same sources serve Android; the Panama classes (PanamaAbi, NativeAbi) are
# compiled with --release 22 and left out of the Android jar.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PKG="$(cd "$ROOT/${1:-.}" && pwd)"
NAME="${2:-acme-client}"
OUT="$ROOT/build/java"
[ "$PKG" = "$ROOT" ] || OUT="$ROOT/build/java-$NAME"
GEN="$PKG/generated/java"
rm -rf "$OUT/classes" "$OUT/jni" && mkdir -p "$OUT/classes" "$OUT/jni"
PANAMA=("$ROOT/runtime/java/com/acme/client/runtime/PanamaAbi.java" "$GEN/com/acme/client/runtime/NativeAbi.java")
COMMON=$(find "$ROOT/runtime/java" "$GEN" -name '*.java' | grep -v -e PanamaAbi.java -e NativeAbi.java)
LINT="-Xlint:all,-serial,-restricted -Werror"
javac --release 17 $LINT -h "$OUT/jni" -d "$OUT/classes" $COMMON
javac --release 22 $LINT -cp "$OUT/classes" -d "$OUT/classes" "${PANAMA[@]}"
jar --create --file "$OUT/$NAME.jar" -C "$OUT/classes" .
jar --create --file "$OUT/$NAME-android.jar" \
  $(cd "$OUT/classes" && find . -name '*.class' ! -name 'PanamaAbi*' ! -name 'NativeAbi*' | sed 's|^|-C '"$OUT/classes"' |')
echo "built $OUT/$NAME.jar, $OUT/$NAME-android.jar"

# Generic JNI shim (host build). -include makes the compiler check every JNI
# function against the signatures javac generated from JniAbi.java.
JAVA_HOME="${JAVA_HOME:-$(dirname "$(dirname "$(readlink -f "$(command -v javac)")")")}"
if [ -f "$JAVA_HOME/include/jni.h" ]; then
  cc -std=c11 -O2 -Wall -Wextra -Werror -fPIC -shared \
    -I"$JAVA_HOME/include" -I"$JAVA_HOME/include/linux" -I"$ROOT/abi" \
    -include "$OUT/jni/com_acme_client_runtime_JniAbi.h" \
    -o "$OUT/libacme_jni.so" "$ROOT/runtime/java/jni/acme_jni.c" -ldl -lpthread
  echo "built $OUT/libacme_jni.so"
else
  echo "SKIP JNI shim: no jni.h under $JAVA_HOME"
fi
