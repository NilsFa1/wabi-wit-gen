#!/usr/bin/env bash
# gematik:pki PoC: gematik ref-GemLibPki (Java) as a native library.
#
#   1. JVM tests of the Java core (wrapper + generated dispatcher), writes the fixtures
#   2. native-image build of the core            (needs GraalVM: GRAALVM_HOME)
#   3. the same scenarios from C#, Java (Panama, JNI) and Dart apps against the .so
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PKG="$ROOT/packages/pki"
B="$ROOT/build/pki"
mkdir -p "$B"
have() { command -v "$1" >/dev/null 2>&1; }
DART="$(command -v dart || echo /opt/dart-sdk/bin/dart)"
declare -a SUMMARY=()
run() { # name, command...
  local name="$1"; shift
  if "$@" > "$B/last.log" 2>&1; then SUMMARY+=("PASS  $name"); else SUMMARY+=("FAIL  $name"); cat "$B/last.log"; fi
  { grep -E "passed|Tests run:|All tests" "$B/last.log" || true; } | tail -1 | sed 's/^/        /'
}

echo "== generated code up to date"
(cd "$ROOT/generator" && cargo run --quiet -- --root ../packages/pki --check)

echo "== Java core on the JVM (writes test fixtures)"
export PKI_FIXTURES="$PKG/tests/fixtures"
run "Java core / JVM tests" bash -c "cd '$PKG/core-java' && mvn -B -q test -Dpki.fixtures='$PKI_FIXTURES' && cat target/surefire-reports/*.txt"

if [ -x "${GRAALVM_HOME:-/nonexistent}/bin/native-image" ]; then
  echo "== native-image build (GraalVM)"
  OUT="$B" "$PKG/core-java/build-native.sh" > "$B/native-image.log" 2>&1 || { cat "$B/native-image.log"; exit 1; }
  LIB="$B/libgematik_pki.so"
  export ACME_NATIVE_LIB="$LIB"
  if have dotnet; then
    run "C# app" dotnet run --project "$PKG/tests/csharp" -c Release
  fi
  if have javac; then
    "$ROOT/sdk/java/build.sh" packages/pki gematik-pki >/dev/null
    J="$ROOT/build/java-gematik-pki"
    mkdir -p "$J/tests"
    javac --release 22 -cp "$J/gematik-pki.jar" -d "$J/tests" "$PKG/tests/java/PkiTests.java"
    for binding in panama jni; do
      run "Java app ($binding)" java -Dacme.binding=$binding -Dacme.jni.lib="$J/libacme_jni.so" \
        --enable-native-access=ALL-UNNAMED -cp "$J/gematik-pki.jar:$J/tests" PkiTests
    done
  fi
  if [ -x "$DART" ]; then
    (cd "$PKG/sdk/dart" && "$DART" pub get >/dev/null)
    run "Dart app" bash -c "cd '$PKG/sdk/dart' && '$DART' test -r expanded"
  fi
else
  SUMMARY+=("SKIP  native library + SDK apps (set GRAALVM_HOME to a GraalVM with native-image)")
fi
have swift || SUMMARY+=("SKIP  Swift (no swift toolchain here; macOS CI)")

echo
echo "== gematik:pki (gemLibPki as a native library)"
printf '%s\n' "${SUMMARY[@]}"
! printf '%s\n' "${SUMMARY[@]}" | grep -q '^FAIL'
