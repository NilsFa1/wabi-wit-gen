#!/usr/bin/env bash
# Builds the npm package: the generator as WebAssembly (wasm32-wasip1) next to
# the launcher, version taken from generator/Cargo.toml. Output: nfails-wabi-wit-gen-<v>.tgz
#
#   generator/npm/build.sh            (needs rustup target wasm32-wasip1)
set -euo pipefail
NPM="$(cd "$(dirname "$0")" && pwd)"
GEN="$(dirname "$NPM")"
rustup target add wasm32-wasip1 >/dev/null 2>&1 || true
cargo build --release --target wasm32-wasip1 --manifest-path "$GEN/Cargo.toml"
cp "$GEN/target/wasm32-wasip1/release/wabi-wit-gen.wasm" "$NPM/"
cp "$GEN/../../LICENSE" "$GEN/../../NOTICE" "$NPM/" # Apache-2.0, packed by npm
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$GEN/Cargo.toml" | head -1)
(cd "$NPM" && npm pkg set version="$VERSION" >/dev/null && npm pack --silent)
echo "built $NPM/nfails-wabi-wit-gen-$VERSION.tgz"
