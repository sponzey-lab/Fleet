#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
BIN="$REPO_ROOT/target/debug/fleet"

if [ ! -x "$BIN" ]; then
  cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p fleet-cli
fi

if [ "$#" -eq 0 ]; then
  set -- --data-dir .fleet
fi

case "${1:-}" in
  agent)
    shift
    if [ "${1:-}" = "start" ]; then
      shift
    fi
    ;;
  start)
    shift
    ;;
  controller)
    if [ "${2:-}" = "-h" ] || [ "${2:-}" = "--help" ]; then
      exec "$BIN" controller --help
    fi
    cat >&2 <<EOF
ERROR: run_agent.sh wraps only 'fleet agent start'

For controller commands, use:

  ./scripts/run_controller.sh --help
  "$BIN" controller --help
EOF
    exit 2
    ;;
esac

for arg in "$@"; do
  case "$arg" in
    -h|--help)
      exec "$BIN" agent start "$@"
      ;;
  esac
done

DATA_DIR=".fleet"
PREV=
for arg in "$@"; do
  if [ "$PREV" = "--data-dir" ]; then
    DATA_DIR="$arg"
    PREV=
    continue
  fi
  case "$arg" in
    --data-dir=*)
      DATA_DIR="${arg#--data-dir=}"
      ;;
    --data-dir)
      PREV="--data-dir"
      ;;
    *)
      PREV=
      ;;
  esac
done

if [ ! -f "$DATA_DIR/agent/agent.conf" ]; then
  cat >&2 <<EOF
ERROR: agent is not enrolled for data dir: $DATA_DIR

Start a controller, create an enrollment token, then enroll the agent with the same data dir:

  "$BIN" controller init --data-dir "$DATA_DIR"
  ./scripts/run_controller.sh --host 127.0.0.1 --port 7700 --data-dir "$DATA_DIR" --external-url http://127.0.0.1:7700
  TOKEN=\$("$BIN" enroll-token create --data-dir "$DATA_DIR" --labels role=web,env=dev)
  "$BIN" agent init --data-dir "$DATA_DIR" --url http://127.0.0.1:7700 --token "\$TOKEN" --name web-01 --labels role=web,env=dev
  ./scripts/run_agent.sh --data-dir "$DATA_DIR"

For a one-command local demo, run:

  "$BIN" demo
EOF
  exit 2
fi

exec "$BIN" agent start "$@"
