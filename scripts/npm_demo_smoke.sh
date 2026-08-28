#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
PACK_DIR="${TMPDIR:-/tmp}/sponzey-fleet-npm-demo-$$"

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

set +e
OUTPUT="$(FLEET_BIN="$REPO_ROOT/target/debug/fleet" "$PACK_DIR/package/bin/fleet" demo 2>&1)"
STATUS=$?
set -e
if [ "$STATUS" -ne 0 ]; then
  case "$OUTPUT" in
    *"Operation not permitted (os error 1)"*)
      printf '%s\n' "npm demo smoke skipped: loopback server bind is not permitted in this environment"
      exit 0
      ;;
    *)
      printf '%s\n' "$OUTPUT" >&2
      exit "$STATUS"
      ;;
  esac
fi
case "$OUTPUT" in
  *"demo controller: http://127.0.0.1:"*"demo command output: demo-ok"*) ;;
  *)
    printf '%s\n' "$OUTPUT" >&2
    echo "demo output did not include expected controller URL and command output" >&2
    exit 1
    ;;
esac

echo "npm demo smoke ok"
