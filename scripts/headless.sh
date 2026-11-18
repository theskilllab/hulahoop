#!/usr/bin/env bash
# Packs the server, the runner, and the CLI for one target triple. This is what a
# host needs to run the agents; the desktop app is a separate build.
set -euo pipefail

cd "$(dirname "$0")/.."

# No argument means "this machine's triple", which is the only case a release leg
# builds: the dev shell carries the host's standard library, not a cross one.
HOST=$(rustc -vV | awk '/^host:/ {print $2}')
TARGET="${1:-$HOST}"

if [[ "$TARGET" == "$HOST" ]]; then
  cargo build --release -p hulahoop-server -p hulahoop-runner -p hulahoop-cli
  OUT="target/release"
else
  cargo build --release --target "$TARGET" -p hulahoop-server -p hulahoop-runner -p hulahoop-cli
  OUT="target/$TARGET/release"
fi

mkdir -p dist
tar -czf "dist/hulahoop-$TARGET.tar.gz" -C "$OUT" \
  hulahoop-server hulahoop-runner hulahoop
tar -tzf "dist/hulahoop-$TARGET.tar.gz"
echo "built dist/hulahoop-$TARGET.tar.gz"
