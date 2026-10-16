#!/usr/bin/env bash
# Regenerate the hicolor theme icons from assets/app-icon/shotcraft.svg (the master).
#
# Needs resvg (cargo install resvg). The PNGs are committed, so builds and packaging never need it.
# The 256 px icon is also compiled into the app as its window and tray icon.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
APP_ID=io.github.mmartinez.shotcraft

command -v resvg >/dev/null || { echo "error: resvg not found (cargo install resvg)" >&2; exit 1; }

for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  resvg -w "$s" -h "$s" "$DIR/shotcraft.svg" "$DIR/hicolor/${s}x${s}/apps/$APP_ID.png" </dev/null
done
echo "icons written to $DIR/hicolor"
