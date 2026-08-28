#!/usr/bin/env sh
set -eu

if [ "$(uname -s)" != "Linux" ]; then
  echo "manual systemd reboot smoke requires Linux" >&2
  exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
  echo "manual systemd reboot smoke requires root. Re-run with sudo." >&2
  exit 1
fi

if ! command -v systemctl >/dev/null 2>&1; then
  echo "manual systemd reboot smoke requires systemd/systemctl" >&2
  exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "manual systemd reboot smoke requires curl" >&2
  exit 1
fi

BIN="${FLEET_BIN:-./target/debug/fleet}"
DATA_DIR="${FLEET_DATA_DIR:-/var/lib/fleet}"
CONTROLLER_URL="${FLEET_CONTROLLER_URL:-http://127.0.0.1:7700}"
MODE="${1:-install}"

case "$MODE" in
  install)
    cargo build -p fleet-cli
    "$BIN" controller init --data-dir "$DATA_DIR" >/dev/null
    TOKEN="$("$BIN" enroll-token create --data-dir "$DATA_DIR" --labels role=local,env=manual)"
    "$BIN" controller install-service --data-dir "$DATA_DIR"
    "$BIN" agent install-service --data-dir "$DATA_DIR"
    "$BIN" controller start-service
    i=0
    while [ "$i" -lt 50 ]; do
      if curl -fsS "$CONTROLLER_URL/healthz" >/dev/null 2>&1; then
        break
      fi
      i=$((i + 1))
      sleep 0.2
    done
    if [ "$i" -eq 50 ]; then
      echo "controller service did not become healthy at $CONTROLLER_URL" >&2
      exit 1
    fi
    "$BIN" agent init \
      --data-dir "$DATA_DIR" \
      --url "$CONTROLLER_URL" \
      --token "$TOKEN" \
      --name local-agent \
      --labels role=local,env=manual
    "$BIN" agent start-service
    systemctl is-enabled fleet-controller.service >/dev/null
    systemctl is-enabled fleet-agent.service >/dev/null
    systemctl is-active fleet-controller.service >/dev/null
    systemctl is-active fleet-agent.service >/dev/null
    echo "services installed and active. Reboot this host, then run:"
    echo "  sudo $0 verify"
    ;;
  verify)
    systemctl is-enabled fleet-controller.service >/dev/null
    systemctl is-enabled fleet-agent.service >/dev/null
    systemctl is-active fleet-controller.service >/dev/null
    systemctl is-active fleet-agent.service >/dev/null
    echo "manual systemd reboot smoke ok"
    ;;
  *)
    echo "usage: $0 [install|verify]" >&2
    exit 1
    ;;
esac
