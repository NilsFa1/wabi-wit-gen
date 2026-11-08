#!/usr/bin/env bash
# Compatibility matrix: the SAME, unchanged SDK test binaries (C#, Java), the
# Dart and TypeScript test suites and the language-neutral C suites (ABI
# conformance, host calls) run against every core implementation.
# If this passes, the SDKs depend only on the native ABI — not on Rust.
#
#   rust       Rust core, mock service in-process (mem://)
#   rust-http  Rust core, mock service over HTTP (demo-mock-server)
#   go         Go core, the same mock service over HTTP
#   csharp     C# core (NativeAOT), the same mock service over HTTP
#
#   example/tests/compatibility/run.sh            # everything whose toolchain is installed
#   CORES="rust go" example/tests/compatibility/run.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)" # example/
B="$ROOT/build"
mkdir -p "$B/fake" "$B/go" "$B/csharp-core"
have() { command -v "$1" >/dev/null 2>&1; }

echo "== building cores and the mock service"
(cd "$ROOT/core-rust" && cargo build --release --quiet --features mock-server)
TARGET="${CARGO_TARGET_DIR:-$ROOT/core-rust/target}/release"
declare -A LIB=( [rust]="$TARGET/libdemo_client.so" )
if have go; then
  (cd "$ROOT/core-go" && CGO_ENABLED=1 go build -buildmode=c-shared -o "$B/go/libdemo_client.so" .)
  LIB[go]="$B/go/libdemo_client.so"
fi
if have dotnet; then # compiled ahead-of-time to a plain native library (needs clang + zlib headers)
  case "$(uname -m)" in aarch64|arm64) RID=linux-arm64 ;; *) RID=linux-x64 ;; esac
  (cd "$ROOT/core-csharp" && dotnet publish -c Release -r "$RID" -nologo -v q -o "$B/csharp-core/publish" >/dev/null)
  cp "$B/csharp-core/publish/demo_client.so" "$B/csharp-core/libdemo_client.so"
  LIB[csharp]="$B/csharp-core/libdemo_client.so"
fi
# The mock service over real HTTP. The Rust core also has it in-process
# (mem://, the default of every test suite); the Go and C# cores speak HTTP only.
MOCK_ADDR="127.0.0.1:${WABI_MOCK_PORT:-19001}"
"$TARGET/demo-mock-server" "$MOCK_ADDR" 2>"$B/mock-server.log" &
MOCK_PID=$!
trap 'kill $MOCK_PID 2>/dev/null || true' EXIT
for _ in $(seq 50); do (exec 3<>"/dev/tcp/${MOCK_ADDR%:*}/${MOCK_ADDR#*:}") 2>/dev/null && break; sleep 0.1; done
declare -A ENDPOINT=( [rust]="mem://tests" [rust-http]="http://$MOCK_ADDR" [go]="http://$MOCK_ADDR" [csharp]="http://$MOCK_ADDR" )
LIB[rust-http]="${LIB[rust]}"
echo "== building test fixtures and SDK test binaries"
ABI="$ROOT/sdk/abi" # the headers the generator wrote
gcc -shared -fPIC -I"$ABI" -o "$B/fake/libfake_abi_v2.so" "$ROOT/tests/compatibility/fake_core.c" '-DFAKE_ABI_VERSION=(2u<<16)'
gcc -shared -fPIC -I"$ABI" -o "$B/fake/libfake_contract_v2.so" "$ROOT/tests/compatibility/fake_core.c" '-DFAKE_CONTRACT="demo:client@2.0.0"'
gcc -shared -fPIC -I"$ABI" -o "$B/fake/libfake_no_streams.so" "$ROOT/tests/compatibility/fake_core.c" '-DFAKE_CAPS=(WABI_CAP_SYNC_CALLS|WABI_CAP_ASYNC_CALLS)'
gcc -std=c11 -Wall -Wextra -Werror -O1 -I"$ABI" -o "$B/conformance" "$ROOT/tests/abi/conformance.c" -ldl -lpthread
# host calls: the C program is the app and holds the key (OpenSSL); linked against
# the Rust core, run against any core through LD_LIBRARY_PATH (same library name)
HAVE_HOST_C=0
if echo '#include <openssl/evp.h>' | gcc -E - >/dev/null 2>&1; then
  gcc -std=gnu11 -Wall -Wextra -Werror -O1 -o "$B/host_calls" "$ROOT/tests/abi/host_calls.c" \
    -I"$ROOT/sdk/c" -I"$ABI" -L"$TARGET" -ldemo_client -lcrypto -lpthread
  HAVE_HOST_C=1
