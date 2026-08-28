#!/usr/bin/env sh
set -eu

if [ "$(uname -s)" != "Linux" ]; then
  echo "manual nginx runbook smoke requires Linux" >&2
  exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
  echo "manual nginx runbook smoke requires root. Re-run with sudo." >&2
  exit 1
fi

if ! command -v systemctl >/dev/null 2>&1; then
  echo "manual nginx runbook smoke requires systemd/systemctl" >&2
  exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "manual nginx runbook smoke requires curl" >&2
  exit 1
fi

if ! command -v apt-get >/dev/null 2>&1 \
  && ! command -v dnf >/dev/null 2>&1 \
  && ! command -v yum >/dev/null 2>&1 \
  && ! command -v apk >/dev/null 2>&1; then
  echo "manual nginx runbook smoke requires apt-get, dnf, yum, or apk" >&2
  exit 1
fi

BIN="${SPONZEY_BIN:-./target/debug/fleet}"
PORT="${SPONZEY_PORT:-7700}"
WORK_DIR="${TMPDIR:-/tmp}/sponzey-fleet-nginx-runbook-smoke-$$"

cleanup() {
  if [ -n "${CONTROLLER_PID:-}" ]; then
    kill "$CONTROLLER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT INT TERM

cargo build -p fleet-cli
mkdir -p "$WORK_DIR"

INIT_OUTPUT="$("$BIN" controller init --data-dir "$WORK_DIR")"
ADMIN_TOKEN="$(printf '%s\n' "$INIT_OUTPUT" | sed -n 's/^admin token: //p')"
"$BIN" controller start \
  --host 127.0.0.1 \
  --port "$PORT" \
  --data-dir "$WORK_DIR" \
  --external-url "http://127.0.0.1:$PORT" \
  > "$WORK_DIR/controller.log" 2>&1 &
CONTROLLER_PID="$!"

i=0
while [ "$i" -lt 50 ]; do
  if curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; then
    break
  fi
  i=$((i + 1))
  sleep 0.2
done
if [ "$i" -eq 50 ]; then
  cat "$WORK_DIR/controller.log" >&2
  echo "controller did not become healthy" >&2
  exit 1
fi

TOKEN="$("$BIN" enroll-token create --data-dir "$WORK_DIR" --labels role=web,env=manual)"
"$BIN" agent init \
  --data-dir "$WORK_DIR" \
  --url "http://127.0.0.1:$PORT" \
  --token "$TOKEN" \
  --name web-01 \
  --labels role=web,env=manual

REQUEST="$WORK_DIR/runbook-request.json"
cat > "$REQUEST" <<'JSON'
{
  "job_id": "job-nginx-runbook-1",
  "target_agent_ids": [],
  "selector": "role=web",
  "runbook_document": "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook\nmetadata:\n  name: nginx-basic\nspec:\n  targets:\n    selector: role=web\n  tasks:\n    - id: nginx-package\n      package:\n        name: nginx\n        state: present\n    - id: nginx-service\n      service:\n        name: nginx\n        state: started\n        enabled: true\n",
  "timeout_seconds": 180,
  "confirmed_high_risk": true,
  "confirmed_by": "manual-linux-smoke",
  "expires_in_seconds": 300,
  "nonce_prefix": "manual-nginx-runbook"
}
JSON

curl -fsS \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary "@$REQUEST" \
  "http://127.0.0.1:$PORT/api/jobs/runbook" >/dev/null

"$BIN" agent start --data-dir "$WORK_DIR" --once

if ! systemctl is-active nginx.service >/dev/null 2>&1; then
  echo "nginx.service is not active after runbook execution" >&2
  exit 1
fi

echo "manual linux nginx runbook smoke ok"
