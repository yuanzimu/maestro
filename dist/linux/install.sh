#!/usr/bin/env bash
# Maestro Linux 安装（R58）：装到 ~/.local（无 root，单用户）
# 用法：./install.sh [--prefix PATH]
set -euo pipefail

PREFIX="${HOME}/.local"
if [ "${1:-}" = "--prefix" ] && [ -n "${2:-}" ]; then PREFIX="$2"; fi

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST_BIN="$PREFIX/bin"

# 依赖检查：maestro-daemon 内嵌 sqlite（bundled），需系统 libc（glibc ≥ 2.31 即可）
fail() { echo "安装失败: $*" >&2; exit 1; }

for b in maestro maestro-daemon maestro-rounder; do
    [ -f "$HERE/bin/$b" ] || fail "包不完整：缺 bin/$b"
done

mkdir -p "$DEST_BIN"
for b in maestro maestro-daemon maestro-rounder; do
    install -m 0755 "$HERE/bin/$b" "$DEST_BIN/$b"
done

# manifest 留档（卸载/doctor 对账用）
mkdir -p "$PREFIX/share/maestro"
cp "$HERE/share/MANIFEST" "$PREFIX/share/maestro/MANIFEST"

# PATH 提示（幂等）
case ":$PATH:" in
    *":$DEST_BIN:"*) ;;
    *)
        cat <<EOF

提示：$DEST_BIN 不在 PATH。加一行到 ~/.bashrc：

    export PATH="$DEST_BIN:\$PATH"

EOF
        ;;
esac

echo "安装完成："
"$DEST_BIN/maestro" --version | head -1 || true
echo "  maestro / maestro-daemon / maestro-rounder → $DEST_BIN"
echo "首次使用：maestro doctor"
