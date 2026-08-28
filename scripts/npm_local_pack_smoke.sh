#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
PACK_DIR="${TMPDIR:-/tmp}/sponzey-fleet-npm-pack-$$"

cleanup() {
  rm -rf "$PACK_DIR"
}
trap cleanup EXIT INT TERM

cd "$REPO_ROOT"
cargo build -p fleet-cli >/dev/null

mkdir -p "$PACK_DIR"
(
  cd npm/fleet
  NPM_CONFIG_CACHE="$PACK_DIR/npm-cache" npm pack --pack-destination "$PACK_DIR" >/dev/null
)

TARBALL="$(find "$PACK_DIR" -name '*.tgz' -print -quit)"
if [ -z "$TARBALL" ]; then
  echo "npm pack did not produce a tarball" >&2
  exit 1
fi

mkdir -p "$PACK_DIR/package"
tar -xzf "$TARBALL" -C "$PACK_DIR/package" --strip-components 1

FLEET_BIN="$REPO_ROOT/target/debug/fleet" "$PACK_DIR/package/bin/fleet" --help >/dev/null

echo "npm local pack smoke ok"
