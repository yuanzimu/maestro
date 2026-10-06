#!/bin/bash
# build.sh — 构建 universal Maestro.app（arm64 + x86_64）并打 DMG
#
# Sprint C 平台面 mac C2 签名链（无凭据部分）：
#   C2-1 Bundle ID 统一 com.yuanzimu.maestro
#   C2-2 hardened runtime + entitlements（Maestro.entitlements）
#   C2-3 sidecar（daemon/rounder/cli）逐二进制签名（由内向外）
#   C2-4 universal lipo（Swift 双架构 + Rust 三件套双架构合并）
#   C2-5/6 notarytool 公证 + DMG 签名：凭据未到位时降级跳过并显式提示
#
# 用法：./build.sh [版本号，默认 0.3.0]
# 可选环境变量：
#   MAESTRO_CODESIGN_IDENTITY  签名身份（默认 "-" ad-hoc；
#                               有 Developer ID 时设 "Developer ID Application: ..."）
#   MACOS_NOTARY_PROFILE        notarytool keychain profile（存在即公证并 staple）
set -euo pipefail
cd "$(dirname "$0")"

VERSION="${1:-0.3.0}"
ROOT="$(cd ../.. && pwd)"          # maestro 仓库根
APP="Maestro"
DIST="dist"
STAGE="$DIST/stage"
APPROOT="$STAGE/$APP.app"

IDENTITY="${MAESTRO_CODESIGN_IDENTITY:--}"
ARM_TRIPLE="aarch64-apple-darwin"
X86_TRIPLE="x86_64-apple-darwin"

# cargo 可能不在默认 PATH
export PATH="$HOME/.cargo/bin:$PATH"

echo "==> 1/7 校验 Rust target 已装（universal 需要双架构）"
for t in "$ARM_TRIPLE" "$X86_TRIPLE"; do
  if ! rustup target list --installed | grep -qx "$t"; then
    echo "    缺少 target ${t}，正在安装…"
    RUSTUP_DIST_SERVER="${RUSTUP_DIST_SERVER:-https://mirrors.ustc.edu.cn/rust-static}" \
      rustup target add "$t"
  fi
done

echo "==> 2/7 Swift universal 构建 ($ARM_TRIPLE + $X86_TRIPLE)"
swift build -c release --arch arm64 --arch x86_64

echo "==> 3/7 Rust sidecar 双架构构建（daemon / rounder / cli）"
for t in "$ARM_TRIPLE" "$X86_TRIPLE"; do
  ( cd "$ROOT" && cargo build --release --target "$t" \
      --bin maestro-daemon --bin maestro-rounder --bin maestro )
done

echo "==> 4/7 组装 .app"
rm -rf "$STAGE"
mkdir -p "$APPROOT/Contents/MacOS" "$APPROOT/Contents/Resources/bin"

# 多架构产物路径随构建系统不同：原生 SwiftPM 多架构用 .build/apple/Products/Release/，
# Xcode build system 用 .build/out/Products/Release/ —— 自动探测
MAESTRO_GUI_BIN=""
for cand in \
  ".build/apple/Products/Release/MaestroGui" \
  ".build/out/Products/Release/MaestroGui"; do
  if [ -f "$cand" ]; then MAESTRO_GUI_BIN="$cand"; break; fi
done
if [ -z "$MAESTRO_GUI_BIN" ]; then
  echo "未找到 universal MaestroGui 产物"; exit 1
fi
echo "    使用主程序：$MAESTRO_GUI_BIN"
cp "$MAESTRO_GUI_BIN" "$APPROOT/Contents/MacOS/$APP"
cp AppIcon.icns "$APPROOT/Contents/Resources/AppIcon.icns"

# sidecar：lipo 合并双架构为 universal
for b in maestro-daemon maestro-rounder maestro; do
  lipo -create \
    "$ROOT/target/$ARM_TRIPLE/release/$b" \
    "$ROOT/target/$X86_TRIPLE/release/$b" \
    -output "$APPROOT/Contents/Resources/bin/$b"
done

cat > "$APPROOT/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>              <string>Maestro</string>
    <key>CFBundleDisplayName</key>       <string>Maestro</string>
    <key>CFBundleExecutable</key>        <string>$APP</string>
    <key>CFBundleIdentifier</key>        <string>com.yuanzimu.maestro</string>
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

echo "==> 5/7 签名：先 sidecar（由内向外），再主程序与外层 bundle"
echo "    identity = ${IDENTITY}"
for b in maestro-daemon maestro-rounder maestro; do
  codesign --force --sign "$IDENTITY" --options runtime \
    "$APPROOT/Contents/Resources/bin/$b"
done
codesign --force --sign "$IDENTITY" --options runtime \
  --entitlements Maestro.entitlements "$APPROOT/Contents/MacOS/$APP"
codesign --force --sign "$IDENTITY" --options runtime \
  --entitlements Maestro.entitlements "$APPROOT"

echo "==> 6/7 校验签名 + 架构"
codesign -dv "$APPROOT" 2>&1 | head -3 || true
lipo -archs "$APPROOT/Contents/MacOS/$APP"
for b in maestro-daemon maestro-rounder maestro; do
  echo -n "    $b: "; lipo -archs "$APPROOT/Contents/Resources/bin/$b"
done

echo "==> 7/7 打 DMG"
DMG="$DIST/Maestro-$VERSION-macos-universal.dmg"
hdiutil create -volname "Maestro" -srcfolder "$STAGE" -ov -format UDZO "$DMG" -quiet

# C2-5/6 公证 + DMG 签名：凭据未到位则降级（显式提示，不伪装已公证）
if [ -n "${MACOS_NOTARY_PROFILE:-}" ]; then
  echo "==> 提交 notarytool 公证 (profile=${MACOS_NOTARY_PROFILE})"
  xcrun notarytool submit "$DMG" --keychain-profile "$MACOS_NOTARY_PROFILE" --wait
  xcrun stapler staple "$APPROOT"
  # C2-6：公证后再对 DMG 签名（可选）
  codesign --force --sign "$IDENTITY" "$DMG"
else
  echo "    ⚠️  未配置 MACOS_NOTARY_PROFILE → 跳过公证/DMG 签名（ad-hoc 构建，仅供本机测试）"
fi

echo ""
echo "✅ 构建完成：$DMG"
echo "   本地验证：open $STAGE/$APP.app"
