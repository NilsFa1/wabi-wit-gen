#!/usr/bin/env bash
# Tests the npm package as a user gets it: install the tarball from build.sh
# into an empty project, then run it with Node (and Bun, if present):
#   1. --check on the example of this repo (relative paths, several cores),
#   2. output byte-for-byte equal to the native generator,
#   3. usage errors exit non-zero,
#   4. a project from scratch (test/fixture): WIT -> Rust core -> apps.
set -euo pipefail
NPM="$(cd "$(dirname "$0")/.." && pwd)"
GEN="$(dirname "$NPM")"
REPO="$(dirname "$GEN")"
TGZ=$(ls "$NPM"/nfails-wabi-wit-gen-*.tgz | tail -1)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
(cd "$WORK" && npm init -y >/dev/null && npm install --silent "$TGZ")
# The launcher itself (node_modules/.bin holds a shell shim on Windows).
BIN="$WORK/node_modules/@nfails/wabi-wit-gen/bin/wabi-wit-gen.mjs"
cargo build --quiet --manifest-path "$GEN/Cargo.toml"
NATIVE="${CARGO_TARGET_DIR:-$GEN/target}/debug/wabi-wit-gen"
node "$NPM/test/regression.mjs" "$NATIVE" "$BIN"

runners=("node")
command -v bun >/dev/null && runners+=("bun")
for runner in "${runners[@]}"; do
  run() { "$runner" "$BIN" "$@"; }
  echo "== $runner: $(run --version)"
  (cd "$REPO/example" && run wit --out sdk --target csharp,java,swift,dart,ts --core rust,go=core-go,csharp --api-baseline wit/api-baseline.snapshot --check)
  rm -rf "$WORK/out-wasm" "$WORK/out-native" "$WORK/contract"
  mkdir -p "$WORK/contract" && cp "$REPO"/example/wit/{client.wit,abi-ids.lock} "$WORK/contract/"
  (cd "$WORK" && run contract/client.wit --out out-wasm --target csharp,java,swift,dart,ts --core rust >/dev/null)
  (cd "$WORK" && "$NATIVE" contract/client.wit --out out-native --target csharp,java,swift,dart,ts --core rust >/dev/null)
  diff -r "$WORK/out-wasm" "$WORK/out-native"
  echo "output identical to the native generator"
  if run "$WORK/contract" --out "$WORK/x" --target cobol 2>/dev/null; then echo "FAIL: unknown target accepted"; exit 1; fi
  echo "PASS  $runner"
done

# 4. a project from scratch (test/fixture: a second, tiny contract): generate,
#    build the Rust core with the generated build-native.sh, call it from Bun,
#    Dart (build hook, as in Flutter) and .NET (NuGet package) — each where the
#    toolchain is available.
if command -v cargo >/dev/null; then
  EX="$WORK/fixture" && cp -r "$NPM/test/fixture" "$EX" && rm -rf "$EX/sdk" "$EX/wit/abi-ids.lock"
  (cd "$EX" && node "$BIN" wit --out sdk --target ts,dart,csharp --core rust >/dev/null)
  bash "$EX/sdk/core-rust/build-native.sh" "$EX/core" host >/dev/null
  for LIB in "$EX"/sdk/native/*/{libdemo_greeter.so,libdemo_greeter.dylib,demo_greeter.dll}; do [ -f "$LIB" ] && break; done
  [ -f "$LIB" ] || { echo "FAIL fixture: build-native.sh produced no library"; exit 1; }
  echo "PASS  fixture: build-native.sh host -> ${LIB#$EX/}"
  expect() { echo "$2" | grep -q 'Hallo Nils' && echo "$2" | grep -qi 'empty' || { echo "FAIL fixture $1: $2"; exit 1; }; echo "PASS  fixture: $1"; }
  if command -v bun >/dev/null; then
    LIBPATH="$LIB"; command -v cygpath >/dev/null && LIBPATH=$(cygpath -w "$LIB") # Windows: native path for Bun
    expect "TypeScript SDK (Bun)" "$(cd "$EX" && WABI_NATIVE_LIB="$LIBPATH" bun app.ts)"
  fi
  if command -v dart >/dev/null; then # no WABI_NATIVE_LIB: the package's build hook bundles sdk/native
    expect "Dart package (build hook)" "$(cd "$EX/dart_app" && env -u WABI_NATIVE_LIB dart run 2>&1)"
  fi
  if command -v dotnet >/dev/null; then # NuGet package incl. runtimes/<rid>/native
    dotnet pack "$EX/sdk/csharp" -c Release -o "$EX/feed" >/dev/null
    expect "NuGet package (.NET)" "$(cd "$EX/dotnet_app" && env -u WABI_NATIVE_LIB dotnet run 2>&1)"
  fi
fi
