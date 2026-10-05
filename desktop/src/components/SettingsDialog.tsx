// 设置：worker 模式（演示 / claude CLI / 自定义）、引擎控制（重启/关停）

import { useEffect, useState } from "react";
import { useStore } from "../state/store";
import * as api from "../api";
import type { Settings, WorkerMode, WorkerProbe } from "../types";

export default function SettingsDialog() {
  const { state, dispatch } = useStore();
  const [s, setS] = useState<Settings | null>(null);
  const [probe, setProbe] = useState<WorkerProbe | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.getSettings().then(setS).catch(() => setS(null));
    api.probeWorker().then(setProbe).catch(() => setProbe(null));
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
              <span>内置 mock worker（无需任何 AI key），3 轮模拟产出后完成 —— 用来体验全流程</span>
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

        <div className="modal-actions">
          <button onClick={() => dispatch({ type: "dialog", dialog: null })}>关闭</button>
        </div>
      </div>
    </div>
  );
}
