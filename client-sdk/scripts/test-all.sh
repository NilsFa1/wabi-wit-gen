#!/usr/bin/env bash
# Everything that can run on a Linux dev box / CI runner.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
echo "== generator: unit tests + generated code up to date"
(cd generator && cargo test --quiet)
(cd generator && cargo run --quiet -- --root .. --check)
echo "== headers compile as C99 and C++11 (generic ABI + generated per-package header)"
for h in abi/wabi_abi.h generated/c/*.h; do
  gcc -std=c99 -pedantic -Wall -Werror -fsyntax-only -Iabi -x c "$h"
  g++ -std=c++11 -Wall -Werror -fsyntax-only -Iabi -x c++ "$h"
done
printf '#define WABI_PREFIX a_b\n#include "wabi_abi_functions.h"\n#undef WABI_PREFIX\n#define WABI_PREFIX c_d\n#include "wabi_abi_functions.h"\n' \
  | gcc -std=c99 -pedantic -Wall -Werror -fsyntax-only -Iabi -x c -
echo "== generic runtimes: cancellation and stream regression tests"
(cd runtime/rust && cargo test --quiet)
(cd runtime/go && go test -race ./...)
echo "== rust core: unit + integration tests (incl. real WebSocket)"
(cd core-rust && cargo test --quiet)
echo "== compatibility matrix"
tests/compatibility/run.sh
echo "== gematik:zero PoC: host calls from every SDK"
packages/zero/run-tests.sh
echo "== gematik:pki PoC: gemLibPki (Java) as a GraalVM native library"
packages/pki/run-tests.sh
echo "== two generated SDKs in one process (runtime per package)"
tests/multi-sdk/run.sh
