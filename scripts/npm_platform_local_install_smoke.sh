#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
WORK_DIR="${TMPDIR:-/tmp}/fleet-npm-platform-smoke-$$"

cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT INT TERM

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
  darwin) PLATFORM_OS="darwin" ;;
  linux) PLATFORM_OS="linux" ;;
  *)
    echo "unsupported smoke OS: $OS" >&2
    exit 1
    ;;
esac

case "$ARCH" in
  arm64|aarch64) PLATFORM_ARCH="arm64" ;;
  x86_64|amd64) PLATFORM_ARCH="x64" ;;
  *)
    echo "unsupported smoke arch: $ARCH" >&2
    exit 1
    ;;
esac

PLATFORM_DIR="$REPO_ROOT/npm/fleet-$PLATFORM_OS-$PLATFORM_ARCH"
STAGED_PLATFORM_DIR="$WORK_DIR/fleet-$PLATFORM_OS-$PLATFORM_ARCH"
PLATFORM_BIN="$STAGED_PLATFORM_DIR/bin/fleet"

cd "$REPO_ROOT"
cargo build -p fleet-cli >/dev/null

mkdir -p "$STAGED_PLATFORM_DIR/bin" "$WORK_DIR"
cp "$PLATFORM_DIR/package.json" "$STAGED_PLATFORM_DIR/package.json"
cp "$PLATFORM_DIR/README.md" "$STAGED_PLATFORM_DIR/README.md"
cp "$REPO_ROOT/target/debug/fleet" "$PLATFORM_BIN"
chmod +x "$PLATFORM_BIN"

(
  cd "$STAGED_PLATFORM_DIR"
  NPM_CONFIG_CACHE="$WORK_DIR/npm-cache" npm pack --pack-destination "$WORK_DIR" >/dev/null
)
(
  cd "$REPO_ROOT/npm/fleet"
  NPM_CONFIG_CACHE="$WORK_DIR/npm-cache" npm pack --pack-destination "$WORK_DIR" >/dev/null
)

WRAPPER_TARBALL="$(find "$WORK_DIR" -name 'sponzey-fleet-*.tgz' ! -name '*darwin*' ! -name '*linux*' -print -quit)"
PLATFORM_TARBALL="$(find "$WORK_DIR" -name "sponzey-fleet-$PLATFORM_OS-$PLATFORM_ARCH-*.tgz" -print -quit)"

if [ -z "$WRAPPER_TARBALL" ] || [ -z "$PLATFORM_TARBALL" ]; then
  echo "missing wrapper or platform tarball" >&2
  exit 1
fi

INSTALL_ROOT="$WORK_DIR/prefix/lib/node_modules/@sponzey"
mkdir -p "$INSTALL_ROOT" "$WORK_DIR/prefix/bin" "$WORK_DIR/extract-wrapper" "$WORK_DIR/extract-platform"
tar -xzf "$PLATFORM_TARBALL" -C "$WORK_DIR/extract-platform"
tar -xzf "$WRAPPER_TARBALL" -C "$WORK_DIR/extract-wrapper"
mv "$WORK_DIR/extract-platform/package" "$INSTALL_ROOT/fleet-$PLATFORM_OS-$PLATFORM_ARCH"
mv "$WORK_DIR/extract-wrapper/package" "$INSTALL_ROOT/fleet"
ln -sf "../lib/node_modules/@sponzey/fleet/bin/fleet" "$WORK_DIR/prefix/bin/fleet"
PREFIX_ROOT="$(cd "$WORK_DIR/prefix" && pwd -P)"
PREFIX_BIN="$(cd "$WORK_DIR/prefix/bin" && pwd -P)"

NPM_CONFIG_PREFIX="$PREFIX_ROOT" \
npm_config_prefix="$PREFIX_ROOT" \
npm_config_global=true \
PATH="$PREFIX_BIN:$PATH" \
  node "$INSTALL_ROOT/fleet/scripts/postinstall.js" >/dev/null

FLEET_NPM_OS="$PLATFORM_OS" \
FLEET_NPM_ARCH="$PLATFORM_ARCH" \
  "$WORK_DIR/prefix/bin/fleet" --help >/dev/null
PATH="$WORK_DIR/prefix/bin:$PATH" \
FLEET_NPM_OS="$PLATFORM_OS" \
FLEET_NPM_ARCH="$PLATFORM_ARCH" \
  fleet --help >/dev/null

echo "npm platform local install smoke ok: $PLATFORM_OS-$PLATFORM_ARCH"
