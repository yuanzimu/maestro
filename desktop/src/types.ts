// maestro 协议 wire 类型的 TS 镜像（对齐 maestro-protocol events.rs / core.rs 响应形状）

export type WorkerState =
  | "queued"
  | "planning"
  | "working"
  | "blocked"
  | "suspended"
  | "done"
  | "failed"
  | "cancelled";

export type BlockedKind =
  | "permission"
  | "plan_approval"
  | "goal_stalled"
  | "acceptance_failed"
  | "infra"
  | "rounds_exhausted";

export type Priority = "info" | "warning" | "critical";

/// task_list / task_get 共用形状（core.rs TaskList/TaskGet）
export interface TaskSummary {
  id: string;
  title: string;
  state: WorkerState;
  round: number;
  narrative: string;
  blocked_kind: BlockedKind | null;
  suspend_reason: string | null;
  acceptance_failures: number;
}

export interface TaskDetail extends TaskSummary {
  worker: string | null;
  session_ref: string | null;
  checkpoint_ref: string | null;
  /// U5 结果卡：task_diff 定位 git 仓库（增量字段，daemon core.rs task_get）
  workdir: string | null;
}

/// Envelope（events.rs）—— event 用宽松 dict（tagged union 数十变体，
/// 前端只按 type 分发，字段按需读取）
export interface Envelope {
  seq: number;
  ts: number;
  priority: Priority;
  event: Record<string, unknown> & { type: string };
}

export interface ServerStatus {
  version: string;
  pid: number;
  uptime_secs: number;
  tasks_total: number;
  workers_active: number;
  event_seq: number;
}

/// task_ledger 汇总（core.rs api_ledger）
export interface LedgerSummary {
  task: string;
  rounds: number;
  wall_ms: number;
  ledger_entries: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  actual_cost_cents: number;
  counterfactual_cost_cents: number;
  saved_cents: number;
  compactions: number;
}

export interface CheckpointItem {
  seq: number;
  reason: string;
  ref: string;
  commit: string;
}

export interface InboxItem {
  task: string;
  kind: BlockedKind | null;
  title: string;
}

export interface WorkerItem {
  id: string;
  task: string;
  state: WorkerState;
  pid: number;
}

export interface TaskCreateResult {
  task: { id: string; title: string; workdir: string; created_at: number };
  worker?: string;
  queued?: boolean;
  reason?: string;
}

export interface EmergencyStopResult {
  frozen_workers: string[];
  suspended_tasks: string[];
  checkpoints: string[];
  sessions_preserved: boolean;
  freeze_ms: number;
}

export interface RollbackResult {
  rolled_back_to: string;
  pre_rollback: string;
}

/// task_diff command 返回（src-tauri commands.rs）：
/// baseline checkpoint → 当前工作区的变更（stat + patch）
export interface DiffSummary {
  available: boolean;
  reason?: string;
  baseline?: string;
  files?: number;
  insertions?: number;
  deletions?: number;
  patch?: string;
  truncated?: boolean;
}

/// 引擎状态（daemon_status command 返回）
export interface DaemonStatus {
  running: boolean;
  managed: boolean;
  version: string;
  pid: number;
  uptime_secs: number;
  event_seq: number;
}

export type WorkerMode = "demo" | "claude" | "custom";

export interface Settings {
  worker_mode: WorkerMode;
  custom_program: string;
  custom_args: string;
}

export interface WorkerProbe {
  mode: WorkerMode;
  claude_found: boolean;
  daemon_path: string;
  rounder_path: string;
  mock_path: string;
  data_dir: string;
}
