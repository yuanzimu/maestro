#!/usr/bin/env bash
# Maestro Linux 安装包构建（R58）：三个二进制 + install/uninstall 脚本 → tar.gz
# 用法：./dist/linux/package.sh [输出目录（默认 dist/linux/out）]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${1:-$ROOT/dist/linux/out}"
VERSION="$(git -C "$ROOT" describe --tags --always 2>/dev/null || echo dev)"
PKG="maestro-$VERSION-linux-x86_64"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "== 构建 release 二进制（workspace）=="
cargo build --release --workspace --manifest-path "$ROOT/Cargo.toml"

BIN="$ROOT/target/release"
mkdir -p "$STAGE/$PKG/bin" "$STAGE/$PKG/share"

echo "== 收集二进制 =="
for b in maestro maestro-daemon maestro-rounder; do
    [ -f "$BIN/$b" ] || { echo "缺少 $BIN/$b" >&2; exit 1; }
    cp "$BIN/$b" "$STAGE/$PKG/bin/"
done
ls -lh "$STAGE/$PKG/bin/"

echo "== 收集安装脚本 =="
for s in install.sh uninstall.sh; do
    [ -f "$ROOT/dist/linux/$s" ] || { echo "缺少 $ROOT/dist/linux/$s" >&2; exit 1; }
    install -m 0755 "$ROOT/dist/linux/$s" "$STAGE/$PKG/"
done

echo "== 生成 manifest（版本 + sha256）=="
{
    echo "version=$VERSION"
    echo "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    for b in maestro maestro-daemon maestro-rounder; do
        echo "sha256_$b=$(sha256sum "$STAGE/$PKG/bin/$b" | cut -d' ' -f1)"
    done
} > "$STAGE/$PKG/share/MANIFEST"

echo "== 打包 =="
mkdir -p "$OUT"
tar -C "$STAGE" -czf "$OUT/$PKG.tar.gz" "$PKG"
sha256sum "$OUT/$PKG.tar.gz" | tee "$OUT/$PKG.tar.gz.sha256"
ls -lh "$OUT/$PKG.tar.gz"
echo "完成: $OUT/$PKG.tar.gz"
