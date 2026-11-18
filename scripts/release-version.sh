#!/usr/bin/env bash
# Prints the version a build should carry: the release tag, without the "v".
#
# `tauri.conf.json` used to hold the number, so every dmg came out as 0.1.1 while the
# release said v0.0.11. The tag is the thing that names a release, so the tag wins:
#
#   HULAHOOP_VERSION=v0.0.12     what CI passes, straight from github.ref_name
#   git tag --points-at HEAD     a local build from the tagged commit
#   git describe --tags          a local build from a commit after a tag
#   tauri.conf.json              a build from a tree with no tags at all
set -euo pipefail
cd "$(dirname "$0")/.."

version="${HULAHOOP_VERSION:-}"
if [[ -z "$version" ]]; then
  version=$(git tag --points-at HEAD 2>/dev/null | grep -E '^v?[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true)
fi
if [[ -z "$version" ]]; then
  version=$(git describe --tags --abbrev=0 2>/dev/null | grep -E '^v?[0-9]+\.[0-9]+\.[0-9]+$' || true)
fi
version="${version#v}"
if [[ -z "$version" ]]; then
  version=$(jq -r .version apps/desktop/src-tauri/tauri.conf.json)
fi
printf '%s\n' "$version"
