#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
BIN="$REPO_ROOT/target/debug/fleet"

# Web Admin assets are embedded into the Rust binary with include_str!, so local
# controller runs must rebuild to pick up HTML/CSS/JS changes.
cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p fleet-cli

if [ "$#" -eq 0 ]; then
  set -- --host 127.0.0.1 --port 7700 --data-dir .fleet --external-url http://127.0.0.1:7700
fi

case "${1:-}" in
  controller)
    shift
    case "${1:-}" in
      ""|-h|--help)
        exec "$BIN" controller "$@"
        ;;
      init)
        shift
        exec "$BIN" controller init "$@"
        ;;
      start)
        shift
        ;;
    esac
    ;;
  init)
    shift
    exec "$BIN" controller init "$@"
    ;;
  start)
    shift
    ;;
  agent)
    if [ "${2:-}" = "-h" ] || [ "${2:-}" = "--help" ]; then
      exec "$BIN" agent --help
    fi
    cat >&2 <<EOF
ERROR: run_controller.sh wraps only 'fleet controller start'

For agent commands, use:

  ./scripts/run_agent.sh --help
  "$BIN" agent --help
EOF
    exit 2
    ;;
esac

for arg in "$@"; do
  case "$arg" in
    -h|--help)
      exec "$BIN" controller start "$@"
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

if [ ! -f "$DATA_DIR/controller/controller_public.key" ] || [ ! -f "$DATA_DIR/controller/controller_private.key" ]; then
  cat >&2 <<EOF
ERROR: controller is not initialized for data dir: $DATA_DIR

Initialize it once before starting the controller:

  "$BIN" controller init --data-dir "$DATA_DIR"

Then start the controller:

  ./scripts/run_controller.sh --host 127.0.0.1 --port 7700 --data-dir "$DATA_DIR" --external-url http://127.0.0.1:7700

Equivalent explicit script form:

  ./scripts/run_controller.sh controller init --data-dir "$DATA_DIR"
  ./scripts/run_controller.sh controller start --host 127.0.0.1 --port 7700 --data-dir "$DATA_DIR" --external-url http://127.0.0.1:7700
EOF
  exit 2
fi

exec "$BIN" controller start "$@"
