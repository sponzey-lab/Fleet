#!/usr/bin/env sh
set -eu

if [ -z "${SPONZEY_TLS_SMOKE_REEXEC:-}" ]; then
  export SPONZEY_TLS_SMOKE_REEXEC=1
  export SPONZEY_KEEP_SMOKE="${SPONZEY_KEEP_SMOKE:-0}"
  exec "$0" "$@"
fi

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
BIN="$REPO_ROOT/target/debug/sponzey"
SMOKE_TMPDIR="${SPONZEY_SMOKE_TMPDIR:-/private/tmp}"
if [ ! -d "$SMOKE_TMPDIR" ]; then
  SMOKE_TMPDIR="${TMPDIR:-/tmp}"
fi
WORK_DIR="$(mktemp -d "$SMOKE_TMPDIR/sponzey-fleet-tls-smoke.XXXXXX")"
if [ -n "${SPONZEY_TLS_SMOKE_PORT:-}" ]; then
  PORT="$SPONZEY_TLS_SMOKE_PORT"
elif command -v python3 >/dev/null 2>&1; then
  PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()' 2>/dev/null || printf '%s' "$((18000 + ($$ % 10000)))")"
else
  PORT="$((18000 + ($$ % 10000)))"
fi
CERT="$WORK_DIR/tls-cert.pem"
KEY="$WORK_DIR/tls-key.pem"
OPENSSL_CONFIG="$WORK_DIR/openssl.cnf"
CONTROLLER_LOG="$WORK_DIR/controller.log"
AGENT_LOG="$WORK_DIR/agent.log"
CONTROLLER_PID=""
AGENT_PID=""
CLEANED=""

cleanup() {
  if [ -n "$AGENT_PID" ] && kill -0 "$AGENT_PID" 2>/dev/null; then
    kill "$AGENT_PID" 2>/dev/null || true
    wait "$AGENT_PID" 2>/dev/null || true
  fi
  if [ -n "$CONTROLLER_PID" ] && kill -0 "$CONTROLLER_PID" 2>/dev/null; then
    kill "$CONTROLLER_PID" 2>/dev/null || true
    wait "$CONTROLLER_PID" 2>/dev/null || true
  fi
  if [ "${SPONZEY_KEEP_SMOKE:-0}" = "1" ] || [ -z "$CLEANED" ]; then
    echo "kept smoke dir: $WORK_DIR"
  fi
}
trap cleanup EXIT INT TERM

command -v openssl >/dev/null 2>&1 || {
  echo "openssl is required for TLS smoke" >&2
  exit 2
}

cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p fleet-cli

cat > "$OPENSSL_CONFIG" <<'EOF'
[req]
distinguished_name = dn
x509_extensions = v3_req
prompt = no

[dn]
CN = localhost

[v3_req]
subjectAltName = @alt_names

[alt_names]
DNS.1 = localhost
IP.1 = 127.0.0.1
EOF

openssl req \
  -x509 \
  -newkey rsa:2048 \
  -nodes \
  -keyout "$KEY" \
  -out "$CERT" \
  -days 1 \
  -config "$OPENSSL_CONFIG" \
  -extensions v3_req >/dev/null 2>&1
chmod 600 "$KEY"

INIT_OUTPUT="$("$BIN" controller init --data-dir "$WORK_DIR")"
ADMIN_TOKEN="$(printf '%s\n' "$INIT_OUTPUT" | sed -n 's/^admin token: //p')"
"$BIN" controller start \
  --host 127.0.0.1 \
  --port "$PORT" \
  --data-dir "$WORK_DIR" \
  --external-url "https://localhost:$PORT" \
  --tls-cert "$CERT" \
  --tls-key "$KEY" >"$CONTROLLER_LOG" 2>&1 &
CONTROLLER_PID="$!"

i=0
until curl --cacert "$CERT" -fsS "https://localhost:$PORT/healthz" >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -eq 100 ]; then
    echo "controller did not become healthy over HTTPS" >&2
    cat "$CONTROLLER_LOG" >&2 || true
    if grep -q "Operation not permitted (os error 1)" "$CONTROLLER_LOG"; then
      echo "smoke skipped: loopback HTTPS listener is not permitted in this environment"
      exit 0
    fi
    exit 1
  fi
  sleep 0.05
done

TOKEN="$("$BIN" enroll-token create --data-dir "$WORK_DIR" --labels role=web,env=tls)"
"$BIN" agent init \
  --data-dir "$WORK_DIR" \
  --url "https://localhost:$PORT" \
  --tls-ca-cert "$CERT" \
  --token "$TOKEN" \
  --name web-tls-01 \
  --labels role=web,env=tls
"$BIN" agent start --data-dir "$WORK_DIR" --once >"$AGENT_LOG" 2>&1 &
AGENT_PID="$!"

i=0
AGENTS_API=""
while [ "$i" -lt 100 ]; do
  AGENTS_API="$(curl --cacert "$CERT" -fsS -H "Authorization: Bearer $ADMIN_TOKEN" "https://localhost:$PORT/api/agents" 2>/dev/null || true)"
  case "$AGENTS_API" in
    *'"id":"agent-web-tls-01"'*'"status":"online"'*) break ;;
    *'"id":"agent-web-tls-01"'*'"status":"degraded"'*) break ;;
  esac
  i=$((i + 1))
  sleep 0.05
done

if [ "$i" -eq 100 ]; then
  cat "$CONTROLLER_LOG" >&2 || true
  cat "$AGENT_LOG" >&2 || true
  echo "TLS agent connection smoke failed: $AGENTS_API" >&2
  exit 1
fi

kill "$CONTROLLER_PID" 2>/dev/null || true
wait "$CONTROLLER_PID" 2>/dev/null || true
CONTROLLER_PID=""
kill "$AGENT_PID" 2>/dev/null || true
wait "$AGENT_PID" 2>/dev/null || true
AGENT_PID=""
if [ "${SPONZEY_KEEP_SMOKE:-0}" != "1" ]; then
  rm -rf "$WORK_DIR"
  CLEANED="1"
fi

echo "remote TLS loopback smoke ok"
