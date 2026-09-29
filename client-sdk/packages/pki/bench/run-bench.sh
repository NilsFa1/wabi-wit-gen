#!/usr/bin/env bash
# Performance of gemLibPki (TUC_PKI_018 verify) used
#   1. directly in a Java application on the JVM (JIT) — the "native" Java way,
#   2. from a Java app through the SDK against the GraalVM native library (Panama, JNI),
#   3. from a C# app through the SDK against the same native library,
# plus the raw cost of one async ABI round trip (C, no SDK runtime).
# Needs: packages/pki/run-tests.sh run once (builds core jar, native lib, SDK jar).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
B="$ROOT/build"
LIB="${LIB:-$B/pki/libgematik_pki.so}"
export PKI_FIXTURES="$ROOT/packages/pki/tests/fixtures"
C="$ROOT/packages/pki/core-java"
CP="$C/target/gematik-pki-core-0.1.0.jar:$(cat "$C/target/classpath.txt")"
J="$B/java-gematik-pki"
mkdir -p "$B/bench/jvm" "$B/bench/sdk"
javac --release 21 -cp "$CP" -d "$B/bench/jvm" Bench.java JvmDirect.java
javac --release 22 -cp "$J/gematik-pki.jar" -d "$B/bench/sdk" Bench.java SdkJava.java
gcc -O2 -std=gnu11 -I"$ROOT/abi" -o "$B/bench/abi_roundtrip" abi_roundtrip.c -ldl -lpthread
quiet() { grep -v -e JAVA_TOOL -e SLF4J; }

java -Dstdout.encoding=UTF-8 -cp "$B/bench/jvm:$CP" JvmDirect 2>&1 | quiet
[ -n "${GRAALVM_HOME:-}" ] && "$GRAALVM_HOME/bin/java" -Dstdout.encoding=UTF-8 -cp "$B/bench/jvm:$CP" JvmDirect 2>&1 | quiet
for binding in panama jni; do
  ACME_NATIVE_LIB="$LIB" java -Dstdout.encoding=UTF-8 -Dacme.binding=$binding -Dacme.jni.lib="$J/libacme_jni.so" \
    --enable-native-access=ALL-UNNAMED -cp "$J/gematik-pki.jar:$B/bench/sdk" SdkJava 2>&1 | quiet
done
ACME_NATIVE_LIB="$LIB" dotnet run --project csharp -c Release 2>&1 | quiet
"$B/bench/abi_roundtrip" pki "$LIB" "$PKI_FIXTURES/tsl.xml" 2>&1 | quiet
