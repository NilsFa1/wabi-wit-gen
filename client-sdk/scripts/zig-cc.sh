#!/usr/bin/env bash
# Zig as a drop-in cross C compiler/linker for cargo: ZIG_TARGET=aarch64-linux-gnu
if command -v zig >/dev/null 2>&1; then Z=(zig); else Z=(python3 -m ziglang); fi
args=()
for a in "$@"; do
  case "$a" in
    --target=*|-Wl,--as-needed|-Wl,-Bdynamic|-Wl,-Bstatic) ;; # rustc flags zig cc rejects/doesn't need
    -Wl,--fix-cortex-a53-843419) ;; # aarch64 erratum workaround rustc adds; zig's lld rejects the flag
    *) args+=("$a") ;;
  esac
done
exec "${Z[@]}" cc -target "${ZIG_TARGET:?set ZIG_TARGET}" "${args[@]}"
