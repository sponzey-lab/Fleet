#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
WORK_DIR="${TMPDIR:-/tmp}/sponzey-fleet-npm-publish-$$"
DRY_RUN=0
TAG="${NPM_PUBLISH_TAG:-latest}"

usage() {
  cat >&2 <<EOF
usage: $0 [--dry-run] [--tag <npm-dist-tag>]

Publishes the current OS/architecture package first, then publishes @sponzey/fleet.

Authentication:
  - use an existing npm login, or
  - set NPM_TOKEN, or
  - set SPONZEY_NPM_TOKEN_FILE to a file containing either a raw token or NPM_TOKEN=<token>.

2FA:
  - set NPM_CONFIG_OTP when npm requires a one-time password.

Examples:
  $0 --dry-run
  NPM_TOKEN=... $0
  SPONZEY_NPM_TOKEN_FILE=token.md $0
  NPM_CONFIG_OTP=123456 $0
EOF
}

cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT INT TERM

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --tag)
      if [ "$#" -lt 2 ]; then
        usage
        exit 2
      fi
      TAG="$2"
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
    echo "unsupported publish OS: $OS" >&2
    exit 1
    ;;
esac

case "$ARCH" in
  arm64|aarch64) PLATFORM_ARCH="arm64" ;;
  x86_64|amd64) PLATFORM_ARCH="x64" ;;
  *)
    echo "unsupported publish arch: $ARCH" >&2
    exit 1
    ;;
esac

PLATFORM_PACKAGE="fleet-$PLATFORM_OS-$PLATFORM_ARCH"
PLATFORM_SOURCE_DIR="$REPO_ROOT/npm/$PLATFORM_PACKAGE"
STAGED_PLATFORM_DIR="$WORK_DIR/$PLATFORM_PACKAGE"
NPM_ARGS="--access public --tag $TAG"

if [ "$DRY_RUN" -eq 1 ]; then
  NPM_ARGS="$NPM_ARGS --dry-run"
fi

if [ ! -f "$PLATFORM_SOURCE_DIR/package.json" ]; then
  echo "missing platform package source: $PLATFORM_SOURCE_DIR/package.json" >&2
  exit 1
fi

mkdir -p "$WORK_DIR/npm-cache" "$STAGED_PLATFORM_DIR/bin"
export NPM_CONFIG_CACHE="$WORK_DIR/npm-cache"

if [ -n "${NPM_TOKEN:-}" ]; then
  NPMRC="$WORK_DIR/.npmrc"
  printf '//registry.npmjs.org/:_authToken=%s\n' "$NPM_TOKEN" > "$NPMRC"
  chmod 600 "$NPMRC"
  export NPM_CONFIG_USERCONFIG="$NPMRC"
elif [ -n "${SPONZEY_NPM_TOKEN_FILE:-}" ]; then
  if [ ! -f "$SPONZEY_NPM_TOKEN_FILE" ]; then
    echo "SPONZEY_NPM_TOKEN_FILE does not exist: $SPONZEY_NPM_TOKEN_FILE" >&2
    exit 1
  fi
  TOKEN="$(
    sed -n \
      -e 's/^[[:space:]]*NPM_TOKEN[[:space:]]*=[[:space:]]*//p' \
      -e 's/^.*_authToken[[:space:]]*=[[:space:]]*//p' \
      "$SPONZEY_NPM_TOKEN_FILE" \
      | sed -n '/^$/!{/^#/!p;}' \
      | head -n 1
  )"
  if [ -z "$TOKEN" ]; then
    TOKEN="$(
      sed -n 's/^.*\(npm_[A-Za-z0-9_=-][A-Za-z0-9_=-]*\).*$/\1/p' "$SPONZEY_NPM_TOKEN_FILE" \
        | head -n 1
    )"
  fi
  if [ -z "$TOKEN" ]; then
    TOKEN="$(
      sed -n 's/^[[:space:]]*//p' "$SPONZEY_NPM_TOKEN_FILE" \
        | sed -n '/^$/!{/^#/!{/^```/!p;};}' \
        | head -n 1
    )"
  fi
  if [ -z "$TOKEN" ]; then
    echo "SPONZEY_NPM_TOKEN_FILE did not contain a token" >&2
    exit 1
  fi
  NPMRC="$WORK_DIR/.npmrc"
  printf '//registry.npmjs.org/:_authToken=%s\n' "$TOKEN" > "$NPMRC"
  chmod 600 "$NPMRC"
  export NPM_CONFIG_USERCONFIG="$NPMRC"
fi

if [ "$DRY_RUN" -eq 0 ]; then
  if ! npm whoami >/dev/null 2>&1; then
    cat >&2 <<EOF
npm authentication failed.

Run one of these before publishing:

  npm login
  NPM_TOKEN=<valid-publish-token> $0
  SPONZEY_NPM_TOKEN_FILE=token.md $0

The token must have publish access to the @sponzey scope. If @sponzey is an npm
organization, the npm account behind the token must be a member with publish
permission. If the scope does not exist yet, create it in npm before publishing.
EOF
    exit 1
  fi
fi

cd "$REPO_ROOT"
cargo build --release -p fleet-cli

cp "$PLATFORM_SOURCE_DIR/package.json" "$STAGED_PLATFORM_DIR/package.json"
cp "$PLATFORM_SOURCE_DIR/README.md" "$STAGED_PLATFORM_DIR/README.md"
cp "$REPO_ROOT/target/release/fleet" "$STAGED_PLATFORM_DIR/bin/fleet"
chmod +x "$STAGED_PLATFORM_DIR/bin/fleet"

echo "publishing staged platform package: @sponzey/$PLATFORM_PACKAGE"
npm publish "$STAGED_PLATFORM_DIR" $NPM_ARGS

echo "publishing wrapper package: @sponzey/fleet"
npm publish "$REPO_ROOT/npm/fleet" $NPM_ARGS

if [ "$DRY_RUN" -eq 1 ]; then
  echo "npm publish dry-run ok for @sponzey/$PLATFORM_PACKAGE and @sponzey/fleet"
else
  echo "npm publish ok for @sponzey/$PLATFORM_PACKAGE and @sponzey/fleet"
  echo "verify with: ./scripts/manual_npm_registry_smoke.sh"
fi
