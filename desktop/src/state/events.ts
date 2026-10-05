// 事件 → 中文文案（移植 maestro ui.rs 的 evText）+ 事件 → 状态映射。
// Envelope.event 是宽松 dict：按 type 分发，字段按需读取（前向兼容）。

import type { Envelope, TaskSummary, WorkerState } from "../types";

/// 事件的一句话中文描述（右栏事件流 + 详情页时间线共用）
export function evText(ev: Record<string, unknown> & { type: string }): string {
  switch (ev.type) {
    case "task_created": {
      const t = ev.task as { title?: string } | undefined;
      return `新任务「${t?.title ?? ""}」`;
    }
    case "task_started":
      return "开始执行";
    case "round_progress":
      return `第 ${ev.round} 轮：${String(ev.summary ?? "").slice(0, 60)}`;
    case "task_completed":
      return `✓ 完成：${String(ev.summary ?? "").slice(0, 50)}`;
    case "task_failed":
      return `✗ 失败：${String(ev.error ?? "").slice(0, 60)}`;
    case "task_cancelled":
      return "已取消";
    case "task_requeued":
      return "已重新排队（重试）";
    case "worker_spawned":
      return "worker 已启动";
    case "worker_died":
      return `worker 退出（code ${ev.exit_code ?? "?"}）`;
    case "steering_queued":
      return `💬 轻推：${String(ev.message ?? "").slice(0, 40)}`;
    case "steering_delivered":
      return `轻推已投递（第 ${ev.round} 轮生效）`;
    case "steering_dropped":
      return "轻推已丢弃（任务终态）";
    case "suspended":
      return `挂起（${ev.reason ?? ""}）`;
    case "resumed":
      return "已恢复";
    case "resume_attempt":
      return `自动恢复第 ${ev.attempt} 次（${ev.next_backoff_secs}s 后）`;
    case "auto_recovery_exhausted":
      return "⚑ 自动恢复耗尽，等你决定";
    case "rounds_exhausted":
      return `⚑ ${ev.rounds} 轮预算耗尽，等你决定`;
    case "acceptance_gate_failed":
      return `⚠ 假完成被拦（第 ${ev.failures} 次，3 次出局）`;
    case "acceptance_gate_passed":
      return "验收通过";
    case "goal_progress":
      return ev.blocked_condition
        ? `⚑ 目标受阻：${String(ev.blocked_condition).slice(0, 40)}`
        : `目标推进中（第 ${ev.round} 轮）`;
    case "narrative_snapshot":
      return `📍 ${String(ev.milestone ?? "").slice(0, 60)}`;
    case "cost_drift":
      return `⚠ 费用对账漂移 ${ev.ledger_cents}¢ vs ${ev.cli_cents}¢`;
    case "context_compacted":
      return "上下文已压缩，继续";
    case "checkpoint_created":
      return `快照已存（${String(ev.cp ?? "").split("/").pop() ?? ""}）`;
    case "checkpoint_rolled_back":
      return `⏪ 已回滚到快照（另留安全垫）`;
    case "ledger_entry":
      return "轮账已记";
    case "emergency_stopped":
      return `⛔ 全局急停：${String(ev.reason ?? "")}（现场冻结保留）`;
    case "emergency_snapshotted":
      return "急停快照完成";
    case "provider_switched":
      return `模型切换 ${ev.from} → ${ev.to}（${ev.cause}）`;
    case "feedback_recorded":
      return ev.positive ? "反馈：好 👍" : "反馈：不好 👎";
    case "batch_marked_suspended":
      return "批次任务挂起";
    case "batch_failed":
      return `批次失败（${ev.failed_items} 项）：${String(ev.reason ?? "").slice(0, 40)}`;
    default:
      return ev.type;
  }
}

/// 从事件里提取 task id（多数事件带 task 字段；task_created 带完整对象）
export function evTaskId(ev: Record<string, unknown>): string | null {
  const t = ev["task"];
  if (typeof t === "string") return t;
  if (t && typeof t === "object" && typeof (t as { id?: unknown }).id === "string") {
    return (t as { id: string }).id;
  }
  return null;
}

/// 事件 → 任务状态更新映射（reducer 用）。
/// 返回对该任务 summary 的部分更新（null = 不影响任务卡，仅进事件流）。
export function applyEventToTask(
  env: Envelope,
  prev: TaskSummary | undefined
): Partial<TaskSummary> | null {
  const ev = env.event;
  switch (ev.type) {
    case "task_created": {
      const t = ev.task as { id: string; title: string };
      return {
        id: t.id,
        title: t.title,
        state: "queued" as WorkerState,
        round: 0,
        narrative: "已创建，等待 worker…",
        blocked_kind: null,
        suspend_reason: null,
        acceptance_failures: 0,
      };
    }
    case "task_started":
      return { state: "working", narrative: "开始执行…" };
    case "round_progress": {
      const summary = String(ev.summary ?? "");
      return {
        state: "working",
        round: (ev.round as number) ?? prev?.round ?? 0,
        narrative: summary || prev?.narrative,
      };
    }
    case "narrative_snapshot":
      return { narrative: String(ev.milestone ?? "") || prev?.narrative };
    case "goal_progress":
      if (ev.blocked_condition) {
        return {
          state: "blocked",
          blocked_kind: "goal_stalled",
          narrative: `目标受阻：${String(ev.blocked_condition).slice(0, 60)}`,
        };
      }
      return { round: (ev.round as number) ?? prev?.round ?? 0 };
    case "acceptance_gate_failed":
      return {
        state: "blocked",
        blocked_kind: "acceptance_failed",
        acceptance_failures: (ev.failures as number) ?? 0,
        narrative: `假完成被拦（第 ${ev.failures} 次）—— 已回滚重跑`,
      };
    case "acceptance_gate_passed":
      return { narrative: "验收通过" };
    case "rounds_exhausted":
      return {
        state: "blocked",
        blocked_kind: "rounds_exhausted",
        narrative: `${ev.rounds} 轮预算耗尽，等你决定`,
      };
    case "auto_recovery_exhausted":
      return {
        state: "blocked",
        blocked_kind: "infra",
        narrative: "自动恢复耗尽，等你决定",
      };
    case "suspended":
      return {
        state: "suspended",
        suspend_reason: String(ev.reason ?? ""),
        narrative: `挂起（${String(ev.reason ?? "")}）`,
      };
    case "resumed":
      return { state: "working", suspend_reason: null };
    case "task_completed":
      return { state: "done", narrative: `✓ ${String(ev.summary ?? "").slice(0, 60)}` };
    case "task_failed":
      return { state: "failed", narrative: `✗ ${String(ev.error ?? "").slice(0, 60)}` };
    case "task_cancelled":
      return { state: "cancelled", narrative: "已取消" };
    case "task_requeued":
      return {
        state: "queued",
        blocked_kind: null,
        narrative: "已重新排队（重试）…",
      };
    case "emergency_stopped":
      return null; // 全局事件，无单任务字段（由 store 特判横幅）
    default:
      return null;
  }
}

/// SuspendReason → 是否自动恢复（types.rs recovery_policy 的 UI 镜像）
export const AUTO_RESUME_REASONS = new Set([
  "network_lost",
  "provider_outage",
  "system_sleep",
]);
