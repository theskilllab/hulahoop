#!/usr/bin/env bash
# Compile the Tauri shell without bundling it.
#
# The shell is a separate Cargo build (see the workspace `exclude`), so a compile
# error there would otherwise surface first on a Mac release run. Tauri's build
# script refuses to start when the external binaries are missing, so this copies
# debug builds under the names the bundler expects.
set -euo pipefail

cd "$(dirname "$0")/.."

triple="$(rustc -vV | awk '/^host:/{print $2}')"
cargo build --bin hulahoop-server --bin hulahoop-runner

mkdir -p apps/desktop/src-tauri/binaries
cp target/debug/hulahoop-server "apps/desktop/src-tauri/binaries/hulahoop-server-$triple"
cp target/debug/hulahoop-runner "apps/desktop/src-tauri/binaries/hulahoop-runner-$triple"

if [[ -d apps/desktop/dist ]]; then
  (cd apps/desktop/src-tauri && cargo check)
else
  # `tauri.conf.json` points at `../dist`; the frontend build produces it.
  pnpm --prefix apps/desktop build
  (cd apps/desktop/src-tauri && cargo check)
fi

rm -f apps/desktop/src-tauri/binaries/hulahoop-server-"$triple"
rm -f apps/desktop/src-tauri/binaries/hulahoop-runner-"$triple"
echo "shell check ok ($triple)"
