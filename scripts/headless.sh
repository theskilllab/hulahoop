#!/usr/bin/env bash
# Packs the hulahoop binary for one target triple. One executable covers the
# server, the runner, and the CLI: a host runs `hulahoop up`, the desktop bundles
# the same file as its sidecar.
set -euo pipefail

cd "$(dirname "$0")/.."

# No argument means "this machine's triple", which is the only case a release leg
# builds: the dev shell carries the host's standard library, not a cross one.
HOST=$(rustc -vV | awk '/^host:/ {print $2}')
TARGET="${1:-$HOST}"

if [[ "$TARGET" == "$HOST" ]]; then
  cargo build --release -p hulahoop-cli
  OUT="target/release"
else
  cargo build --release --target "$TARGET" -p hulahoop-cli
  OUT="target/$TARGET/release"
fi

mkdir -p dist
tar -czf "dist/hulahoop-$TARGET.tar.gz" -C "$OUT" hulahoop
tar -tzf "dist/hulahoop-$TARGET.tar.gz"
echo "built dist/hulahoop-$TARGET.tar.gz"
