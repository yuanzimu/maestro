// invoke 薄封装：每个 tauri command 一个 typed 函数。
// 后端把 ClientError/RPC 错误统一格式化为 "RPC -409: xxx" 字符串。

import { invoke } from "@tauri-apps/api/core";
import type {
  CheckpointItem,
  DaemonStatus,
  DiffRevertResult,
  DiffSummary,
  EmergencyStopResult,
  InboxItem,
  LedgerSummary,
  RollbackResult,
  Settings,
  TaskCreateResult,
  TaskDetail,
  TaskSummary,
  WorkerItem,
  WorkerProbe,
} from "./types";

export const getDaemonStatus = () =>
  invoke<DaemonStatus>("daemon_status");

export const restartDaemon = () => invoke<void>("restart_daemon");
export const shutdownDaemon = () => invoke<void>("shutdown_daemon");

export const createTask = (title: string, prompt: string, workdir?: string) =>
  invoke<TaskCreateResult>("create_task", { title, prompt, workdir: workdir || null });

export const listTasks = () =>
  invoke<TaskSummary[]>("list_tasks");

export const getTask = (id: string) =>
  invoke<TaskDetail>("get_task", { id });

export const pauseTask = (id: string) => invoke<void>("pause_task", { id });
export const resumeTask = (id: string) => invoke<void>("resume_task", { id });
export const cancelTask = (id: string) => invoke<void>("cancel_task", { id });

export const steerTask = (id: string, message: string) =>
  invoke<{ queued: boolean; seq: number }>("steer_task", { id, message });

export const getLedger = (id: string) =>
  invoke<LedgerSummary>("get_ledger", { id });

export const listWorkers = () =>
  invoke<WorkerItem[]>("list_workers");

export const listInbox = () =>
  invoke<InboxItem[]>("list_inbox");

export const emergencyStop = (reason?: string) =>
  invoke<EmergencyStopResult>("emergency_stop", { reason: reason || null });

export const resumeAll = (steering: "flush" | "hold") =>
  invoke<{ resumed: number }>("resume_all", { steering });

// daemon wire 为 { checkpoints: [...] } 包装（core.rs CheckpointList），
// 此处解包 —— 否则 TaskDetail 渲染 checkpoints.map 直接 TypeError 黑屏
export const listCheckpoints = (task: string) =>
  invoke<{ checkpoints: CheckpointItem[] }>("list_checkpoints", { task }).then(
    (r) => r.checkpoints ?? []
  );

export const rollbackCheckpoint = (task: string, to: string) =>
  invoke<RollbackResult>("rollback_checkpoint", { task, to });

// U5 结果卡：变更明细（baseline checkpoint → 工作区，按需加载）
export const taskDiff = (id: string) =>
  invoke<DiffSummary>("task_diff", { id });

// C2 hunk 级部分接受：拒绝所选 hunk（撤销 + 自动转修正任务一条龙）。
// patch 必须传「用户所见」的 diff 原文 —— 后端按它拼拒绝子 patch 并
// reverse-apply；所见原文与工作区不一致时 git apply 原子失败（防静默错撤）
export const taskDiffRevert = (
  id: string,
  hunks: number[],
  reason: string,
  patch: string
) => invoke<DiffRevertResult>("task_diff_revert", { id, hunks, reason, patch });

// U7 v1 反馈闭环：结果卡 👍/👎 + 理由（👎 必填）→ 事件 + 项目记忆
export const taskFeedback = (id: string, positive: boolean, reason?: string) =>
  invoke<{ recorded: boolean }>("task_feedback", {
    id,
    positive,
    reason: reason || null,
  });

export const getSettings = () => invoke<Settings>("get_settings");
export const saveSettings = (settings: Settings) =>
  invoke<void>("save_settings", { settings });

export const probeWorker = () => invoke<WorkerProbe>("probe_worker");