fi
HAVE_DOTNET=0; HAVE_JAVA=0; HAVE_DART=0; HAVE_BUN=0
DART="$(command -v dart || echo /opt/dart-sdk/bin/dart)"
if have dotnet; then
  dotnet build "$ROOT/tests/csharp" -nologo -v q -c Release -o "$B/csharp-tests" >/dev/null
  HAVE_DOTNET=1
fi
if have javac; then
  bash "$ROOT/sdk/java/build.sh" "$B/java" >/dev/null
  javac --release 22 -cp "$B/java/demo-client.jar" -d "$B/java/tests" "$ROOT/tests/java/DemoClientTests.java"
  HAVE_JAVA=1
fi
if [ -x "$DART" ]; then
  gcc -std=c11 -shared -fPIC -Wall -Wextra -Werror -I"$ABI" \
    "$ROOT/tests/abi/host_lifetime_probe.c" -o "$B/libhost_lifetime_probe.so"
  export WABI_HOST_PROBE="$B/libhost_lifetime_probe.so"
  (cd "$ROOT/tests/dart" && "$DART" pub get >/dev/null)
  HAVE_DART=1
fi
if have bun; then # host calls need Bun >= 1.4 (threadsafe JSCallback)
  if bun -e 'process.exit(Bun.semver.order(Bun.version, "1.4.0") < 0 ? 1 : 0)'; then
    (cd "$ROOT/tests/ts" && bun install --frozen-lockfile >/dev/null && bun x tsc --noEmit)
    HAVE_BUN=1
  else echo "SKIP TypeScript: Bun $(bun --version) < 1.4"; fi
fi
export WABI_FAKE_CORES_DIR="$B/fake"

declare -a SUMMARY=()
run() { # name, command...
  local name="$1"; shift
  if "$@" > "$B/last.log" 2>&1; then SUMMARY+=("PASS  $name"); else SUMMARY+=("FAIL  $name"); cat "$B/last.log"; fi
  { grep -aE "[0-9]+ (passed|pass$|checks)|All tests passed|tests passed" "$B/last.log" || true; } | tail -1 | sed 's/^/        /'
}

for core in ${CORES:-rust rust-http go csharp}; do
  lib="${LIB[$core]:-}"
  [ -n "$lib" ] || { SUMMARY+=("SKIP  $core core (toolchain missing)"); continue; }
  export WABI_TEST_ENDPOINT="${ENDPOINT[$core]}"
  echo "== core: $core, backend: $WABI_TEST_ENDPOINT"
  run "$core / C ABI conformance" "$B/conformance" "$lib"
  [ $HAVE_HOST_C = 1 ] && run "$core / C host-call protocol" env LD_LIBRARY_PATH="$(dirname "$lib")" "$B/host_calls"
  [ $HAVE_DOTNET = 1 ] && run "$core / C# SDK tests" env WABI_NATIVE_LIB="$lib" dotnet "$B/csharp-tests/Demo.Client.Tests.dll"
  [ $HAVE_JAVA = 1 ] && run "$core / Java SDK tests (Panama)" env WABI_NATIVE_LIB="$lib" \
      java -Dwabi.binding=panama --enable-native-access=ALL-UNNAMED -cp "$B/java/demo-client.jar:$B/java/tests" DemoClientTests
  [ $HAVE_JAVA = 1 ] && [ -f "$B/java/libdemo_client_jni.so" ] && run "$core / Java SDK tests (JNI, Android path)" env WABI_NATIVE_LIB="$lib" \
      java -Dwabi.binding=jni -Dcom.demo.client.runtime.jniLib="$B/java/libdemo_client_jni.so" -cp "$B/java/demo-client.jar:$B/java/tests" DemoClientTests
  [ $HAVE_DART = 1 ] && run "$core / Dart SDK tests" bash -c "cd '$ROOT/tests/dart' && WABI_NATIVE_LIB='$lib' '$DART' test -r expanded"
  [ $HAVE_BUN = 1 ] && run "$core / TypeScript SDK tests (Bun)" bash -c "cd '$ROOT/tests/ts' && WABI_NATIVE_LIB='$lib' bun test"
done

echo; echo "== compatibility matrix"
printf '%s\n' "${SUMMARY[@]}"
! printf '%s\n' "${SUMMARY[@]}" | grep -q '^FAIL'
