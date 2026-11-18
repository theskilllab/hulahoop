#!/usr/bin/env bash
# Builds the desktop bundle for one target triple, with `hulahoop` as its sidecar.
set -euo pipefail

TARGET="${1:?usage: bundle <target-triple>}"
cd "$(dirname "$0")/.."

cargo build --release --target "$TARGET" -p hulahoop-cli

mkdir -p apps/desktop/src-tauri/binaries
cp "target/$TARGET/release/hulahoop" "apps/desktop/src-tauri/binaries/hulahoop-$TARGET"

pnpm --prefix apps/desktop install --frozen-lockfile

if [[ "$TARGET" == *apple-darwin ]]; then
  # The nix shell's linker is convenient and toxic: it records /nix/store paths
  # in the Mach-O, and then the app only starts on a machine with nix. Build the
  # app bundle, strip those paths, re-sign, and make the dmg from the result.
  pnpm --prefix apps/desktop tauri build --target "$TARGET" --bundles app
  BUNDLE="apps/desktop/src-tauri/target/$TARGET/release/bundle"
  APP=$(find "$BUNDLE/macos" -maxdepth 1 -name '*.app' -print -quit)
  scripts/macos-fix-deps.sh "$APP"
  VERSION=$(jq -r .version apps/desktop/src-tauri/tauri.conf.json)
  DMG="$BUNDLE/dmg/Hulahoop_${VERSION}_${TARGET%%-*}.dmg"
  mkdir -p "$(dirname "$DMG")"
  rm -f "$DMG"
  STAGE=$(mktemp -d)
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -volname "Hulahoop" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
  rm -rf "$STAGE"
  echo "built $DMG"
else
  pnpm --prefix apps/desktop tauri build --target "$TARGET" --bundles app
fi
