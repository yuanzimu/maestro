// 设置：worker 模式（演示 / claude CLI / 自定义）、引擎控制（重启/关停）、
// 全局快捷键状态（C4-3：Wayland 降级引导）

import { useEffect, useState } from "react";
import { useStore } from "../state/store";
import * as api from "../api";
import type { Settings, WorkerMode, WorkerProbe, ShortcutStatus } from "../types";

export default function SettingsDialog() {
  const { state, dispatch } = useStore();
  const [s, setS] = useState<Settings | null>(null);
  const [probe, setProbe] = useState<WorkerProbe | null>(null);
  const [shortcut, setShortcut] = useState<ShortcutStatus | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.getSettings().then(setS).catch(() => setS(null));
    api.probeWorker().then(setProbe).catch(() => setProbe(null));
    api.shortcutStatus().then(setShortcut).catch(() => setShortcut(null));
  }, []);

  if (!s) return null;

  const save = async (next: Settings) => {
    setBusy(true);
    try {
      await api.saveSettings(next);
      setS(next);
      dispatch({ type: "toast", kind: "ok", text: "已保存 —— 重启引擎后生效" });
    } catch (e) {
      dispatch({ type: "toast", kind: "err", text: String(e) });
    } finally {
      setBusy(false);
    }
  };

  const setMode = (mode: WorkerMode) => save({ ...s, worker_mode: mode });

  return (
    <div className="overlay" onClick={() => dispatch({ type: "dialog", dialog: null })}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h3>设置</h3>

        <section>
          <h4>Worker 引擎（AI 执行器）</h4>
          <div className="mode-grid">
            <button
              className={`mode ${s.worker_mode === "demo" ? "sel" : ""}`}
              disabled={busy}
              onClick={() => setMode("demo")}
            >
              <b>🧪 演示模式</b>
              <span>
                内置 mock worker（无需任何 AI key），3 轮模拟产出后完成 —— 用来体验全流程
                {probe && !probe.mock_path && <b className="dim">—— 发行包未携带演示 worker，请改用下方模式</b>}
              </span>
            </button>
            <button
              className={`mode ${s.worker_mode === "claude" ? "sel" : ""}`}
              disabled={busy}
              onClick={() => setMode("claude")}
            >
              <b>🤖 Claude Code</b>
              <span>本机 claude CLI（多轮驱动 + 断点续跑）{probe && !probe.claude_found && <b className="dim">—— 未检测到，请先安装</b>}</span>
            </button>
            <button
              className={`mode ${s.worker_mode === "custom" ? "sel" : ""}`}
              disabled={busy}
              onClick={() => setMode("custom")}
            >
              <b>⌨ 自定义命令</b>
              <span>headless CLI（须兼容 claude stream-json 或配置 MAESTRO_CLI_DIALECT）</span>
            </button>
          </div>
          {s.worker_mode === "custom" && (
            <div className="custom-cmd">
              <label>
                程序
                <input
                  value={s.custom_program}
                  placeholder="例：C:\\tools\\my-cli.exe"
                  onChange={(e) => setS({ ...s, custom_program: e.target.value })}
                />
              </label>
              <label>
                参数（空格分隔，透传给内层 CLI）
                <input
                  value={s.custom_args}
                  placeholder="留空即默认"
                  onChange={(e) => setS({ ...s, custom_args: e.target.value })}
                />
              </label>
              <button className="primary" disabled={busy} onClick={() => save(s)}>
                保存自定义命令
              </button>
            </div>
          )}
        </section>

        <section>
          <h4>引擎状态</h4>
          <div className="kv">
            <span className="k">引擎</span>
            <span className="v">
              {state.daemon.running
                ? `运行中 · v${state.daemon.version || "?"} · pid ${state.daemon.pid} · ${state.daemon.managed ? "由本应用托管" : "外部启动（接管）"}`
                : "未运行"}
            </span>
            {!state.daemon.running && state.daemon.error && (
              <>
                <span className="k">失败原因</span>
                <span className="v">{state.daemon.error}</span>
              </>
            )}
            {probe?.data_dir && (
              <>
                <span className="k">数据目录</span>
                <span className="v mono small">{probe.data_dir}</span>
              </>
            )}
          </div>
          <div className="btns" style={{ marginTop: 8 }}>
            <button disabled={busy || !state.daemon.running} onClick={() => api.restartDaemon().then(
              () => dispatch({ type: "toast", kind: "ok", text: "引擎已重启" }),
              (e) => dispatch({ type: "toast", kind: "err", text: String(e) })
            )}>
              ↻ 重启引擎
            </button>
            <button
              className="warn"
              disabled={busy || !state.daemon.running}
              onClick={() =>
                api.shutdownDaemon().then(
                  () => dispatch({ type: "toast", kind: "ok", text: "引擎已关停（任务现场保留）" }),
                  (e) => dispatch({ type: "toast", kind: "err", text: String(e) })
                )
              }
            >
              ⏹ 关停引擎
            </button>
          </div>
          <div className="hint" style={{ marginTop: 8 }}>
            关掉本应用窗口不会杀引擎 —— 任务照跑，重开窗口自动接上现场。
          </div>
        </section>

        {shortcut && (
          <section>
            <h4>全局快捷键</h4>
            <div className="kv">
              <span className="k">{shortcut.shortcut}</span>
              <span className="v">
                {shortcut.registered
                  ? "已注册 —— 任意界面按下即可唤起 / 藏起指挥台"
                  : shortcut.wayland
                    ? "未注册 —— Wayland 桌面不支持应用级全局快捷键"
                    : "未注册"}
              </span>
            </div>
            {!shortcut.registered && shortcut.wayland && (
              <div className="hint" style={{ marginTop: 8 }}>
                Wayland 出于安全设计不允许多数应用自行注册全局快捷键。可在
                <b> 系统设置 → 键盘 → 自定义快捷键</b> 中手动绑定，命令填
                <code> maestro-desktop</code>（deb/rpm 安装形态；已在运行时
                再次启动只会唤起既有窗口，不会开新实例）。托盘图标与 Alt+M
                在 X11 / Windows 上可用。
              </div>
            )}
            {!shortcut.registered && !shortcut.wayland && shortcut.error && (
              <div className="hint" style={{ marginTop: 8 }}>
                注册失败：{shortcut.error}
              </div>
            )}
          </section>
        )}

        <div className="modal-actions">
          <button onClick={() => dispatch({ type: "dialog", dialog: null })}>关闭</button>
        </div>
      </div>
    </div>
  );
}
