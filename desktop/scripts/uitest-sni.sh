#!/usr/bin/env bash
# uitest-sni.sh —— SNI 托盘可见性自动化测试（C4-1 / B4-3，DEV_PLAN Sprint C）
#
# 两种模式：
#   mock（默认）—— headless：dbus-run-session 隔离总线 + mock StatusNotifierWatcher
#                  + Xvfb 启动 GUI，验证 SNI 协议全链（注册/属性/菜单/点击行为）
#   live        —— 真实桌面会话（GNOME/KDE）直接跑：检查宿主 watcher、注册轮询、
#                  桌面环境识别（GNOME 需 AppIndicator 扩展；KDE 原生），输出可见性证据
#
# 实证背景（2026-10-06 调试 /tmp/sni-debug.sh）：
# - tauri tray-icon 0.25 Linux 后端 = libappindicator（ayatana）：注册传的是
#   **对象路径** /org/ayatana/NotificationItem/...，服务走注册者唯一总线名（:1.x），
#   不是 ksni 形态的 org.kde.StatusNotifierItem-<pid> 服务名 —— 断言需两形态兼容
# - Alt+M：global-hotkey X11 后端（XGrabKey）在 Xvfb 下 xdotool XTEST 注入可触发，
#   toggle 双向验证通过（隐藏→唤起）
# - 窗口可见性判据用 xdotool search --onlyvisible（xwininfo -tree 连 withdrawn
#   窗口也列出，会把「已隐藏」误判成「仍在」）
#
# mock 模式断言集（对应 C4-1 验收「托盘菜单六项可用」的协议面）：
#   1. mock watcher 收到 RegisterStatusNotifierItem（ayatana 路径或 kde 服务名）
#   2. item Category = ApplicationStatus
#   3. dbusmenu GetLayout 非空且含菜单六项（显示窗口/新建任务/收件箱/急停/设置/退出）
#   4. item Activate(0,0)（左键语义）→ 窗口恢复可见
#   5. Alt+M 全局快捷键 toggle：可见→藏→再按→唤
#   6. 降级路径：无 watcher 的裸 dbus session → GUI 启动不崩（优雅降级）
#
# 用法：./uitest-sni.sh [mock|live] [--bin /path/to/maestro-desktop]
# 依赖：python3 + python3-dbus + PyGObject、Xvfb、gdbus、xdotool（mock 模式）
set -euo pipefail

MODE="${1:-mock}"
BIN=""
shift || true
while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN="$2"; shift 2 ;;
    *) shift ;;
  esac
done
BIN="${BIN:-$(cd "$(dirname "$0")/.." && pwd)/src-tauri/target/release/maestro-desktop}"
[ -x "$BIN" ] || { echo "FAIL: 找不到可执行 $BIN（先 cargo build --release 或用 --bin 指定）" >&2; exit 1; }

PASS=0; FAIL=0
# 断言记录（子 bash 与父进程共享；父进程只计数汇总，内容子 bash 已实时输出）
: > /tmp/sni-assertions.$$ || true
LOG=/tmp/sni-assertions.$$

# ---------------------------------------------------------------------------
# 内嵌 mock StatusNotifierWatcher（python3-dbus）
# 记录「sender 路径/服务名」二元组 —— ayatana 形态后续调用需 dest=注册者唯一名
# ---------------------------------------------------------------------------
WATCHER_PY=$(mktemp /tmp/sni-watcher-XXXX.py)
export WATCHER_PY LOG BIN
cat > "$WATCHER_PY" <<'PYEOF'
import sys, dbus, dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

OUT = sys.argv[1]
BUS = 'org.kde.StatusNotifierWatcher'

class Watcher(dbus.service.Object):
    def __init__(self, bus):
        self.items = []      # RegisteredStatusNotifierItems（原样回显注册参数）
        super().__init__(bus, '/StatusNotifierWatcher')
        # 坑：BusName 必须保持引用，否则 GC 后总线名即释放（watcher 消失）
        self.name = dbus.service.BusName(BUS, bus)

    @dbus.service.method(BUS, in_signature='s', out_signature='',
                         sender_keyword='sender')
    def RegisterStatusNotifierItem(self, service, sender=None):
        s = str(service)
        if s not in self.items:
            self.items.append(s)
        # ayatana 形态：s 是对象路径，真正的服务是调用者唯一名（:1.x）
        with open(OUT, 'a') as f:
            f.write(f"{sender}\t{s}\n")
        self.StatusNotifierItemRegistered(s)

    @dbus.service.signal(BUS, signature='s')
    def StatusNotifierItemRegistered(self, s):
        pass

    @dbus.service.method(dbus.PROPERTIES_IFACE, in_signature='ss', out_signature='v')
    def Get(self, iface, name):
        if name == 'RegisteredStatusNotifierItems':
            return dbus.Array(self.items, signature='s')
        if name == 'IsStatusNotifierHostRegistered':
            return dbus.Boolean(True)
        if name == 'ProtocolVersion':
            return dbus.Int32(0)
        raise dbus.exceptions.DBusException(f'no such property {name}')

