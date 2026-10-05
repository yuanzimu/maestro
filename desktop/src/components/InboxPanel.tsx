// 收件箱：blocked 任务清单 + kind 中文标签 + 行动（重跑/放弃/全部恢复）

import { useState } from "react";
import { useStore } from "../state/store";
import * as api from "../api";
import type { BlockedKind } from "../types";

const KIND: Record<BlockedKind, string> = {
  permission: "权限待批",
  plan_approval: "方案待批",
  goal_stalled: "目标受阻",
  acceptance_failed: "假完成被拦",
  infra: "基建故障",
  rounds_exhausted: "轮数耗尽",
};

export default function InboxPanel() {
  const { state, dispatch } = useStore();
  const [busy, setBusy] = useState(false);

  const act = async (fn: () => Promise<unknown>, ok: string) => {
    setBusy(true);
    try {
      await fn();
      dispatch({ type: "toast", kind: "ok", text: ok });
    } catch (e) {
      dispatch({ type: "toast", kind: "err", text: String(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="inbox">
      <div className="inbox-head">
        <h3>收件箱 —— 需要你的决定</h3>
        <div className="btns">
          {state.emergency && (
            <button
              disabled={busy}
              onClick={() =>
                act(async () => {
                  await api.resumeAll("flush");
                  // RPC 成功 = daemon 已解除急停（EmergencyPhase::None）；
                  // 0 挂起任务时 daemon 不发任何事件，横幅须在此直清
                  dispatch({ type: "clear-emergency" });
                }, "已全部恢复（轻推 flush）")
              }
            >
              ▶▶ 全部恢复
            </button>
          )}
          <button className="ghost" onClick={() => dispatch({ type: "dialog", dialog: null })}>
            ← 返回任务列表
          </button>
        </div>
      </div>
      {state.inbox.length === 0 ? (
        <div className="empty">收件箱是空的 —— 没有等你决定的任务。</div>
      ) : (
        state.inbox.map((it) => (
          <div key={it.task} className="card">
            <div className="row1">
              <span className="st blocked" />
              <span className="title">{it.title}</span>
              <span className="tid">
                {KIND[it.kind ?? "infra"] ?? it.kind} · {it.task}
              </span>
            </div>
            <div className="btns">
              <button disabled={busy} onClick={() => act(() => api.resumeTask(it.task), `已重跑 ${it.title}`)}>
                ▶ 重跑
              </button>
              <button
                className="warn"
                disabled={busy}
                onClick={() => act(() => api.cancelTask(it.task), `已放弃 ${it.title}`)}
              >
                ✕ 放弃
              </button>
            </div>
          </div>
        ))
      )}
    </div>
  );
}
