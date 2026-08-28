#!/usr/bin/env sh
set -eu

cargo build -p fleet-cli
BIN="./target/debug/fleet"
SMOKE_TMPDIR="${TMPDIR:-/tmp}"
WORK_DIR="$(mktemp -d "$SMOKE_TMPDIR/sponzey-fleet-smoke.XXXXXX")"
if [ -n "${SPONZEY_MVP_SMOKE_PORT:-}" ]; then
  PORT="$SPONZEY_MVP_SMOKE_PORT"
elif command -v python3 >/dev/null 2>&1; then
  PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()' 2>/dev/null || printf '%s' "$((16000 + ($$ % 10000)))")"
else
  PORT="$((16000 + ($$ % 10000)))"
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
printf '%s\n' "$INIT_OUTPUT"
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
    echo "smoke skipped: loopback server bind is not permitted in this environment"
    exit 0
  fi
  echo "controller did not become healthy" >&2
  exit 1
fi

if ! grep -q "WARNING: insecure HTTP controller URL enabled" "$WORK_DIR/controller.log"; then
  cat "$WORK_DIR/controller.log" >&2
  echo "remote HTTP warning smoke failed: controller warning missing" >&2
  exit 1
fi

TOKEN="$("$BIN" enroll-token create --data-dir "$WORK_DIR" --labels role=web,env=dev)"
"$BIN" agent init \
  --data-dir "$WORK_DIR" \
  --url "$CONTROLLER_URL" \
  --token "$TOKEN" \
  --name web-01 \
  --labels role=web,env=dev

"./scripts/run_agent.sh" \
  --data-dir "$WORK_DIR" \
  --heartbeat-interval-seconds 30 \
  > "$WORK_DIR/agent.log" 2>&1 &
AGENT_PID="$!"

i=0
while [ "$i" -lt 50 ]; do
  AGENTS_API="$(curl -fsS -H "Authorization: Bearer $ADMIN_TOKEN" "$CONTROLLER_URL/api/agents" 2>/dev/null || true)"
  case "$AGENTS_API" in
    *'"id":"agent-web-01"'*'"status":"online"'*) break ;;
    *'"id":"agent-web-01"'*'"status":"degraded"'*) break ;;
  esac
  i=$((i + 1))
  sleep 0.1
done

if [ "$i" -eq 50 ]; then
  cat "$WORK_DIR/agent.log" >&2
  echo "agents API smoke failed: $AGENTS_API" >&2
  exit 1
fi

if ! grep -q "WARNING: insecure HTTP controller URL enabled" "$WORK_DIR/agent.log"; then
  cat "$WORK_DIR/agent.log" >&2
  echo "remote HTTP warning smoke failed: agent warning missing" >&2
  exit 1
fi

curl -fsS \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"job_id":"job-remote-1","target_agent_ids":[],"selector":"role=web","program":"echo","args":["remote-ok"],"timeout_seconds":30,"confirmed_high_risk":false,"confirmed_by":"smoke-admin","expires_in_seconds":60,"nonce_prefix":"remote-smoke"}' \
  "$CONTROLLER_URL/api/jobs/command" >/dev/null

i=0
REMOTE_STATUS=""
REMOTE_JOB_API=""
REMOTE_OUTPUT_API=""
while [ "$i" -lt 50 ]; do
  REMOTE_JOB_API="$(curl -fsS -H "Authorization: Bearer $ADMIN_TOKEN" "$CONTROLLER_URL/api/jobs/job-remote-1" 2>/dev/null || true)"
  REMOTE_OUTPUT_API="$(curl -fsS -H "Authorization: Bearer $ADMIN_TOKEN" "$CONTROLLER_URL/api/jobs/job-remote-1/output" 2>/dev/null || true)"
  case "$REMOTE_JOB_API" in
    *'"status":"success"'*) REMOTE_STATUS="success" ;;
    *) REMOTE_STATUS="" ;;
  esac
  if [ "$REMOTE_STATUS" = "success" ]; then
    case "$REMOTE_OUTPUT_API" in
      *remote-ok*) break ;;
    esac
  fi
  i=$((i + 1))
  sleep 0.1
done

if [ "$REMOTE_STATUS" != "success" ]; then
  cat "$WORK_DIR/controller.log" >&2
  cat "$WORK_DIR/agent.log" >&2
  echo "remote command smoke failed: job=$REMOTE_JOB_API output=$REMOTE_OUTPUT_API" >&2
  exit 1
fi
case "$REMOTE_OUTPUT_API" in
  *remote-ok*) ;;
  *)
    echo "remote output API smoke failed: $REMOTE_OUTPUT_API" >&2
    exit 1
    ;;
esac

