#!/usr/bin/env bash
# Everything that can run on a Linux dev box / CI runner.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
echo "== generator: unit tests + generated code up to date"
(cd generator && cargo test --quiet)
(cd generator && cargo run --quiet -- --root .. --check)
echo "== header compiles as C99 and C++11"
gcc -std=c99 -pedantic -Wall -Werror -fsyntax-only -x c abi/acme_client.h
g++ -std=c++11 -Wall -Werror -fsyntax-only -x c++ abi/acme_client.h
echo "== rust core: unit + integration tests (incl. real WebSocket)"
(cd core-rust && cargo test --quiet)
echo "== compatibility matrix"
tests/compatibility/run.sh
