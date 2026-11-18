#!/usr/bin/env bash
# Makes a built .app self-contained.
#
# The release builds inside the nix dev shell, whose linker resolves libraries
# through /nix/store. A Mach-O that records those paths only runs on a machine
# that has nix, which is exactly the person this app is for. So: rewrite every
# /nix/store dependency to the system library of the same name, and carry the
# rest inside Contents/Frameworks.
set -euo pipefail

APP="${1:?usage: macos-fix-deps <path/to/App.app>}"
FRAMEWORKS="$APP/Contents/Frameworks"
mkdir -p "$FRAMEWORKS"

# Rewriting a system library keeps the app small: dyld reads these from the
# shared cache, so the file does not even need to exist on disk.
system_lib() {
  case "$(basename "$1")" in
    libiconv*|libc++*|libz*|libbz2*|libxml2*|libresolv*|libSystem*|libobjc*|libsqlite3*)
      echo "/usr/lib/$(basename "$1")"
      ;;
    *) echo "" ;;
  esac
}

patch() {
  local binary="$1"
  local deps
  deps=$(otool -L "$binary" | tail -n +2 | awk '{print $1}' | grep '^/nix/store/' || true)
  for dep in $deps; do
    local replacement
    replacement=$(system_lib "$dep")
    if [[ -n "$replacement" ]]; then
      echo "  $(basename "$binary"): $dep → $replacement"
      install_name_tool -change "$dep" "$replacement" "$binary"
    else
      local name
      name=$(basename "$dep")
      echo "  $(basename "$binary"): $dep → @executable_path/../Frameworks/$name"
      if [[ ! -f "$FRAMEWORKS/$name" ]]; then
        cp "$dep" "$FRAMEWORKS/$name"
        chmod u+w "$FRAMEWORKS/$name"
        install_name_tool -id "@executable_path/../Frameworks/$name" "$FRAMEWORKS/$name"
        # The copy may itself point into the store.
        for inner in $(otool -L "$FRAMEWORKS/$name" | tail -n +2 | awk '{print $1}' | grep '^/nix/store/' || true); do
          local inner_replacement
          inner_replacement=$(system_lib "$inner")
          if [[ -n "$inner_replacement" ]]; then
            install_name_tool -change "$inner" "$inner_replacement" "$FRAMEWORKS/$name"
          fi
        done
        codesign --force --sign - "$FRAMEWORKS/$name" >/dev/null 2>&1 || true
      fi
      install_name_tool -change "$dep" "@executable_path/../Frameworks/$name" "$binary"
    fi
  done

  # An rpath into the store is dead on the user's machine too.
  while read -r rpath; do
    [[ -z "$rpath" ]] && continue
    echo "  $(basename "$binary"): dropping rpath $rpath"
    install_name_tool -delete_rpath "$rpath" "$binary" 2>/dev/null || true
  done < <(otool -l "$binary" | awk '/LC_RPATH/ {found=1} found && /path / {print $2; found=0}' | grep '^/nix/store/' || true)
}

echo "patching Mach-O files under $APP"
while IFS= read -r -d '' binary; do
  if file "$binary" | grep -q 'Mach-O'; then
    patch "$binary"
  fi
done < <(find "$APP/Contents/MacOS" "$FRAMEWORKS" -type f -perm -u+x -print0)

leftovers=$(find "$APP/Contents/MacOS" "$FRAMEWORKS" -type f -perm -u+x -print0 |
  xargs -0 -I{} sh -c 'otool -L "{}" 2>/dev/null | tail -n +2 | awk "{print \$1}"' |
  grep '^/nix/store/' || true)
if [[ -n "$leftovers" ]]; then
  echo "still depends on the nix store:" >&2
  echo "$leftovers" >&2
  exit 1
fi

# Re-sign everything together: install_name_tool invalidates the old signature.
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
echo "dependencies clean, signature ad-hoc and valid"
