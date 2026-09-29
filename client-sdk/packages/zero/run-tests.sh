#!/usr/bin/env bash
# gematik:zero PoC: the Go core (libzero) calls back into the app (ABI 1.1 host
# calls) to sign DPoP proofs. The same scenarios run from every SDK language;
# each app plays the keystore with its platform crypto and verifies the result.
# Every app runs against every core (Go with gematik libzero, Rust, C#).
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

echo "== cores: three interchangeable implementations of gematik:zero"
declare -A LIBS=()
if have go; then # Go + gematik libzero
  mkdir -p "$B/go"
  (cd "$PKG/core-go" && CGO_ENABLED=1 go build -buildmode=c-shared -o "$B/go/libgematik_zero.so" .)
  LIBS[go]="$B/go"
fi
if have cargo; then # Rust + acme-abi-rt
  (cd "$PKG/core-rust" && cargo build --release --quiet)
  mkdir -p "$B/rust" && cp "${CARGO_TARGET_DIR:-$PKG/core-rust/target}/release/libgematik_zero.so" "$B/rust/"
  LIBS[rust]="$B/rust"
fi
if have dotnet; then # C# NativeAOT + runtime/csharp-core
  (cd "$PKG/core-csharp" && dotnet publish -c Release -r linux-x64 -nologo -v q -o "$B/csharp/publish" >/dev/null)
  cp "$B/csharp/publish/gematik_zero.so" "$B/csharp/libgematik_zero.so"
  LIBS[csharp]="$B/csharp"
fi

echo "== app-side test programs"
FIRST="${LIBS[go]:-${LIBS[rust]:-${LIBS[csharp]}}}"
gcc -std=gnu11 -Wall -Wextra -Werror -O1 -o "$B/host_calls" "$ROOT/tests/abi/host_calls.c" \
  -I"$PKG/generated/c" -I"$ROOT/abi" -L"$FIRST" -lgematik_zero -lcrypto -lpthread
HAVE_JAVA=0; HAVE_DART=0
if have javac; then
  "$ROOT/sdk/java/build.sh" packages/zero gematik-zero >/dev/null
  J="$ROOT/build/java-gematik-zero"
  mkdir -p "$J/tests"
  javac --release 22 -cp "$J/gematik-zero.jar" -d "$J/tests" "$PKG/tests/java/ZeroTests.java" && HAVE_JAVA=1
fi
if [ -x "$DART" ]; then (cd "$PKG/sdk/dart" && "$DART" pub get >/dev/null) && HAVE_DART=1; fi

for core in go rust csharp; do
  dir="${LIBS[$core]:-}"
  [ -n "$dir" ] || { SUMMARY+=("SKIP  $core core (toolchain missing)"); continue; }
  LIB="$dir/libgematik_zero.so"
  echo "== core: $core"
  run "$core / C host-call protocol" env LD_LIBRARY_PATH="$dir" "$B/host_calls"
  if have dotnet; then
    run "$core / C# app" env ACME_NATIVE_LIB="$LIB" dotnet run --project "$PKG/tests/csharp" -c Release
  fi
  if [ $HAVE_JAVA = 1 ]; then
    for binding in panama jni; do
      run "$core / Java app ($binding)" env ACME_NATIVE_LIB="$LIB" java -Dacme.binding=$binding \
        -Dacme.jni.lib="$J/libacme_jni.so" --enable-native-access=ALL-UNNAMED -cp "$J/gematik-zero.jar:$J/tests" ZeroTests
    done
  fi
  if [ $HAVE_DART = 1 ]; then
    run "$core / Dart app" bash -c "cd '$PKG/sdk/dart' && ACME_NATIVE_LIB='$LIB' '$DART' test -r expanded"
  fi
done

if have swift; then
  run "Swift app (Go core)" bash -c "cd '$PKG/sdk/swift' && swift test -Xlinker -L'$B/go'"
else SUMMARY+=("SKIP  Swift (no swift toolchain here; generated + runtime code not compiled)"); fi

echo
echo "== gematik:zero host calls"
printf '%s\n' "${SUMMARY[@]}"
! printf '%s\n' "${SUMMARY[@]}" | grep -q '^FAIL'
