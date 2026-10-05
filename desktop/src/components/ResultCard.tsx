// U5 结果卡：终态任务的 5 秒扫视卡 ——
// 一句话摘要（TaskCompleted.summary）+ 验收门状态 + 花费 + 节省比例 + 变更明细 diff（按需加载）
// DEV_PLAN B1-5：改 TaskDetail 顶部 / 5 秒可扫视；节省数据显著

import { useState } from "react";
import * as api from "../api";
import type { DiffSummary, Envelope, LedgerSummary, TaskSummary } from "../types";

const TERMINAL = new Set(["done", "failed", "cancelled"]);

export default function ResultCard({
  task,
  ledger,
  events,
}: {
  task: TaskSummary;
  ledger: LedgerSummary | null;
  /** 本任务事件流（最新在前，与 TaskDetail 的 taskEvents 同序） */
  events: Envelope[];
}) {
  const [diff, setDiff] = useState<DiffSummary | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  // 非终态不渲染（运行中任务看事件时间线即可）
  if (!TERMINAL.has(task.state)) return null;

  // 终态信息以事件流为权威源，narrative 兜底（事件可能被截断/错过）
  const last = (type: string) =>
    events.find((e) => e.event.type === type)?.event;
  const completed = last("task_completed");
  const failed = last("task_failed");
  const gatePassed = last("acceptance_gate_passed");
  const gateFailed = last("acceptance_gate_failed");

  const summary =
    task.state === "done"
      ? String(completed?.summary ?? task.narrative)
      : task.state === "failed"
        ? String(failed?.error ?? task.narrative)
        : "已取消（未完成全部目标）";

  // 验收门：done 隐含通过；output 是门的原始输出（如 "12/12 tests passed"）
  const gate =
    task.state === "done"
      ? gatePassed
        ? `通过 · ${String(gatePassed.output ?? "").slice(0, 24)}`
        : "通过"
      : gateFailed
        ? `被拦（第 ${gateFailed.failures} 次）`
        : "未通过";

  // 节省比例 = saved / 反事实成本（0 或缺账本时显示 —）
  const pct =
    ledger && ledger.counterfactual_cost_cents > 0
      ? Math.round((ledger.saved_cents / ledger.counterfactual_cost_cents) * 100)
      : null;

  const toggle = async () => {
    if (open) {
      setOpen(false);
      return;
    }
    setBusy(true);
    try {
      // diff 缓存一次即可；失败也缓存 reason，避免反复重试闪烁
      setDiff(diff ?? (await api.taskDiff(task.id)));
      setOpen(true);
    } catch (e) {
      setDiff({ available: false, reason: String(e) });
      setOpen(true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="result-card">
      <div className="rc-head">
        <b className={`rc-state ${task.state}`}>
          {task.state === "done" ? "✓ 完成" : task.state === "failed" ? "✗ 失败" : "⊘ 已取消"}
        </b>
        <span className="rc-summary">{summary}</span>
      </div>
      <div className="rc-stats">
        <div>
          <b>{gate}</b>
          <span>验收门</span>
        </div>
        <div>
          <b>{ledger ? `${ledger.actual_cost_cents}¢` : "—"}</b>
          <span>花费</span>
        </div>
        <div className={pct !== null && pct > 0 ? "good" : ""}>
          <b>{pct !== null && pct > 0 ? `省 ${pct}%` : "—"}</b>
          <span>vs 直连成本</span>
        </div>
        <div>
          <b>{task.round}</b>
          <span>轮数</span>
        </div>
      </div>
      <button className="ghost rc-diff-btn" disabled={busy} onClick={toggle}>
        {busy ? "diff 计算中…" : open ? "收起变更明细" : "查看变更明细（diff）"}
      </button>
      {open && diff && (
        diff.available ? (
          <div className="rc-diff">
            <div className="rc-diff-meta">
              {diff.files} 文件
              <span className="ins"> +{diff.insertions}</span>
              <span className="del"> −{diff.deletions}</span>
              {diff.truncated && <span className="dim">（已截断）</span>}
            </div>
            <pre className="rc-patch">{diff.patch}</pre>
          </div>
        ) : (
          <div className="hint">{diff.reason}</div>
        )
      )}
    </section>
  );
}
