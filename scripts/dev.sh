#!/usr/bin/env bash
# Builds the workspace and runs the server and the runner against a scratch data
# directory, on the host and port you name:
#
#   just dev                     this machine only, port 9000
#   just dev 0.0.0.0             every interface, so a Mac on the LAN can connect
#   just dev 192.168.1.19 9100   one address, one port
#
# It is the debug build, so `just dev` after a fix is the whole loop: no release,
# no download. Ctrl-C stops both processes.
set -euo pipefail

cd "$(dirname "$0")/.."

# Outside the dev shell there is no C compiler, and three crates here build C, so a
# `just dev` from a shell without direnv would fail with a cc-rs error. Re-enter the
# shell instead of asking the user to.
if ! command -v cc >/dev/null && command -v nix >/dev/null; then
  exec nix develop --command "$0" "$@"
fi

HOST="${1:-127.0.0.1}"
PORT="${2:-9000}"
DATA_DIR="${HULAHOOP_DATA:-$PWD/.hulahoop-dev}"
mkdir -p "$DATA_DIR"

cargo build --quiet -p hulahoop-server -p hulahoop-runner -p hulahoop-cli
BIN="$PWD/target/debug"

echo "building done; starting on $HOST:$PORT (data in $DATA_DIR)"
# exec: one Ctrl-C, and the CLI already stops the server when the runner goes.
export HULAHOOP_DATA="$DATA_DIR"
exec "$BIN/hulahoop" up --listen "$HOST" --port "$PORT" --data-dir "$DATA_DIR"
