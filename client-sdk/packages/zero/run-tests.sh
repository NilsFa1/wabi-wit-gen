#!/usr/bin/env bash
# gematik:zero PoC: the Go core (libzero) calls back into the app (ABI 1.1 host
# calls) to sign DPoP proofs. The same scenarios run from every SDK language;
# each app plays the keystore with its platform crypto and verifies the result.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PKG="$ROOT/packages/zero"
B="$ROOT/build/zero"
mkdir -p "$B"
have() { command -v "$1" >/dev/null 2>&1; }
DART="$(command -v dart || echo /opt/dart-sdk/bin/dart)"
declare -a SUMMARY=()
run() { # name, command...
  local name="$1"; shift
  if "$@" > "$B/last.log" 2>&1; then SUMMARY+=("PASS  $name"); else SUMMARY+=("FAIL  $name"); cat "$B/last.log"; fi
  { grep -E "passed|checks" "$B/last.log" || true; } | tail -1 | sed 's/^/        /'
}

echo "== generated code up to date"
(cd "$ROOT/generator" && cargo run --quiet -- --root ../packages/zero --check)

echo "== Go core (libzero) as a shared library"
(cd "$PKG/core-go" && CGO_ENABLED=1 go build -buildmode=c-shared -o "$B/libgematik_zero.so" .)
LIB="$B/libgematik_zero.so"

echo "== C: host-call protocol (OpenSSL as the app keystore)"
gcc -std=gnu11 -Wall -Wextra -Werror -O1 -o "$B/host_calls" "$ROOT/tests/abi/host_calls.c" \
  -I"$PKG/generated/c" -I"$ROOT/abi" -L"$B" -lgematik_zero -lcrypto -lpthread -Wl,-rpath,"$B"
run "C / host calls" "$B/host_calls"

if have dotnet; then
  run "C# / host calls" env ACME_NATIVE_LIB="$LIB" dotnet run --project "$PKG/tests/csharp" -c Release
else SUMMARY+=("SKIP  C# (no dotnet)"); fi

if have javac; then
  "$ROOT/sdk/java/build.sh" packages/zero gematik-zero >/dev/null
  J="$ROOT/build/java-gematik-zero"
  mkdir -p "$J/tests"
  javac --release 22 -cp "$J/gematik-zero.jar" -d "$J/tests" "$PKG/tests/java/ZeroTests.java"
  for binding in panama jni; do
    run "Java / host calls ($binding)" env ACME_NATIVE_LIB="$LIB" java -Dacme.binding=$binding \
      -Dacme.jni.lib="$J/libacme_jni.so" --enable-native-access=ALL-UNNAMED -cp "$J/gematik-zero.jar:$J/tests" ZeroTests
  done
else SUMMARY+=("SKIP  Java (no javac)"); fi

if [ -x "$DART" ]; then
  (cd "$PKG/sdk/dart" && "$DART" pub get >/dev/null)
  run "Dart / host calls" bash -c "cd '$PKG/sdk/dart' && ACME_NATIVE_LIB='$LIB' '$DART' test -r expanded"
else SUMMARY+=("SKIP  Dart (no dart)"); fi

if have swift; then
  run "Swift / host calls" bash -c "cd '$PKG/sdk/swift' && swift test -Xlinker -L'$B'"
else SUMMARY+=("SKIP  Swift (no swift toolchain here; generated + runtime code not compiled)"); fi

echo
echo "== gematik:zero host calls"
printf '%s\n' "${SUMMARY[@]}"
! printf '%s\n' "${SUMMARY[@]}" | grep -q '^FAIL'