FACTS_API="$(curl -fsS -H "Authorization: Bearer $ADMIN_TOKEN" "$CONTROLLER_URL/api/agents/agent-web-01/facts/latest")"
METRICS_API="$(curl -fsS -H "Authorization: Bearer $ADMIN_TOKEN" "$CONTROLLER_URL/api/agents/agent-web-01/metrics/latest")"
case "$FACTS_API" in
  *'"agent_id":"agent-web-01"'*'"os"'*) ;;
  *)
    echo "facts API smoke failed: $FACTS_API" >&2
    exit 1
    ;;
esac
case "$METRICS_API" in
  *'"agent_id":"agent-web-01"'*'"cpu"'*) ;;
  *)
    echo "metrics API smoke failed: $METRICS_API" >&2
    exit 1
    ;;
esac

PATCH_LABELS_API="$(curl -fsS \
  -X PATCH \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"labels":[{"key":"role","value":"api"},{"key":"env","value":"dev"}]}' \
  "$CONTROLLER_URL/api/agents/agent-web-01/labels")"
case "$PATCH_LABELS_API" in
  *'"key":"role"'*'"value":"api"'*) ;;
  *)
    echo "agent label patch smoke failed: $PATCH_LABELS_API" >&2
    exit 1
    ;;
esac

"$BIN" agents list --data-dir "$WORK_DIR"
"$BIN" run --selector role=api --confirm-risk uptime
"$BIN" facts web-01
"$BIN" metrics web-01
"$BIN" drift check --policy examples/policies/nginx-running.yml
"$BIN" apply examples/runbooks/nginx-basic.yml

RUNBOOK_REQUEST="$WORK_DIR/runbook-request.json"
cat > "$RUNBOOK_REQUEST" <<'JSON'
{
  "job_id": "job-runbook-1",
  "target_agent_ids": [],
  "selector": "role=api",
  "runbook_document": "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook\nmetadata:\n  name: nginx-basic\nspec:\n  targets:\n    selector: role=web\n  tasks:\n    - id: nginx-package\n      package:\n        name: nginx\n        state: present\n",
  "timeout_seconds": 30,
  "confirmed_high_risk": true,
  "confirmed_by": "smoke-admin",
  "expires_in_seconds": 60,
  "nonce_prefix": "runbook-smoke"
}
JSON
RUNBOOK_CREATE_API="$(curl -fsS \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary "@$RUNBOOK_REQUEST" \
  "$CONTROLLER_URL/api/jobs/runbook")"
RUNBOOK_APPROVAL_ID="$(printf '%s\n' "$RUNBOOK_CREATE_API" | sed -n 's/.*"approval_request_id":"\([^"]*\)".*/\1/p')"
if [ -z "$RUNBOOK_APPROVAL_ID" ]; then
  echo "runbook approval id was not returned: $RUNBOOK_CREATE_API" >&2
  exit 1
fi
curl -fsS \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"reason":"approved by smoke test"}' \
  "$CONTROLLER_URL/api/approvals/$RUNBOOK_APPROVAL_ID/approve" >/dev/null

i=0
RUNBOOK_STATUS=""
RUNBOOK_JOB_API=""
RUNBOOK_OUTPUT_API=""
while [ "$i" -lt 50 ]; do
  RUNBOOK_JOB_API="$(curl -fsS -H "Authorization: Bearer $ADMIN_TOKEN" "$CONTROLLER_URL/api/jobs/job-runbook-1" 2>/dev/null || true)"
  RUNBOOK_OUTPUT_API="$(curl -fsS -H "Authorization: Bearer $ADMIN_TOKEN" "$CONTROLLER_URL/api/jobs/job-runbook-1/output" 2>/dev/null || true)"
  case "$RUNBOOK_JOB_API:$RUNBOOK_OUTPUT_API" in
    *'"status":"failed"'*'"no supported Linux package manager detected"'*) RUNBOOK_STATUS="failed"; break ;;
    *'"status":"success"'*) RUNBOOK_STATUS="success"; break ;;
  esac
  i=$((i + 1))
  sleep 0.1
done
case "$RUNBOOK_JOB_API:$RUNBOOK_OUTPUT_API" in
  *'"status":"failed"'*'"no supported Linux package manager detected"'*) ;;
  *'"status":"success"'*) ;;
  *)
    cat "$WORK_DIR/controller.log" >&2
    cat "$WORK_DIR/agent.log" >&2
    echo "runbook signed dispatch smoke failed: job=$RUNBOOK_JOB_API output=$RUNBOOK_OUTPUT_API" >&2
    exit 1
    ;;
esac

"$BIN" retention cleanup --data-dir "$WORK_DIR" --older-than-days 1 --dry-run

echo "smoke ok"
