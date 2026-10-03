#!/usr/bin/env bash
# Builds the gematik:pki Java core as a native shared library with GraalVM:
#
#   build/pki/libgematik_pki.so         the ABI library apps load (C shim, exports gematik_pki_*)
#   build/pki/libgematik_pki_image.so   the native image (core + gemLibPki + JDK), next to it
#
# Needs GraalVM for JDK 21+ with native-image (GRAALVM_HOME or JAVA_HOME) and a
# C compiler. The library needs no JVM at run time.
#
#   1. mvn package                      Java core (+ generated glue + runtime/java-core)
#   2. native-image --shared            libgematik_pki_image.so (exports only wabi_jvm_*/graal_*)
#   3. cc wabi_graal_shim.c             libgematik_pki.so: <prefix>_* ABI symbols -> isolate entry points,
#                                       linked against the image with rpath $ORIGIN
# (native-image hides every symbol but its own entry points, so the shim is a
#  separate, tiny library instead of being linked into the image.)
#
# Reachability metadata (reflection/resources of gemLibPki, JAXB, BouncyCastle)
# is in src/main/resources/META-INF/native-image and was recorded by running the
# JVM tests with the tracing agent:  ./build-native.sh --trace
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
OUT="${OUT:-$ROOT/build/pki}"
GRAAL="${GRAALVM_HOME:-${JAVA_HOME:-}}"
[ -x "$GRAAL/bin/native-image" ] || { echo "native-image not found: set GRAALVM_HOME to a GraalVM (JDK 21+)" >&2; exit 2; }
export JAVA_HOME="$GRAAL"
PREFIX=gematik_pki
CFG="$HERE/src/main/resources/META-INF/native-image/de.gematik.example/gematik-pki-core"
mkdir -p "$OUT"

if [ "${1:-}" = "--trace" ]; then # refresh the reachability metadata
  (cd "$HERE" && mvn -B -q test -DargLine="-agentlib:native-image-agent=config-merge-dir=$CFG,caller-filter-file=$HERE/native-image-filter.json")
  exit 0
fi

(cd "$HERE" && mvn -B -q package -DskipTests && mvn -B -q dependency:build-classpath -Dmdep.includeScope=runtime -Dmdep.outputFile=target/classpath.txt)
"$GRAAL/bin/native-image" --shared --no-fallback \
  -cp "$HERE/target/gematik-pki-core-0.1.0.jar:$(cat "$HERE/target/classpath.txt")" \
  -o "$OUT/lib${PREFIX}_image" \
  -H:+ReportExceptionStackTraces \
  --enable-url-protocols=http \
  ${NATIVE_IMAGE_ARGS:-}

cc -std=c11 -O2 -Wall -Wextra -Werror -fPIC -shared -DWABI_PREFIX=$PREFIX -I"$ROOT/abi" \
  -o "$OUT/lib$PREFIX.so" "$ROOT/runtime/java-core/native/wabi_graal_shim.c" \
  -L"$OUT" -l${PREFIX}_image -Wl,-rpath,'$ORIGIN' -lpthread
echo "built $OUT/lib$PREFIX.so (+ lib${PREFIX}_image.so)"
