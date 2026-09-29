#!/usr/bin/env bash
# gematik:pki PoC: TUC_PKI_018 certificate verification from every SDK, against
# two interchangeable cores implementing the same WIT contract:
#
#   go    gematik zero-lab gempki + TUC_PKI_018 layer (EUPL-1.2, core-go/): the default core
#   java  gematik ref-GemLibPki as a GraalVM native library (needs GRAALVM_HOME): reference
#         on Linux only, compared with the Go core by running the same tests
#
#   1. JVM tests of the Java core (wrapper + generated dispatcher); writes missing fixtures
#   2. Go core unit tests, then its c-shared build
#   3. the same scenarios from C#, Java (Panama, JNI) and Dart apps against each core
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

# run_apps <core> <library>: the unchanged SDK test apps against one core.
run_apps() {
  local core="$1"
  export ACME_NATIVE_LIB="$2"
  if have dotnet; then
    run "$core / C# app" dotnet run --project "$PKG/tests/csharp" -c Release
  fi
  if have javac; then
    local J="$ROOT/build/java-gematik-pki"
    if [ ! -f "$J/tests/PkiTests.class" ]; then
      "$ROOT/sdk/java/build.sh" packages/pki gematik-pki >/dev/null
      mkdir -p "$J/tests"
      javac --release 22 -cp "$J/gematik-pki.jar" -d "$J/tests" "$PKG/tests/java/PkiTests.java"
    fi
    for binding in panama jni; do
      run "$core / Java app ($binding)" java -Dacme.binding=$binding -Dacme.jni.lib="$J/libacme_jni.so" \
        --enable-native-access=ALL-UNNAMED -cp "$J/gematik-pki.jar:$J/tests" PkiTests
    done
  fi
  if [ -x "$DART" ]; then
    (cd "$PKG/sdk/dart" && "$DART" pub get >/dev/null)
    run "$core / Dart app" bash -c "cd '$PKG/sdk/dart' && '$DART' test -r expanded"
  fi
}
rm -f "$ROOT/build/java-gematik-pki/tests/PkiTests.class"

if [ -x "${GRAALVM_HOME:-/nonexistent}/bin/native-image" ]; then
  echo "== Java core: native-image build (GraalVM)"
  OUT="$B" "$PKG/core-java/build-native.sh" > "$B/native-image.log" 2>&1 || { cat "$B/native-image.log"; exit 1; }
  run_apps java "$B/libgematik_pki.so"
else
  SUMMARY+=("SKIP  java / native library + SDK apps (set GRAALVM_HOME to a GraalVM with native-image)")
fi

if have go; then
  echo "== Go core (gempki): unit tests + c-shared build"
  run "go / core unit tests" bash -c "cd '$PKG/core-go' && go test -count=1 -v . 2>&1 | tail -15"
  mkdir -p "$B/go"
  (cd "$PKG/core-go" && CGO_ENABLED=1 go build -buildmode=c-shared -o "$B/go/libgematik_pki.so" .)
  run_apps go "$B/go/libgematik_pki.so"
else
  SUMMARY+=("SKIP  go core (no go toolchain)")
fi
have swift || SUMMARY+=("SKIP  Swift (no swift toolchain here; macOS CI)")

echo
echo "== gematik:pki (Java core: gemLibPki, Go core: gempki)"
printf '%s\n' "${SUMMARY[@]}"
! printf '%s\n' "${SUMMARY[@]}" | grep -q '^FAIL'