DBusGMainLoop(set_as_default=True)
Watcher(dbus.SessionBus())
GLib.MainLoop().run()
PYEOF

cleanup() { rm -f "$WATCHER_PY" "$LOG"; pkill -f "$WATCHER_PY" 2>/dev/null || true; }
trap cleanup EXIT

# ---------------------------------------------------------------------------
mock_mode() {
  export SNI_TMP="$(mktemp -d /tmp/sni-test-XXXX)"
  export XDG_DATA_HOME="$SNI_TMP/data"
  mkdir -p "$XDG_DATA_HOME"

  # timeout -k 硬杀防御：GUI/portal 残留偶发卡 dbus-run-session 关总线
  #（TERM 不退则 KILL；顺序效应，单跑可复现通过）
  timeout -k 5 45 dbus-run-session -- bash -e <<'OUTER'
    ok()  { echo "PASS: $*"; echo "PASS: $*" >> "$LOG"; }
    bad() { echo "FAIL: $*" >&2; echo "FAIL: $*" >> "$LOG"; }

    # --- 1. mock watcher + Xvfb + GUI ---
    python3 "$WATCHER_PY" "$SNI_TMP/reg.log" >"$SNI_TMP/watcher.log" 2>&1 &
    Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &
    XVFB_PID=$!
    export DISPLAY=:99
    sleep 1
    # watcher 自检：总线名必须已有 owner（早失败早诊断）
    if gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner org.kde.StatusNotifierWatcher 2>/dev/null \
        | grep -q true; then
      :
    else
      bad "mock watcher 未上线（watcher.log: $(tail -3 "$SNI_TMP/watcher.log" 2>/dev/null)）"
    fi

    "$BIN" >/dev/null 2>"$SNI_TMP/gui.log" &
    GUI_PID=$!
    sleep 6   # GTK init + daemon 拉起 + SNI 注册（libappindicator 探测 watcher）

    # --- 断言 1：watcher 收到注册（ayatana 路径 / kde 服务名两形态兼容）---
    # reg.log 行格式："<sender唯一名>\t<路径或服务名>"
    REG=""
    for i in $(seq 1 20); do
      [ -s "$SNI_TMP/reg.log" ] && REG="$(head -1 "$SNI_TMP/reg.log")" && break
      sleep 1
    done
    SENDER=""; ITEM=""
    if [ -n "$REG" ]; then
      SENDER="${REG%%	*}"
      ITEM="${REG##*	}"
      ok "SNI 注册：sender=$SENDER item=$ITEM"
      # item 调用参数归一化：路径形态（ayatana）→ dest=SENDER；服务名形态（ksni）→ dest=ITEM path=/MenuBar
      if [ "${ITEM#/}" != "$ITEM" ]; then
        DEST="$SENDER"; OBJPATH="$ITEM"
      else
        DEST="$ITEM"; OBJPATH="/MenuBar"
      fi
    else
      bad "SNI 未注册（watcher 20s 无响应）—— gui.log: $(tail -3 "$SNI_TMP/gui.log" 2>/dev/null)"
    fi

    # --- 断言 2：Category = ApplicationStatus ---
    if [ -n "$ITEM" ]; then
      CAT="$(gdbus call --session --dest "$DEST" --object-path "$OBJPATH" \
        --method org.freedesktop.DBus.Properties.Get org.kde.StatusNotifierItem Category 2>&1 || true)"
      case "$CAT" in
        *ApplicationStatus*) ok "Category=ApplicationStatus" ;;
        *) bad "Category 异常: $CAT" ;;
      esac
    fi

    # --- 断言 3：dbusmenu 菜单六项 ---
    # 菜单对象路径 ≠ item 路径：SNI Menu 属性给出（ayatana = item 路径 + "/Menu"）。
    # label 比对走 python（gdbus 文本输出对非 ASCII label 转义，bash grep 不可靠）
    MENUPATH=""
    if [ -n "$ITEM" ]; then
      MENUPATH="$(gdbus call --session --dest "$DEST" --object-path "$OBJPATH" \
        --method org.freedesktop.DBus.Properties.Get org.kde.StatusNotifierItem Menu 2>/dev/null \
        | grep -o "'[^']*'" | tr -d "'")"
      [ -z "$MENUPATH" ] && MENUPATH="$OBJPATH/Menu"
      LABELS="$(python3 - "$DEST" "$MENUPATH" <<'PYDUMP' 2>>"$SNI_TMP/gui.log" || true
