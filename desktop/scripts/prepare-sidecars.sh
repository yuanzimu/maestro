#!/usr/bin/env bash
# Build workspace engine binaries and copy them as Tauri sidecars
# (the "-<triple>" suffix is a hard convention of Tauri externalBin).
# Linux counterpart of prepare-sidecars.ps1. Produces:
#   maestro-daemon-<triple>, maestro-rounder-<triple>, mock-cli-<triple>
# IMPORTANT: build everything first, copy only at the end. Tauri's build
# script validates that ALL externalBin files exist; copying one early
# while another is still missing makes the mock-cli build fail.
# Usage: ./prepare-sidecars.sh [triple] [workspace target dir]
set -euo pipefail

TRIPLE="${1:-x86_64-unknown-linux-gnu}"
WORKSPACE_TARGET="${2:-}"

SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP="$(cd "$SCRIPT/.." && pwd)"
REPO="$(cd "$DESKTOP/.." && pwd)"
BIN="$DESKTOP/src-tauri/bin"
DESKTOP_TARGET="$DESKTOP/target"
mkdir -p "$BIN"

# Resolve engine target dir
if [ -n "$WORKSPACE_TARGET" ]; then
  export CARGO_TARGET_DIR="$WORKSPACE_TARGET"
  ENGINE_REL="$WORKSPACE_TARGET/release"
else
  ENGINE_REL="$REPO/target/release"
fi

echo "== 1/3 place dummy sidecars (Tauri build script requires all present) =="
for name in maestro-daemon maestro-rounder mock-cli; do
  dst="$BIN/$name-$TRIPLE"
  if [ ! -f "$dst" ]; then
    printf '#!/bin/sh\nexit 0\n' > "$dst"
    chmod 755 "$dst"
  fi
done

echo "== 2/3 build engine pair + demo worker =="
cargo build --release -p maestro-daemon --manifest-path "$REPO/Cargo.toml"
(
  cd "$DESKTOP/src-tauri"
  CARGO_TARGET_DIR="$DESKTOP_TARGET" cargo build --release --bin mock-cli
)

echo "== 3/3 overwrite placeholders with real sidecars =="
cp -f "$ENGINE_REL/maestro-daemon" "$BIN/maestro-daemon-$TRIPLE"
cp -f "$ENGINE_REL/maestro-rounder" "$BIN/maestro-rounder-$TRIPLE"
cp -f "$DESKTOP_TARGET/release/mock-cli" "$BIN/mock-cli-$TRIPLE"

chmod 755 "$BIN"/*
echo "sidecars ready in $BIN:"
ls -lh "$BIN"
