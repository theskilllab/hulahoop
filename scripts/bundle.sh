#!/usr/bin/env bash
# Builds the server, the runner, and the desktop bundle for one target triple.
set -euo pipefail

TARGET="${1:?usage: bundle <target-triple>}"
cd "$(dirname "$0")/.."

cargo build --release --target "$TARGET" -p hulahoop-server -p hulahoop-runner

mkdir -p apps/desktop/src-tauri/binaries
cp "target/$TARGET/release/hulahoop-server" "apps/desktop/src-tauri/binaries/hulahoop-server-$TARGET"
cp "target/$TARGET/release/hulahoop-runner" "apps/desktop/src-tauri/binaries/hulahoop-runner-$TARGET"

pnpm --prefix apps/desktop install --frozen-lockfile
pnpm --prefix apps/desktop tauri build --target "$TARGET" --bundles app,dmg