import sys, dbus
sender, menupath = sys.argv[1], sys.argv[2]
bus = dbus.SessionBus()
menu = dbus.Interface(bus.get_object(sender, menupath), 'com.canonical.dbusmenu')
_rev, layout = menu.GetLayout(0, -1, [])
def deep(n):
    if isinstance(n, (dbus.Struct, dbus.Array)):
        return [deep(x) for x in n]
    if isinstance(n, dbus.Dictionary):
        return {k: deep(v) for k, v in n.items()}
    return n
def walk(node):
    i, props, children = node
    yield props.get('label', '')
    for c in children:
        yield from walk(c)
for lab in walk(deep(layout)):
    if lab:
        print(lab)
PYDUMP
)"
      MISS=0
      # 包含匹配（非整行）：菜单文本可能带前缀（如「⛔ 急停」）；六项互不为子串
      for label in 显示窗口 新建任务 收件箱 急停 设置 退出; do
        if echo "$LABELS" | grep -q "$label"; then :; else bad "菜单缺项: $label"; MISS=1; fi
      done
      if [ "$MISS" = 0 ]; then ok "托盘菜单六项齐全（dbusmenu @$MENUPATH：$(echo $LABELS | tr '\n' ' ')）"; fi
    fi

    # --- 断言 5a：Alt+M 可见→藏 ---
    # 判据：xdotool search --onlyvisible（xwininfo -tree 不过滤 unmapped）
    # 注：SNI Activate（左键）在 tray-icon libappindicator 后端无事件
    #（上游未连接 activate 信号，2026-10-06 源码实证）—— 唤回走菜单/Alt+M/单实例
    win_visible() { xdotool search --onlyvisible --name "Maestro" >/dev/null 2>&1; }
    if kill -0 $GUI_PID 2>/dev/null; then
      xdotool key --clearmodifiers alt+m
      sleep 1
      if win_visible; then
        bad "Alt+M 后窗口仍可见（未隐藏）"
      else
        ok "Alt+M 隐藏窗口"
        # --- 断言 4：菜单「显示窗口」项点击（dbusmenu Event）→ 窗口恢复 ---
        # Linux 真实可用的托盘唤回路径：on_menu_event → 前端 dispatch / show
        if [ -n "$MENUPATH" ]; then
          python3 - "$DEST" "$MENUPATH" 显示窗口 <<'PYCLICK' 2>>"$SNI_TMP/gui.log" && CLICKED=1 || CLICKED=0
import sys, dbus
sender, menupath, want = sys.argv[1], sys.argv[2], sys.argv[3]
bus = dbus.SessionBus()
obj = bus.get_object(sender, menupath)
menu = dbus.Interface(obj, 'com.canonical.dbusmenu')
_rev, layout = menu.GetLayout(0, -1, [])
def deep(n):
    if isinstance(n, (dbus.Struct, dbus.Array)):
        return [deep(x) for x in n]
    if isinstance(n, dbus.Dictionary):
        return {k: deep(v) for k, v in n.items()}
    return n
def find(node, w):
    i, props, children = node
    if props.get('label', '') == w:
        return i
    for c in children:
        r = find(c, w)
        if r is not None:
            return r
    return None
mid = find(deep(layout), want)
if mid is None:
    sys.exit(2)
menu.Event(mid, 'clicked', dbus.String(''), dbus.UInt32(0))
PYCLICK
          sleep 1
          if [ "$CLICKED" = 1 ] && win_visible; then
            ok "托盘菜单「显示窗口」点击唤起窗口（dbusmenu Event 链路）"
          elif [ "$CLICKED" = 0 ]; then
            bad "菜单项点击失败（helper 异常，见 gui.log）"
          else
            bad "菜单项已点击但窗口未恢复"
          fi
        fi
        # --- 断言 5b：Alt+M toggle 双向（此刻窗口可见：先藏→再唤）---
        xdotool key --clearmodifiers alt+m
        sleep 1
        if win_visible; then
          bad "Alt+M toggle：可见时按应隐藏，未生效"
        else
          xdotool key --clearmodifiers alt+m
          sleep 1
          if win_visible; then
            ok "Alt+M toggle 双向（藏→唤）"
          else
            bad "Alt+M 第二次未唤起"
          fi
        fi
      fi
    else
      bad "GUI 进程已退出（启动即崩？）—— $(tail -5 "$SNI_TMP/gui.log" 2>/dev/null)"
    fi

    # --- 清理本轮（保留日志；watcher 不 kill 会让 wait 永久阻塞）---
    kill $GUI_PID $WATCHER_PID $XVFB_PID 2>/dev/null || true
    pkill -x maestro-daemon 2>/dev/null || true
    wait 2>/dev/null || true
