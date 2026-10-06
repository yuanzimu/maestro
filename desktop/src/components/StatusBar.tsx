// 顶栏：引擎状态点 + 版本/uptime + 任务计数 + 全局操作（新建/收件箱/急停/设置）

import { useState } from "react";
import { useStore } from "../state/store";
import * as api from "../api";
import type { EmergencyStopResult } from "../types";

export default function StatusBar() {
  const { state, dispatch } = useStore();
  const [stopping, setStopping] = useState(false);
  // C6 省 token 徽标：30 天口径（全量在 title 提示里不喧宾夺主）
  const savings30d = state.savings?.last_30d ?? null;

  const counts: Record<string, number> = {};
  for (const id of state.taskOrder) {
    const s = state.tasks[id]?.state;
    if (s) counts[s] = (counts[s] ?? 0) + 1;
  }

  const emergencyStop = async () => {
    if (!confirm("全局急停：冻结所有 AI worker 并保存现场？")) return;
    setStopping(true);
    try {
      const r: EmergencyStopResult = await api.emergencyStop("desktop-ui");
      dispatch({
        type: "toast",
        kind: "ok",
        text: `已急停：冻结 ${r.frozen_workers.length} 个 worker、挂起 ${r.suspended_tasks.length} 个任务（${r.freeze_ms}ms）`,
      });
    } catch (e) {
      dispatch({ type: "toast", kind: "err", text: String(e) });
    } finally {
      setStopping(false);
    }
  };

  return (
    <header>
      <span className={`dot ${state.daemon.running ? "on" : ""}`} title={state.daemon.running ? "引擎在线" : "引擎未连接"} />
      <span className="logo">Maestro 指挥台</span>
      <span className="hint">
        {state.daemon.running
          ? `引擎 v${state.daemon.version || "?"} · 已运行 ${fmtUptime(state.daemon.uptime_secs)}`
          : "引擎未连接"}
      </span>
      <span className="counts">
        进行中 <b>{counts.working ?? 0}</b> · 排队 <b>{counts.queued ?? 0}</b> · 待处理{" "}
        <b>{(counts.blocked ?? 0) + (counts.suspended ?? 0)}</b> · 完成 <b>{counts.done ?? 0}</b> · 失败{" "}
        <b>{(counts.failed ?? 0) + (counts.cancelled ?? 0)}</b>
      </span>
      {savings30d && savings30d.saved_pct !== null && savings30d.saved_pct > 0 && (
        <span
          className="hint"
          title={`30 天缓存命中率 ${savings30d.cache_hit_pct ?? 0}% · 反事实口径：同内容冷跑直连牌价`}
        >
          💰 30 天省 <b>{savings30d.saved_pct}%</b>（{savings30d.saved_cents}¢）
        </span>
      )}
      <button className="primary" onClick={() => dispatch({ type: "dialog", dialog: "new-task" })}>
        ＋ 新建任务
      </button>
      <button onClick={() => dispatch({ type: "dialog", dialog: state.dialog === "inbox" ? null : "inbox" })}>
        📮 收件箱{state.inbox.length > 0 ? ` (${state.inbox.length})` : ""}
      </button>
      <button className="warn" disabled={stopping} onClick={emergencyStop}>
        ⛔ 急停
      </button>
      <button onClick={() => dispatch({ type: "dialog", dialog: "settings" })}>⚙ 设置</button>
    </header>
  );
}

function fmtUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  return `${Math.floor(secs / 3600)}h${Math.floor((secs % 3600) / 60)}m`;
}
