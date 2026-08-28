#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$REPO_ROOT/dist/npm"
BUILD_PROFILE="release"

usage() {
  cat >&2 <<EOF
usage: $0 [--out-dir <directory>] [--profile release|debug]

Builds the Rust fleet binary for the current OS/architecture and stages the
matching npm platform package under <directory>/fleet-<os>-<arch>.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --out-dir)
      if [ "$#" -lt 2 ]; then
        usage
        exit 2
      fi
      OUT_DIR="$2"
      shift 2
      ;;
    --profile)
      if [ "$#" -lt 2 ]; then
        usage
        exit 2
      fi
      BUILD_PROFILE="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
  darwin) PLATFORM_OS="darwin" ;;
  linux) PLATFORM_OS="linux" ;;
  *)
    echo "unsupported staging OS: $OS" >&2
    exit 1
    ;;
esac

case "$ARCH" in
  arm64|aarch64) PLATFORM_ARCH="arm64" ;;
  x86_64|amd64) PLATFORM_ARCH="x64" ;;
  *)
    echo "unsupported staging arch: $ARCH" >&2
    exit 1
    ;;
esac

case "$BUILD_PROFILE" in
  release)
    CARGO_PROFILE_ARG="--release"
    BINARY_PATH="$REPO_ROOT/target/release/fleet"
    ;;
  debug)
    CARGO_PROFILE_ARG=""
    BINARY_PATH="$REPO_ROOT/target/debug/fleet"
    ;;
  *)
    echo "unsupported build profile: $BUILD_PROFILE" >&2
    exit 2
    ;;
esac

PLATFORM_PACKAGE="fleet-$PLATFORM_OS-$PLATFORM_ARCH"
PLATFORM_SOURCE_DIR="$REPO_ROOT/npm/$PLATFORM_PACKAGE"
STAGED_PLATFORM_DIR="$OUT_DIR/$PLATFORM_PACKAGE"

if [ ! -f "$PLATFORM_SOURCE_DIR/package.json" ]; then
  echo "missing platform package source: $PLATFORM_SOURCE_DIR/package.json" >&2
  exit 1
fi

cd "$REPO_ROOT"
cargo build $CARGO_PROFILE_ARG -p fleet-cli

rm -rf "$STAGED_PLATFORM_DIR"
mkdir -p "$STAGED_PLATFORM_DIR/bin"
cp "$PLATFORM_SOURCE_DIR/package.json" "$STAGED_PLATFORM_DIR/package.json"
cp "$PLATFORM_SOURCE_DIR/README.md" "$STAGED_PLATFORM_DIR/README.md"
cp "$BINARY_PATH" "$STAGED_PLATFORM_DIR/bin/fleet"
chmod +x "$STAGED_PLATFORM_DIR/bin/fleet"

echo "staged @sponzey/$PLATFORM_PACKAGE at $STAGED_PLATFORM_DIR"
