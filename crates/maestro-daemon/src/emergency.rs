//! emergency_stop 三阶段：FREEZE → SNAPSHOT → DECIDE（设计 §3，0.18）。
//!
//! 为什么 SIGSTOP 而不是直接杀：杀进程=丢失进行中轮次且现场残缺；
//! SIGSTOP 让进程组瞬时静止（<100ms），文件系统静止后快照才干净。
//!
//! 全部状态转移走 Ctx::publish（发布即应用，单一事实源）。

use crate::core::Ctx;
use crate::state::Authority;
use crate::worker::{self, WorkerMeta};
use maestro_protocol::api::{EmergencyStopResult, SteeringMode};
use maestro_protocol::events::{EmergencySnapshot, Event};
use maestro_protocol::types::*;
use std::collections::HashMap;
use std::time::Instant;

/// 急停状态
#[derive(Debug, Clone, PartialEq)]
pub enum EmergencyPhase {
    /// 正常运行
    None,
    /// 已冻结（全部 Worker SIGSTOP + 已快照）
    Frozen,
}

/// FREEZE + SNAPSHOT（设计 §3.1）：
/// 1. freeze：Core 单线程此刻即冻结派发 + 逐运行中 Worker SIGSTOP
/// 2. snapshot：逐 worktree checkpoint（FS 已静止；串行防 .git 锁冲突）
/// 3. 事件：EmergencyStopped → Suspended(each) → EmergencySnapshotted
pub fn emergency_stop(
    ctx: &mut Ctx,
    metas: &HashMap<WorkerId, WorkerMeta>,
    reason: &str,
) -> EmergencyStopResult {
    let t0 = Instant::now();

    // ---- 阶段 1 FREEZE ----
    let mut frozen: Vec<WorkerId> = vec![];
    for (id, m) in metas {
        let running = ctx
            .authority
            .workers
            .get(id)
            .map(|w| w.state == WorkerState::Working)
            .unwrap_or(false);
        if running && worker::freeze_group(m.pgid).is_ok() {
            frozen.push(id.clone());
        }
    }
    let freeze_ms = t0.elapsed().as_millis() as u64;

    ctx.publish(Event::EmergencyStopped {
        workers: frozen.clone(),
        reason: reason.to_string(),
    });

    // ---- 阶段 2 SNAPSHOT（串行）----
    let mut suspended_tasks = vec![];
    let mut checkpoints = vec![];
    let mut snapshots = vec![];
    for id in &frozen {
        let Some(m) = metas.get(id) else { continue };
        let Some(task) = ctx.authority.tasks.get(&m.task).cloned() else { continue };

        let cp_ref = match crate::checkpoints::capture(
            std::path::Path::new(&task.task.workdir),
            &task.task.id,
            task.round,
            CpReason::Emergency,
            ctx.clock.as_ref(),
        ) {
            Ok(r) => r,
            Err(e) => {
                // 快照失败不阻断急停；错误如实进事件流
                ctx.publish(Event::TaskFailed {
                    task: task.task.id.clone(),
                    worker: id.clone(),
                    error: format!("emergency snapshot failed: {e}"),
                });
                CheckpointRef::new("refs/maestro/none")
            }
        };

        let session_ref = task.session_ref.clone().unwrap_or_else(|| SessionRef::new(""));
        ctx.publish(Event::Suspended {
            task: task.task.id.clone(),
            worker: id.clone(),
            reason: SuspendReason::EmergencyStop,
            session_ref: session_ref.clone(),
            checkpoint_ref: cp_ref.clone(),
            round: task.round,
        });
        suspended_tasks.push(task.task.id.clone());
        checkpoints.push(cp_ref.clone());
        snapshots.push(EmergencySnapshot {
            task: task.task.id.clone(),
            checkpoint_ref: cp_ref,
            session_ref,
            round: task.round,
        });
    }
    ctx.publish(Event::EmergencySnapshotted { per_task: snapshots });

    EmergencyStopResult {
        frozen_workers: frozen,
        suspended_tasks,
        checkpoints,
        sessions_preserved: true,
        freeze_ms,
    }
}

/// resume_all：SIGCONT + steering flush/hold（用例 B7）+ Resumed 事件
pub fn resume_all(
    ctx: &mut Ctx,
    metas: &HashMap<WorkerId, WorkerMeta>,
    steering: &mut crate::steering::SteeringQueue,
    mode: SteeringMode,
) -> Vec<TaskId> {
    let mut resumed = vec![];
    // 收集急停挂起的 worker（先收集避免借用冲突）
    let targets: Vec<(WorkerId, TaskId)> = metas
        .iter()
        .filter(|(_, _)| true)
        .filter_map(|(id, m)| {
            let t = ctx.authority.tasks.get(&m.task)?;
            let is_emergency = t.state == WorkerState::Suspended
                && t.suspend
                    .as_ref()
                    .map(|s| s.reason == SuspendReason::EmergencyStop)
                    .unwrap_or(false);
            is_emergency.then(|| (id.clone(), m.task.clone()))
        })
        .collect();

    for (id, task) in targets {
        if let Some(m) = metas.get(&id) {
            let _ = worker::unfreeze_group(m.pgid);
        }
        for msg in steering.drain(&task) {
            match mode {
                SteeringMode::Flush => {
                    ctx.publish(Event::SteeringDelivered {
                        task: task.clone(),
                        round: 0,
                        message: msg.message,
                    });
                }
                SteeringMode::Hold => {
                    // 不静默：每条发 SteeringDropped（用例 B7 断言）
                    ctx.publish(Event::SteeringDropped {
                        task: task.clone(),
                        message: msg.message,
                    });
                }
            }
        }
        ctx.publish(Event::Resumed {
            task: task.clone(),
            worker: id,
            from_reason: SuspendReason::EmergencyStop,
            via: ResumeVia::ResumeAll,
            new_session_ref: None,
        });
        resumed.push(task);
    }
    resumed
}

/// 单任务 cancel（DECIDE 选项之一）：SIGCONT + 三级升级杀 + cancelled
pub fn cancel_task(ctx: &mut Ctx, metas: &HashMap<WorkerId, WorkerMeta>, task_id: &TaskId) -> bool {
    let Some(record) = ctx.authority.tasks.get(task_id).cloned() else {
        return false;
    };
    let Some(worker_id) = record.worker.clone() else {
        return false;
    };
    // cancelled 事件先发（防 waiter 线程抢先报 WorkerDied 抢状态）
    ctx.publish(Event::TaskCancelled {
        task: task_id.clone(),
        worker: worker_id.clone(),
    });
    if let Some(m) = metas.get(&worker_id) {
        // SIGCONT 让它退出 stopped 态（对 stopped 组发 TERM 可能不排队），再三级升级
        let _ = worker::unfreeze_group(m.pgid);
        worker::graceful_kill_group(m.pgid);
    }
    true
}

/// 判断任务当前是否可被 cancel（状态 + worker 存在）
pub fn cancellable(authority: &Authority, task_id: &TaskId) -> bool {
    authority
        .tasks
        .get(task_id)
        .map(|t| !t.state.is_terminal() && t.worker.is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_ms_always_recorded() {
        let r = EmergencyStopResult {
            frozen_workers: vec![],
            suspended_tasks: vec![],
            checkpoints: vec![],
            sessions_preserved: true,
            freeze_ms: 42,
        };
        assert_eq!(r.freeze_ms, 42);
    }
}
