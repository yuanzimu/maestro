#!/bin/bash
# build.sh — 构建 Maestro.app 并打 DMG
# 用法：./build.sh [版本号，默认 0.1.0]
# 前置：swift（系统自带）+ Rust release 二进制（../../target/release/，可选）
set -euo pipefail
cd "$(dirname "$0")"

VERSION="${1:-0.1.0}"
ARCH="$(uname -m)"           # arm64 / x86_64
ROOT="$(cd ../.. && pwd)"    # maestro 仓库根
APP="Maestro"
DIST="dist"

echo "==> 1/6 swift build (release)"
swift build -c release

echo "==> 2/6 图标"
if [ ! -f AppIcon.icns ]; then
  swift make_icon.swift
fi

echo "==> 3/6 组装 .app"
STAGE="$DIST/stage"
APPROOT="$STAGE/$APP.app"
rm -rf "$STAGE"
mkdir -p "$APPROOT/Contents/MacOS" "$APPROOT/Contents/Resources/bin"

cp .build/release/MaestroGui "$APPROOT/Contents/MacOS/$APP"
cp AppIcon.icns "$APPROOT/Contents/Resources/AppIcon.icns"

cat > "$APPROOT/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>              <string>Maestro</string>
    <key>CFBundleDisplayName</key>       <string>Maestro</string>
    <key>CFBundleExecutable</key>        <string>$APP</string>
    <key>CFBundleIdentifier</key>        <string>io.github.yuanzimu.maestro.gui</string>
    <key>CFBundlePackageType</key>       <string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key>           <string>1</string>
    <key>CFBundleIconFile</key>          <string>AppIcon</string>
    <key>LSMinimumSystemVersion</key>    <string>13.0</string>
    <key>NSHighResolutionCapable</key>   <true/>
    <key>LSApplicationCategoryType</key> <string>public.app-category.developer-tools</string>
    <key>NSPrincipalClass</key>          <string>NSApplication</string>
</dict>
</plist>
EOF

echo "==> 4/6 内嵌 Rust 二进制（daemon / rounder / cli）"
for b in maestro-daemon maestro-rounder maestro; do
  if [ -f "$ROOT/target/release/$b" ]; then
    cp "$ROOT/target/release/$b" "$APPROOT/Contents/Resources/bin/"
  else
    echo "    ⚠️  $ROOT/target/release/$b 不存在（App 将回退 PATH 搜索）"
  fi
done

echo "==> 5/6 ad-hoc 签名"
codesign --force --sign - "$APPROOT"

echo "==> 6/6 打 DMG"
DMG="$DIST/Maestro-$VERSION-macos-$ARCH.dmg"
hdiutil create -volname "Maestro" -srcfolder "$STAGE" -ov -format UDZO "$DMG" -quiet

echo ""
echo "✅ 构建完成：$DMG"
echo "   本地验证：open $STAGE/$APP.app"
codesign -dv "$APPROOT" 2>&1 | head -2 || true
