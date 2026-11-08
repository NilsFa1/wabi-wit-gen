#!/usr/bin/env bash
# (Re)generates everything under sdk/ and the Go glue in core-go from wit/.
#   ./generate.sh            after changing wit/client.wit
#   ./generate.sh --check    CI: lock file and generated code up to date?
set -euo pipefail
cd "$(dirname "$0")"
cargo run --quiet --manifest-path ../generator/Cargo.toml -- wit --out sdk \
  --target csharp,java,swift,dart,ts --core rust,go=core-go,csharp \
  --api-baseline wit/api-baseline.snapshot "$@"
