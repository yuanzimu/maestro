// 任务详情（右滑面板）：概况 + 账本（成本）+ 快照时间线（回滚）+ 该任务事件流

import { useEffect, useState } from "react";
import { useStore } from "../state/store";
import * as api from "../api";
import { evText } from "../state/events";
import type { CheckpointItem, LedgerSummary, TaskDetail as TaskDetailT } from "../types";
import ResultCard from "./ResultCard";

export default function TaskDetailPanel({ id }: { id: string }) {
  const { state, dispatch } = useStore();
  const [detail, setDetail] = useState<TaskDetailT | null>(null);
  const [ledger, setLedger] = useState<LedgerSummary | null>(null);
  const [checkpoints, setCheckpoints] = useState<CheckpointItem[]>([]);
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      const [d, l, cps] = await Promise.all([
        api.getTask(id),
        api.getLedger(id).catch(() => null),
        api.listCheckpoints(id).catch(() => [] as CheckpointItem[]),
      ]);
      setDetail(d);
      setLedger(l);
      setCheckpoints(cps);
    } catch {
      /* 任务可能已不存在 */
    }
  };

  useEffect(() => {
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, [id]);

  const taskEvents = state.events
    .filter((e) => {
      const t = e.event["task"];
      const tid = typeof t === "string" ? t : t && typeof t === "object" ? (t as { id?: string }).id : null;
      return tid === id;
    })
    .slice(-100)
    .reverse();

  const rollback = async (to: string) => {
    const cp = checkpoints.find((c) => c.ref === to);
    const label = cp ? `${cp.reason}@${cp.commit.slice(0, 7)}` : to;
    if (!confirm(`回滚到快照 ${label}？\n当前未提交的改动会先存入"pre-rollback"安全垫，不会丢。`)) return;
    setBusy(true);
    try {
      const r = await api.rollbackCheckpoint(id, to);
      dispatch({
        type: "toast",
        kind: "ok",
        text: `已回滚到 ${r.rolled_back_to}（安全垫 ${r.pre_rollback}）`,
      });
      load();
    } catch (e) {
      dispatch({ type: "toast", kind: "err", text: String(e) });
    } finally {
      setBusy(false);
    }
  };

  const task = state.tasks[id] ?? detail;

  return (
    <div className="overlay right" onClick={() => dispatch({ type: "select", id: null })}>
      <div className="detail" onClick={(e) => e.stopPropagation()}>
        <div className="detail-head">
          <h3>{task?.title ?? id}</h3>
          <button className="ghost" onClick={() => dispatch({ type: "select", id: null })}>
            ✕ 关闭
          </button>
        </div>

        {task && (
          <div className="kv">
            <span className="k">状态</span>
            <span className="v">
              <span className={`st ${task.state}`} /> {task.state} · 第 {task.round} 轮
            </span>
            <span className="k">任务 ID</span>
            <span className="v mono">{id}</span>
            <span className="k">进度</span>
            <span className="v">{task.narrative || "—"}</span>
            {task.suspend_reason && (
              <>
                <span className="k">挂起原因</span>
                <span className="v">{task.suspend_reason}</span>
              </>
            )}
          </div>
        )}

        {/* U5 结果卡：终态任务置顶 5 秒扫视（非终态组件自渲染 null） */}
        {task && <ResultCard task={task} ledger={ledger} events={taskEvents} />}

        {ledger && (
          <section>
            <h4>账本（token 经济）</h4>
            <div className="ledger">
              <div>
                <b>{ledger.rounds}</b>
                <span>轮</span>
              </div>
              <div>
                <b>{fmtDur(ledger.wall_ms)}</b>
                <span>耗时</span>
              </div>
              <div>
                <b>{fmtTok(ledger.input_tokens + ledger.output_tokens + ledger.cache_read_tokens)}</b>
                <span>tokens</span>
              </div>
              <div>
                <b>{ledger.actual_cost_cents}¢</b>
                <span>实际成本</span>
              </div>
              <div className={ledger.saved_cents > 0 ? "good" : ""}>
                <b>{ledger.saved_cents > 0 ? `省 ${ledger.saved_cents}¢` : "—"}</b>
                <span>vs 反事实</span>
              </div>
              <div>
                <b>{ledger.compactions}</b>
                <span>上下文压缩</span>
              </div>
            </div>
          </section>
        )}

        <section>
          <h4>快照时间线（回滚用）</h4>
          {checkpoints.length === 0 ? (
            <div className="hint">无快照（workdir 须为 git 仓库才会产生）</div>
          ) : (
            <div className="cps">
              {checkpoints.map((c) => (
                <div key={c.ref} className="cp">
                  <span className="cp-dot" />
                  <span className="cp-meta">
                    #{c.seq} {c.reason}
                    <span className="mono dim"> {c.commit.slice(0, 7)}</span>
                  </span>
                  <button className="ghost" disabled={busy} onClick={() => rollback(c.ref)}>
                    回滚
                  </button>
                </div>
              ))}
            </div>
          )}
        </section>

        <section>
          <h4>事件时间线（最新 {taskEvents.length} 条）</h4>
          <div className="evlist">
            {taskEvents.length === 0 && <div className="hint">暂无该任务的事件。</div>}
            {taskEvents.map((e) => (
              <div key={e.seq} className={`ev p-${e.priority}`}>
                <span className="t">{new Date(e.ts).toLocaleTimeString("zh-CN", { hour12: false })}</span>
                <span className="x">{evText(e.event)}</span>
              </div>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}

function fmtDur(ms: number): string {
  if (!ms) return "0s";
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m${s % 60}s`;
  return `${Math.floor(s / 3600)}h${Math.floor((s % 3600) / 60)}m`;
}

function fmtTok(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}
