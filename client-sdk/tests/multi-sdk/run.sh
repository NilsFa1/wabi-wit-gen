#!/usr/bin/env bash
# Two generated SDKs (gematik:pki, gematik:zero) in one JVM, Panama and JNI.
# Needs the jars, JNI shims and cores built by packages/{pki,zero}/run-tests.sh
# (pki: Go core; zero: Rust core, since two Go runtimes cannot share a process).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
P="$ROOT/build/java-gematik-pki" Z="$ROOT/build/java-gematik-zero" O="$ROOT/build/multi-sdk"
PKI="$ROOT/build/pki/go/libgematik_pki.so"
ZERO="$ROOT/build/zero/rust/libgematik_zero.so"
for f in "$P/gematik-pki.jar" "$Z/gematik-zero.jar" "$PKI" "$ZERO"; do
  [ -f "$f" ] || { echo "SKIP  two SDKs in one JVM: $f missing"; exit 0; }
done
mkdir -p "$O"
javac --release 22 -cp "$P/gematik-pki.jar:$Z/gematik-zero.jar" -d "$O" "$ROOT/tests/multi-sdk/TwoSdks.java"
java --enable-native-access=ALL-UNNAMED \
  -Dcom.gematik.pki.runtime.jniLib="$P/libgematik_pki_jni.so" \
  -Dcom.gematik.zero.runtime.jniLib="$Z/libgematik_zero_jni.so" \
  -cp "$O:$P/gematik-pki.jar:$Z/gematik-zero.jar" TwoSdks "$PKI" "$ZERO"
echo "PASS  two SDKs (gematik:pki + gematik:zero) in one JVM, Panama and JNI"
