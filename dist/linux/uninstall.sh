#!/usr/bin/env bash
# Maestro Linux 卸载（R58）：删二进制 + manifest（不碰用户数据目录）
set -euo pipefail

PREFIX="${HOME}/.local"
if [ "${1:-}" = "--prefix" ] && [ -n "${2:-}" ]; then PREFIX="$2"; fi

for b in maestro maestro-daemon maestro-rounder; do
    rm -f "$PREFIX/bin/$b"
done
rm -rf "$PREFIX/share/maestro"
echo "已卸载（数据目录 ${MAESTRO_DATA_DIR:-/tmp/maestro} 未动，如需清理请手动删除）"