OUTER

  # --- 断言 6：无 watcher 降级（裸 dbus session，GUI 不崩）---
  # heredoc 无引号：bash 特殊变量（$!）须转义；-k 硬杀同 OUTER 防御
  timeout -k 5 40 dbus-run-session -- bash -e <<INNER
    ok()  { echo "PASS: \$*"; echo "PASS: \$*" >> "\$LOG"; }
    bad() { echo "FAIL: \$*" >&2; echo "FAIL: \$*" >> "\$LOG"; }
    Xvfb :98 -screen 0 1280x800x24 >/dev/null 2>&1 &
    XV=\$!
    export DISPLAY=:98
    sleep 1
    "\$BIN" >/dev/null 2>"\$SNI_TMP/gui-nodeck.log" &
    GP=\$!
    sleep 8
    if kill -0 \$GP 2>/dev/null; then
      ok "无托盘宿主：GUI 存活 8s（优雅降级，无 panic）"
      kill \$GP 2>/dev/null || true
      pkill -x maestro-daemon 2>/dev/null || true
    else
      bad "无托盘宿主：GUI 退出（gui-nodeck.log: \$(tail -3 "\$SNI_TMP/gui-nodeck.log" 2>/dev/null)）"
    fi
    kill \$XV 2>/dev/null || true
    wait 2>/dev/null || true
INNER

  # 汇总计数（内容子 bash 已实时输出）
  while IFS= read -r line; do
    case "$line" in
      PASS:*) PASS=$((PASS+1)) ;;
      FAIL:*) FAIL=$((FAIL+1)) ;;
    esac
  done < "$LOG"
}

# ---------------------------------------------------------------------------
live_mode() {
  echo "== live 模式：当前桌面会话 SNI 托盘实测 =="
  echo "桌面: ${XDG_CURRENT_DESKTOP:-未知} / session: ${XDG_SESSION_TYPE:-?}"
  case "${XDG_CURRENT_DESKTOP:-}" in
    *GNOME*)
      if command -v gnome-extensions >/dev/null && gnome-extensions list 2>/dev/null | grep -qi appindicator; then
        echo "[ok] GNOME + AppIndicator 扩展已装（SNI 宿主应存在）"
      else
        echo "[warn] GNOME 未检测到 AppIndicator 扩展 —— 托盘大概率不可见，装 AppIndicator/TopIcons 之一"
      fi ;;
    *KDE*) echo "[ok] KDE Plasma：SNI 原生支持" ;;
    *) echo "[warn] 非 GNOME/KDE 桌面 —— 按 SNI 兼容宿主处理" ;;
  esac
  if ! gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
      --method org.freedesktop.DBus.ListNames 2>/dev/null | grep -q StatusNotifierWatcher; then
    echo "FAIL: org.kde.StatusNotifierWatcher 无 owner —— 本会话无托盘宿主"
    FAIL=1
    echo "== 结果: 0 PASS / 1 FAIL =="; exit 1
  fi
  echo "PASS: SNI 宿主在线（org.kde.StatusNotifierWatcher 有 owner）"

  "$BIN" >/dev/null 2>&1 &
  GUI_PID=$!
  FOUND=""
  for i in $(seq 1 20); do
    if gdbus call --session --dest org.kde.StatusNotifierWatcher \
        --object-path /StatusNotifierWatcher \
        --method org.freedesktop.DBus.Properties.Get \
        org.kde.StatusNotifierWatcher RegisteredStatusNotifierItems 2>/dev/null \
        | grep -qE "NotificationItem|StatusNotifierItem"; then
      FOUND=1; break
    fi
    sleep 1
  done
  if [ -n "$FOUND" ]; then
    echo "PASS: 真实宿主接受注册"
    echo "== 人工目视确认（自动化到此为止，像素级可见性需目击）=="
    echo "  1. 任务栏/托盘区应出现 Maestro 图标"
    echo "  2. 左键 → 窗口唤起；右键 → 菜单六项（显示窗口/新建任务/收件箱/急停/设置/退出）"
    echo "  3. 确认后: kill $GUI_PID 收尾"
    PASS=1
  else
    echo "FAIL: 20s 内宿主未接受注册 —— GNOME 无扩展 / 宿主异常"
    FAIL=1
    kill $GUI_PID 2>/dev/null || true
  fi
  echo "== 结果: $PASS PASS / $FAIL FAIL =="
  exit $([ "$FAIL" = 0 ] && echo 0 || echo 1)
}

case "$MODE" in
  mock) mock_mode ;;
  live) live_mode ;;
  *) echo "用法: $0 [mock|live] [--bin PATH]" >&2; exit 2 ;;
esac

echo "== 结果: $PASS PASS / $FAIL FAIL =="
exit $([ "$FAIL" = 0 ] && echo 0 || echo 1)
