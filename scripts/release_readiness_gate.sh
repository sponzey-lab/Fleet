#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
INCLUDE_MANUAL=0
INCLUDE_REGISTRY=0
VERIFY_MANUAL_REBOOT=0
RELEASE_PUBLIC_KEY="docs/release-signing-public.pem"

usage() {
  echo "usage: $0 [--include-manual] [--include-registry] [--verify-manual-reboot] [--release-public-key <path>]" >&2
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --include-manual)
      INCLUDE_MANUAL=1
      shift
      ;;
    --include-registry)
      INCLUDE_REGISTRY=1
      shift
      ;;
    --verify-manual-reboot)
      VERIFY_MANUAL_REBOOT=1
      shift
      ;;
    --release-public-key)
      shift
      if [ "$#" -eq 0 ]; then
        usage
        exit 1
      fi
      RELEASE_PUBLIC_KEY="$1"
      shift
      ;;
    *)
      usage
      exit 1
      ;;
  esac
done

cd "$REPO_ROOT"

run() {
  echo "==> $*"
  "$@"
}

require_linux_root() {
  if [ "$(uname -s)" != "Linux" ]; then
    echo "manual release checks require Linux; current host is $(uname -s)" >&2
    exit 1
  fi
  if [ "$(id -u)" -ne 0 ]; then
    echo "manual release checks require root. Re-run with sudo." >&2
    exit 1
  fi
}

run cargo fmt --all --check
run cargo test --workspace
run cargo clippy --workspace --all-targets -- -D warnings
run npm test --workspace @sponzey/fleet
run npm test --workspace web-admin
run npm run typecheck --workspace web-admin
run ./scripts/npm_local_pack_smoke.sh
run ./scripts/npm_platform_local_install_smoke.sh
run ./scripts/npm_demo_smoke.sh
run ./scripts/smoke_mvp.sh
run ./scripts/smoke_immediate_dispatch.sh
run ./scripts/smoke_remote_tls_loopback.sh
run ./scripts/signature_verification_smoke.sh
run sh ./scripts/storage_decision_gate.sh
if [ -f dist/release/SHA256SUMS ]; then
  run ./scripts/verify_standalone_artifacts.sh dist/release
else
  echo "standalone artifact verification skipped: dist/release/SHA256SUMS not found."
fi
if [ -f dist/release/SHA256SUMS.sig ]; then
  if [ ! -f "$RELEASE_PUBLIC_KEY" ]; then
    echo "release signature found but public key not found: $RELEASE_PUBLIC_KEY" >&2
    echo "Pass --release-public-key <path> with the pinned release public key." >&2
    exit 1
  fi
  run ./scripts/verify_release_signature.sh dist/release "$RELEASE_PUBLIC_KEY"
else
  echo "release signature verification skipped: dist/release/SHA256SUMS.sig not found."
fi
if [ -f target/release/fleet ]; then
  run ./scripts/check_linux_glibc_baseline.sh target/release/fleet
else
  echo "glibc baseline check skipped: target/release/fleet not built."
fi
run ./scripts/hardening_audit.sh

if [ "$INCLUDE_REGISTRY" -eq 1 ]; then
  run ./scripts/manual_npm_registry_smoke.sh
else
  echo "registry install check skipped."
  echo "After npm registry publish, run:"
  echo "  ./scripts/release_readiness_gate.sh --include-registry"
fi

if [ "$VERIFY_MANUAL_REBOOT" -eq 1 ]; then
  require_linux_root
  run ./scripts/manual_systemd_reboot_smoke.sh verify
elif [ "$INCLUDE_MANUAL" -eq 1 ]; then
  require_linux_root
  run ./scripts/manual_linux_nginx_runbook_smoke.sh
  run ./scripts/manual_systemd_reboot_smoke.sh install
  echo "manual systemd install completed. Reboot, then run:"
  echo "  sudo ./scripts/release_readiness_gate.sh --verify-manual-reboot"
else
  echo "manual checks skipped."
  echo "To include destructive Linux checks, run:"
  echo "  sudo ./scripts/release_readiness_gate.sh --include-manual"
  echo "After reboot from the manual install phase, run:"
  echo "  sudo ./scripts/release_readiness_gate.sh --verify-manual-reboot"
fi

echo "release readiness gate ok"
