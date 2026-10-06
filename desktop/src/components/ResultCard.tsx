// U5 结果卡：终态任务的 5 秒扫视卡 ——
// 一句话摘要（TaskCompleted.summary）+ 验收门状态 + 花费 + 节省比例 + 变更明细 diff（按需加载）
// DEV_PLAN B1-5：改 TaskDetail 顶部 / 5 秒可扫视；节省数据显著
// C2：diff hunk 级部分接受 —— 勾选拒绝 + 理由 → 撤销该 hunk 并自动转修正任务

import { useMemo, useState } from "react";
import { useStore } from "../state/store";
import * as api from "../api";
import type { DiffSummary, Envelope, LedgerSummary, TaskSummary } from "../types";

const TERMINAL = new Set(["done", "failed", "cancelled"]);

/// hunk 解析（与后端 split_patch 同规则：`diff --git` 分文件、`@@` 分
/// hunk、全局序号 = 解析顺序）—— 后端按同序号拼拒绝子 patch，
/// 两端规则必须一致，改动需双侧同步。
interface Hunk {
  index: number;
  head: string;
  body: string[];
}
interface FileSection {
  path: string;
  hunks: Hunk[];
}

function parsePatch(patch: string): FileSection[] {
  const files: FileSection[] = [];
  let cur: FileSection | null = null;
  for (const line of patch.split("\n")) {
    if (line.startsWith("diff --git ")) {
      cur = { path: "", hunks: [] };
      files.push(cur);
    } else if (line.startsWith("+++ b/")) {
      if (cur && !cur.path) cur.path = line.slice(6);
    } else if (line.startsWith("--- a/")) {
      // 删除文件 b 侧为 /dev/null → 用 a 侧路径（且不得覆盖已有 b/ 值）
      if (cur && !cur.path) cur.path = line.slice(6);
    } else if (line.startsWith("@@")) {
      cur?.hunks.push({ index: 0, head: line, body: [] });
    } else if (cur?.hunks.length) {
      cur.hunks[cur.hunks.length - 1].body.push(line);
    }
  }
  let i = 0;
  for (const f of files) {
    if (!f.path) f.path = "(未知)";
    for (const h of f.hunks) h.index = i++;
  }
  return files;
}

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
  const { dispatch } = useStore();
  const [diff, setDiff] = useState<DiffSummary | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [reject, setReject] = useState<Set<number>>(new Set());
  const [reason, setReason] = useState("");
  // U7 反馈流：fbNegative = null 收起 / false 展开 👎 理由输入
  const [fbNegative, setFbNegative] = useState<boolean | null>(null);
  const [fbReason, setFbReason] = useState("");
  const [fbBusy, setFbBusy] = useState(false);

  // hooks 必须全部先于条件早退（否则非终态→终态翻转时
  // "Rendered more hooks than during the previous render" 崩整面板）
  const sections = useMemo(
    () => (diff?.patch ? parsePatch(diff.patch) : []),
    [diff?.patch]
  );

  // 非终态不渲染（运行中任务看事件时间线即可）
  if (!TERMINAL.has(task.state)) return null;

  // 已反馈：事件流为准（FeedbackRecorded 已入库）——重开详情恢复状态
  const feedback = events.find(
    (e) => e.event.type === "feedback_recorded" && e.event.task === task.id
  )?.event;

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

  const toggleHunk = (i: number) => {
    setReject((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });
  };

  const submitReject = async () => {
    if (!reason.trim() || reject.size === 0 || !diff?.patch) return;
    setBusy(true);
    try {
      // 传「所见原文」：后端按它拼拒绝子 patch；工作区已漂移则 apply 原子失败
      const r = await api.taskDiffRevert(task.id, [...reject], reason.trim(), diff.patch);
      dispatch({
        type: "toast",
        kind: "ok",
        text: `已拒绝 ${r.reverted_hunks} 段并转修正任务（${r.reverted_files} 文件已还原）`,
      });
      // 重载 diff（剩余改动）+ 清拒绝流
      setDiff(null);
      setReject(new Set());
      setReason("");
      setDiff(await api.taskDiff(task.id));
    } catch (e) {
      dispatch({ type: "toast", kind: "err", text: String(e) });
    } finally {
      setBusy(false);
    }
  };

  // U7 反馈：👍 直发（理由可选）；👎 必填理由（后端 400 校验双保险）
  const submitFeedback = async (positive: boolean, fbReasonText?: string) => {
    if (fbBusy) return;
    if (!positive && !fbReasonText?.trim()) return;
    setFbBusy(true);
    try {
      await api.taskFeedback(task.id, positive, fbReasonText?.trim());
      dispatch({
        type: "toast",
        kind: "ok",
        text: positive ? "已记录 👍" : "已记录 👎（理由入项目记忆）",
      });
      setFbNegative(null);
      setFbReason("");
    } catch (e) {
      dispatch({ type: "toast", kind: "err", text: String(e) });
    } finally {
      setFbBusy(false);
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

      {/* U7 反馈闭环：已反馈从事件流恢复；👍 直发 / 👎 必填理由 */}
      <div className="rc-feedback">
        {feedback ? (
          <span className="dim">
            已反馈 {feedback.positive ? "👍" : "👎"}
            {feedback.reason ? `：${String(feedback.reason).slice(0, 40)}` : ""}
          </span>
        ) : fbNegative === false ? (
          <>
            <textarea
              className="rc-reason"
              placeholder="👎 拒绝理由（必填，将沉淀到项目记忆，指导后续任务）…"
              value={fbReason}
              onChange={(e) => setFbReason(e.target.value)}
              rows={2}
            />
            <div className="rc-fb-btns">
              <button
                className="ghost rc-reject-btn"
                disabled={fbBusy || !fbReason.trim()}
                onClick={() => submitFeedback(false, fbReason)}
              >
                {fbBusy ? "提交中…" : "提交 👎"}
              </button>
              <button className="ghost" onClick={() => setFbNegative(null)}>
                取消
              </button>
            </div>
          </>
        ) : (
          <div className="rc-fb-btns">
            <span className="dim">这个结果如何？</span>
            <button
              className="ghost rc-fb-good"
              disabled={fbBusy}
              onClick={() => submitFeedback(true)}
            >
              👍 好
            </button>
            <button
              className="ghost rc-reject-btn"
              disabled={fbBusy}
              onClick={() => setFbNegative(false)}
            >
              👎 不好
            </button>
          </div>
        )}
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
            {sections.map((f, fi) => (
              <div key={`${f.path}#${fi}`} className="rc-file">
                <div className="rc-file-path mono">{f.path || "(未知)"}</div>
                {f.hunks.map((h) => (
                  <div key={h.index} className={`rc-hunk ${reject.has(h.index) ? "rejected" : ""}`}>
                    <label className="rc-hunk-head">
                      <input
                        type="checkbox"
                        checked={reject.has(h.index)}
                        onChange={() => toggleHunk(h.index)}
                      />
                      <span className="mono dim">{h.head}</span>
                    </label>
                    <pre className="rc-hunk-body">
                      {h.body.join("\n").replace(/\n+$/, "")}
                    </pre>
                  </div>
                ))}
              </div>
            ))}
            {sections.every((f) => f.hunks.length === 0) && (
              <pre className="rc-patch">{diff.patch}</pre>
            )}
            <div className="rc-reject">
              <textarea
                className="rc-reason"
                placeholder={`拒绝理由（必填，将随被拒段落转给修正任务）…`}
                value={reason}
                onChange={(e) => setReason(e.target.value)}
                rows={2}
              />
              <button
                className="ghost rc-reject-btn"
                disabled={busy || reject.size === 0 || !reason.trim()}
                onClick={submitReject}
              >
                {reject.size > 0
                  ? `拒绝所选 ${reject.size} 段并转修正任务`
                  : "勾选要拒绝的段落"}
              </button>
            </div>
          </div>
        ) : (
          <div className="hint">{diff.reason}</div>
        )
      )}
    </section>
  );
}
