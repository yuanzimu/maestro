#!/usr/bin/env bash
# One-command Linux packaging (B4, Sprint B):
#   1) build + stage sidecars (maestro-daemon / maestro-rounder / mock-cli)
#   2) build AppImage (.deb/.rpm via system libs) through tauri
# Bundle kinds are passed with --bundles (the config's top-level targets
# stays "nsis" for Windows); override kinds as $1 (default all three).
# Usage: ./build-linux.sh [appimage,deb,rpm]
set -euo pipefail

BUNDLES="${1:-appimage,deb,rpm}"
SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP="$(cd "$SCRIPT/.." && pwd)"

echo "== 1/2 prepare sidecars =="
"$SCRIPT/prepare-sidecars.sh"

echo "== 2/2 tauri build --bundles $BUNDLES =="
cd "$DESKTOP"
./node_modules/.bin/tauri build --bundles "$BUNDLES"

echo "== artifacts =="
find "$DESKTOP/src-tauri/target/release/bundle" \
  \( -name "*.AppImage" -o -name "*.deb" -o -name "*.rpm" \) \
  -exec ls -lh {} \;
