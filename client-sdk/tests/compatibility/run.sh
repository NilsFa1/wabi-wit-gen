#!/usr/bin/env bash
# Compatibility matrix: the SAME, unchanged SDK test binaries (C#, Java) and the
# language-neutral C conformance suite run against every core implementation.
# If this passes, the SDKs depend only on the native ABI — not on Rust.
#
#   tests/compatibility/run.sh            # all cores whose toolchain is installed
#   CORES="rust go" tests/compatibility/run.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
B="$ROOT/build"
mkdir -p "$B/fake" "$B/go" "$B/zig"
have() { command -v "$1" >/dev/null 2>&1; }
zigc() { if have zig; then zig "$@"; else python3 -m ziglang "$@"; fi; }
have_zig() { have zig || python3 -c "import ziglang" 2>/dev/null; }

echo "== building cores"
(cd "$ROOT/core-rust" && cargo build --release --quiet)
declare -A LIB=( [rust]="${CARGO_TARGET_DIR:-$ROOT/core-rust/target}/release/libacme_client.so" )
declare -A PROFILE=( [rust]=full [go]=full [zig]=basic )
if have go; then
  (cd "$ROOT/alt-cores/go" && CGO_ENABLED=1 go build -buildmode=c-shared -o "$B/go/libacme_client.so" .)
  LIB[go]="$B/go/libacme_client.so"
fi
if have_zig; then
  (cd "$ROOT/alt-cores/zig" && zigc build-lib -dynamic -lc -O ReleaseSafe acme_core.zig -femit-bin="$B/zig/libacme_client.so")
  rm -f "$ROOT/alt-cores/zig"/*.o
  LIB[zig]="$B/zig/libacme_client.so"
fi

echo "== building test fixtures and SDK test binaries"
gcc -shared -fPIC -o "$B/fake/libfake_abi_v2.so" "$ROOT/tests/compatibility/fake_core.c" '-DFAKE_ABI_VERSION=(2u<<16)'
gcc -shared -fPIC -o "$B/fake/libfake_contract_v2.so" "$ROOT/tests/compatibility/fake_core.c" '-DFAKE_CONTRACT="acme:client@2.0.0"'
gcc -shared -fPIC -o "$B/fake/libfake_no_streams.so" "$ROOT/tests/compatibility/fake_core.c" '-DFAKE_CAPS=(ACME_CAP_SYNC_CALLS|ACME_CAP_ASYNC_CALLS)'
gcc -std=c11 -Wall -Wextra -Werror -O1 -o "$B/conformance" "$ROOT/tests/abi/conformance.c" -ldl -lpthread
HAVE_DOTNET=0; HAVE_JAVA=0
if have dotnet; then
  dotnet build "$ROOT/tests/csharp" -nologo -v q -c Release -o "$B/csharp-tests" >/dev/null && HAVE_DOTNET=1
fi
if have javac; then
  "$ROOT/sdk/java/build.sh" >/dev/null
  javac --release 22 -cp "$B/java/acme-client.jar" -d "$B/java/tests" "$ROOT/tests/java/AcmeClientTests.java" && HAVE_JAVA=1
fi
export ACME_FAKE_CORES_DIR="$B/fake"

declare -a SUMMARY=()
run() { # name, command...
  local name="$1"; shift
  if "$@" > "$B/last.log" 2>&1; then SUMMARY+=("PASS  $name"); else SUMMARY+=("FAIL  $name"); cat "$B/last.log"; fi
  grep -E "passed|checks" "$B/last.log" | tail -1 | sed 's/^/        /'
}

for core in ${CORES:-rust go zig}; do
  lib="${LIB[$core]:-}"
  [ -n "$lib" ] || { SUMMARY+=("SKIP  $core core (toolchain missing)"); continue; }
  prof="${PROFILE[$core]}"
  flag=""; [ "$prof" = basic ] && flag="--basic"
  echo "== core: $core ($prof profile)"
  run "$core / C ABI conformance" "$B/conformance" "$lib" --profile "$prof"
  [ $HAVE_DOTNET = 1 ] && run "$core / C# SDK tests" env ACME_NATIVE_LIB="$lib" dotnet "$B/csharp-tests/Acme.Client.Tests.dll" $flag
  [ $HAVE_JAVA = 1 ] && run "$core / Java SDK tests" env ACME_NATIVE_LIB="$lib" \
      java --enable-native-access=ALL-UNNAMED -cp "$B/java/acme-client.jar:$B/java/tests" AcmeClientTests $flag
done

echo; echo "== compatibility matrix"
printf '%s\n' "${SUMMARY[@]}"
! printf '%s\n' "${SUMMARY[@]}" | grep -q '^FAIL'
