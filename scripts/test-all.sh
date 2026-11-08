#!/usr/bin/env bash
# Everything that can run on a Linux dev box / CI runner.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
echo "== generator: unit tests + generated code of the example up to date"
(cd generator && cargo test --quiet)
example/generate.sh --check
echo "== headers compile as C99 and C++11 (generic ABI + generated per-package header)"
for h in abi/wabi_abi.h example/sdk/c/*.h; do
  gcc -std=c99 -pedantic -Wall -Werror -fsyntax-only -Iabi -x c "$h"
  g++ -std=c++11 -Wall -Werror -fsyntax-only -Iabi -x c++ "$h"
done
printf '#define WABI_PREFIX a_b\n#include "wabi_abi_functions.h"\n#undef WABI_PREFIX\n#define WABI_PREFIX c_d\n#include "wabi_abi_functions.h"\n' \
  | gcc -std=c99 -pedantic -Wall -Werror -fsyntax-only -Iabi -x c -
echo "== generic runtimes: cancellation and stream regression tests"
(cd runtime/rust && cargo test --quiet)
(cd runtime/go && go vet ./... && go test -race ./...)
echo "== example cores: unit + integration tests (Rust incl. real HTTP against the mock server)"
(cd example/core-rust && cargo test --quiet --features mock-server)
(cd example/core-go && go vet . && go test .)
echo "== example: compatibility matrix"
example/tests/compatibility/run.sh
echo "== two generated SDKs in one process (runtime per package)"
example/tests/multi-package/run.sh
