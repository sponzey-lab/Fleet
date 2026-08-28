#!/usr/bin/env sh
set -eu

cargo build -p fleet-cli

BIN="./target/debug/fleet"
SMOKE_TMPDIR="${TMPDIR:-/tmp}"
WORK_DIR="$(mktemp -d "$SMOKE_TMPDIR/fleet-immediate-dispatch.XXXXXX")"
if [ -n "${FLEET_IMMEDIATE_SMOKE_PORT:-}" ]; then
  PORT="$FLEET_IMMEDIATE_SMOKE_PORT"
elif command -v python3 >/dev/null 2>&1; then
  PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()' 2>/dev/null || printf '%s' "$((17000 + ($$ % 10000)))")"
else
  PORT="$((17000 + ($$ % 10000)))"
fi
CONTROLLER_URL="http://127.0.0.1:$PORT"
CONTROLLER_PID=""
AGENT_PID=""

cleanup() {
  if [ -n "$AGENT_PID" ]; then
    kill "$AGENT_PID" 2>/dev/null || true
  fi
  if [ -n "$CONTROLLER_PID" ]; then
    kill "$CONTROLLER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT INT TERM

INIT_OUTPUT="$("$BIN" controller init --data-dir "$WORK_DIR")"
ADMIN_TOKEN="$(printf '%s\n' "$INIT_OUTPUT" | sed -n 's/^admin token: //p')"

"./scripts/run_controller.sh" \
  --host 127.0.0.1 \
  --port "$PORT" \
  --data-dir "$WORK_DIR" \
  --external-url "$CONTROLLER_URL" \
  > "$WORK_DIR/controller.log" 2>&1 &
CONTROLLER_PID="$!"

i=0
while [ "$i" -lt 50 ]; do
  if curl -fsS "$CONTROLLER_URL/healthz" >/dev/null 2>&1; then
    break
  fi
  i=$((i + 1))
  sleep 0.1
done

if [ "$i" -eq 50 ]; then
  cat "$WORK_DIR/controller.log" >&2
  if grep -q "Operation not permitted (os error 1)" "$WORK_DIR/controller.log"; then
    echo "immediate dispatch smoke skipped: loopback server bind is not permitted in this environment"
    exit 0
  fi
  echo "controller did not become healthy" >&2
  exit 1
fi

TOKEN="$("$BIN" enroll-token create --data-dir "$WORK_DIR" --labels role=web,env=dev)"
"$BIN" agent init \
  --data-dir "$WORK_DIR" \
  --url "$CONTROLLER_URL" \
  --token "$TOKEN" \
  --name web-01 \
  --labels role=web,env=dev

"$BIN" agent start \
  --data-dir "$WORK_DIR" \
  --heartbeat-interval-seconds 30 \
  > "$WORK_DIR/agent.log" 2>&1 &
AGENT_PID="$!"

i=0
while [ "$i" -lt 50 ]; do
  AGENTS_API="$(curl -fsS -H "Authorization: Bearer $ADMIN_TOKEN" "$CONTROLLER_URL/api/agents" 2>/dev/null || true)"
  case "$AGENTS_API" in
    *'"id":"agent-web-01"'*'"status":"online"'*) break ;;
  esac
  i=$((i + 1))
  sleep 0.1
done

if [ "$i" -eq 50 ]; then
  cat "$WORK_DIR/agent.log" >&2
  echo "agent did not become online" >&2
  exit 1
fi

curl -fsS \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"job_id":"job-immediate-1","target_agent_ids":[],"selector":"role=web","program":"echo","args":["immediate-ok"],"timeout_seconds":30,"confirmed_high_risk":false,"confirmed_by":"smoke-admin","expires_in_seconds":60,"nonce_prefix":"immediate-smoke"}' \
  "$CONTROLLER_URL/api/jobs/command" >/dev/null

i=0
OUTPUT_API=""
while [ "$i" -lt 50 ]; do
  OUTPUT_API="$(curl -fsS -H "Authorization: Bearer $ADMIN_TOKEN" "$CONTROLLER_URL/api/jobs/job-immediate-1/output" 2>/dev/null || true)"
  case "$OUTPUT_API" in
    *immediate-ok*) break ;;
  esac
  i=$((i + 1))
  sleep 0.1
done

if [ "$i" -eq 50 ]; then
  cat "$WORK_DIR/controller.log" >&2
  cat "$WORK_DIR/agent.log" >&2
  echo "immediate dispatch smoke failed: output was not visible within 5 seconds" >&2
  echo "$OUTPUT_API" >&2
  exit 1
fi

echo "immediate dispatch smoke ok"
