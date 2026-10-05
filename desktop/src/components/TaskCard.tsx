// 任务卡：状态色点 + 标题 + 轮数 + narrative 一句话 + 就地操作
// （轻推/暂停/恢复/放弃/详情）—— 逐行对照 ui.rs card()

import { useState } from "react";
import { useStore } from "../state/store";
import * as api from "../api";
import type { TaskSummary } from "../types";
import { AUTO_RESUME_REASONS } from "../state/events";

const ST: Record<string, string> = {
  done: "完成",
  working: "进行中",
  planning: "规划中",
  queued: "排队中",
  blocked: "待处理",
  failed: "失败",
  cancelled: "已取消",
  suspended: "已挂起",
};

export default function TaskCard({ task }: { task: TaskSummary }) {
  const { dispatch } = useStore();
  const [steer, setSteer] = useState("");
  const [sending, setSending] = useState(false);

  const act = async (fn: () => Promise<unknown>, okText?: string) => {
    try {
      await fn();
      if (okText) dispatch({ type: "toast", kind: "ok", text: okText });
    } catch (e) {
      dispatch({ type: "toast", kind: "err", text: String(e) });
    }
  };

  const sendSteer = async () => {
    if (!steer.trim() || sending) return;
    setSending(true);
    try {
      await api.steerTask(task.id, steer.trim());
      dispatch({ type: "toast", kind: "ok", text: "轻推已排队，下一轮生效" });
      setSteer("");
    } catch (e) {
      dispatch({ type: "toast", kind: "err", text: String(e) });
    } finally {
      setSending(false);
    }
  };

  const autoResume = task.suspend_reason && AUTO_RESUME_REASONS.has(task.suspend_reason);

  return (
    <div className="card" onClick={() => dispatch({ type: "select", id: task.id })}>
      <div className="row1">
        <span className={`st ${task.state}`} title={ST[task.state] ?? task.state} />
        <span className="title">{task.title}</span>
        <span className="tid">
          {task.id} · 第 {task.round} 轮
        </span>
      </div>
      {task.narrative && <div className="narrative">{task.narrative}</div>}
      <div className="btns" onClick={(e) => e.stopPropagation()}>
        {task.state === "working" && (
          <>
            <input
              className="steer"
              maxLength={500}
              placeholder="给 AI 补充指示（下一轮生效）…"
              value={steer}
              onChange={(e) => setSteer(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && sendSteer()}
            />
            <button disabled={sending} onClick={sendSteer}>
              发送
            </button>
            <button onClick={() => act(() => api.pauseTask(task.id), "已暂停")}>⏸ 暂停</button>
          </>
        )}
        {task.state === "planning" && (
          <button onClick={() => act(() => api.pauseTask(task.id), "已暂停")}>⏸ 暂停</button>
        )}
        {task.state === "suspended" && (
          <>
            <span className="hint">
              {autoResume ? "挂起中 —— 网络类原因会自动恢复" : `挂起（${task.suspend_reason}）`}
            </span>
            <button onClick={() => act(() => api.resumeTask(task.id), "已恢复")}>▶ 恢复</button>
            <button className="warn" onClick={() => act(() => api.cancelTask(task.id), "已放弃")}>
              ✕ 放弃
            </button>
          </>
        )}
        {task.state === "blocked" && (
          <>
            <span className="hint">⚑ 需要你的决定</span>
            <button onClick={() => act(() => api.resumeTask(task.id), "已重新排队")}>▶ 重跑</button>
            <button className="warn" onClick={() => act(() => api.cancelTask(task.id), "已放弃")}>
              ✕ 放弃
            </button>
          </>
        )}
        {(task.state === "failed" || task.state === "cancelled") && (
          <>
            <button onClick={() => act(() => api.resumeTask(task.id), "已重新排队")}>▶ 重跑</button>
            <button className="warn" onClick={() => dispatch({ type: "select", id: task.id })}>
              查看详情
            </button>
          </>
        )}
        <button className="ghost" onClick={() => dispatch({ type: "select", id: task.id })}>
          详情
        </button>
      </div>
    </div>
  );
}
