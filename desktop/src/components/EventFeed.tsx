// 右栏事件流：最新在上（60 条窗口），priority 着色

import { useMemo } from "react";
import { useStore } from "../state/store";
import { evText, evTaskId } from "../state/events";

export default function EventFeed() {
  const { state } = useStore();

  const rows = useMemo(
    () =>
      state.events
        .slice(-60)
        .reverse()
        .map((env) => ({
          seq: env.seq,
          ts: env.ts,
          text: evText(env.event),
          task: evTaskId(env.event),
          priority: env.priority,
        })),
    [state.events]
  );

  if (rows.length === 0) {
    return <div className="hint" style={{ padding: "16px 0" }}>暂无事件 —— 引擎启动后这里会实时滚动。</div>;
  }

  return (
    <div className="evlist">
      {rows.map((r) => (
        <div key={r.seq} className={`ev p-${r.priority}`}>
          <span className="t">{fmtTime(r.ts)}</span>
          <span className="x">{r.text}</span>
          {r.task && <span className="taskid">{r.task}</span>}
        </div>
      ))}
    </div>
  );
}

function fmtTime(ts: number): string {
  const d = new Date(ts);
  return d.toLocaleTimeString("zh-CN", { hour12: false });
}
