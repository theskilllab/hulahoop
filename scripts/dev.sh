#!/usr/bin/env bash
# Runs the server and the runner against a scratch data directory so a
# developer can open the app or the browser build without touching real data.
set -euo pipefail

cd "$(dirname "$0")/.."
DATA_DIR="${HULAHOOP_DATA:-$PWD/.hulahoop-dev}"
mkdir -p "$DATA_DIR"

cargo build --quiet -p hulahoop-server -p hulahoop-runner -p hulahoop-cli
SERVER_BIN="$PWD/target/debug/hulahoop-server"
RUNNER_BIN="$PWD/target/debug/hulahoop-runner"

HULAHOOP_DATA="$DATA_DIR" "$SERVER_BIN" --data-dir "$DATA_DIR" &
SERVER_PID=$!
trap 'kill $SERVER_PID ${RUNNER_PID:-} 2>/dev/null || true' EXIT

for _ in $(seq 1 100); do
  [[ -f "$DATA_DIR/connection.json" ]] && break
  sleep 0.1
done

if [[ ! -f "$DATA_DIR/connection.json" ]]; then
  echo "the server did not start; see the output above" >&2
  exit 1
fi

HULAHOOP_DATA="$DATA_DIR" "$RUNNER_BIN" &
RUNNER_PID=$!

echo "data dir: $DATA_DIR"
jq -r '"server on http://127.0.0.1:\(.port) — token \(.token[0:8])…"' "$DATA_DIR/connection.json"
wait $SERVER_PID
