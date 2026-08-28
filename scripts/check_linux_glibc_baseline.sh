#!/usr/bin/env sh
set -eu

BIN="${1:-target/release/fleet}"
MAX_GLIBC="${SPONZEY_MAX_GLIBC:-2.35}"

if [ "$(uname -s)" != "Linux" ]; then
  echo "glibc baseline check skipped on non-Linux host: $(uname -s)"
  exit 0
fi

if [ ! -f "$BIN" ]; then
  echo "glibc baseline check requires an existing binary: $BIN" >&2
  exit 1
fi

if ! command -v strings >/dev/null 2>&1; then
  echo "glibc baseline check requires strings" >&2
  exit 1
fi

versions="$(strings "$BIN" | sed -n 's/.*GLIBC_\([0-9][0-9.]*\).*/\1/p' | sort -Vu)"
if [ -z "$versions" ]; then
  echo "glibc baseline check ok: no dynamic GLIBC symbols found"
  exit 0
fi

required="$(printf '%s\n' "$versions" | tail -n 1)"
highest="$(printf '%s\n%s\n' "$MAX_GLIBC" "$required" | sort -V | tail -n 1)"

if [ "$highest" != "$MAX_GLIBC" ]; then
  echo "glibc baseline check failed: $BIN requires GLIBC_$required, max allowed is GLIBC_$MAX_GLIBC" >&2
  echo "Build Linux release binaries on an older baseline such as Ubuntu 22.04 or choose a musl/static target." >&2
  exit 1
fi

echo "glibc baseline check ok: $BIN requires GLIBC_$required <= GLIBC_$MAX_GLIBC"
