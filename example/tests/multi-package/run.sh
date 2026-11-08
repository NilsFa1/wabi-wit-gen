#!/usr/bin/env bash
# Two generated SDKs in one JVM, Panama and JNI: the example (demo:client) and a
# second contract, the generator's test fixture demo:greeter, generated and
# built from scratch here in standalone mode. Needs the example's jar, JNI shim
# and Rust core as built by tests/compatibility/run.sh.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)" # example/
REPO="$(dirname "$ROOT")"
TARGET="${CARGO_TARGET_DIR:-$ROOT/core-rust/target}/release"
J="$ROOT/build/java" M="$ROOT/build/multi-package"
for f in "$J/demo-client.jar" "$J/libdemo_client_jni.so" "$TARGET/libdemo_client.so"; do
  [ -f "$f" ] || { echo "SKIP  two SDKs in one JVM: $f missing (run tests/compatibility/run.sh first)"; exit 0; }
done
rm -rf "$M" && mkdir -p "$M"
cp -r "$REPO/generator/npm/test/fixture" "$M/greeter"
(cd "$REPO/generator" && cargo run --quiet -- "$M/greeter/wit" --out "$M/greeter/sdk" --target java --core rust >/dev/null)
(cd "$M/greeter/core" && cargo build --release --quiet)
GREETER="${CARGO_TARGET_DIR:-$M/greeter/core/target}/release/libdemo_greeter.so"
G="$M/java"
bash "$M/greeter/sdk/java/build.sh" "$G" >/dev/null
javac --release 22 -cp "$J/demo-client.jar:$G/demo-greeter.jar" -d "$M/classes" "$ROOT/tests/multi-package/TwoSdks.java"
java --enable-native-access=ALL-UNNAMED \
  -Dcom.demo.client.runtime.jniLib="$J/libdemo_client_jni.so" \
  -Dcom.demo.greeter.runtime.jniLib="$G/libdemo_greeter_jni.so" \
  -cp "$M/classes:$J/demo-client.jar:$G/demo-greeter.jar" TwoSdks "$TARGET/libdemo_client.so" "$GREETER"
echo "PASS  two SDKs (demo:client + demo:greeter) in one JVM, Panama and JNI"
