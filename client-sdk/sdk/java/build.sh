#!/usr/bin/env bash
# Builds the Java SDK (hand-written runtime + generated sources) into
# build/java/acme-client.jar. Requires JDK 22+ (Panama FFM is final since 22).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/build/java"
rm -rf "$OUT/classes" && mkdir -p "$OUT/classes"
javac --release 22 -Xlint:all,-serial,-restricted -Werror -d "$OUT/classes" \
  $(find "$ROOT/runtime/java" "$ROOT/generated/java" -name '*.java')
jar --create --file "$OUT/acme-client.jar" -C "$OUT/classes" .
echo "built $OUT/acme-client.jar"
