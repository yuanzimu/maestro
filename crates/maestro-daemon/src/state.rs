//! Authority：权威状态。**只存原子事实，派生态从不落库**（同类项目 OBSERVE→UPDATE→DERIVE 的教训）。
//! 状态由事件重放派生（用例 A9：重放结果与在线一致）。

use maestro_protocol::events::{Envelope, Event, Task};
use maestro_protocol::types::*;
use std::collections::HashMap;

/// 任务记录（在线态；崩溃后从事件流重放重建）
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task: Task,
    pub prompt: String,
    pub worker: Option<WorkerId>,
    pub state: WorkerState,
    /// blocked 分类（state==Blocked 时有效）
    pub blocked_kind: Option<BlockedKind>,
    /// 挂起信息（state==Suspended 时有效）
    pub suspend: Option<SuspendInfo>,
    /// 当前轮次（多轮驱动）
    pub round: u32,
    /// CLI 会话引用（--resume 用）
    pub session_ref: Option<SessionRef>,
    /// 最近 checkpoint
    pub checkpoint_ref: Option<CheckpointRef>,
    /// 自动恢复已尝试次数（退避状态）
    pub resume_attempts: u32,
}

/// 挂起信息
#[derive(Debug, Clone)]
pub struct SuspendInfo {
    pub reason: SuspendReason,
    pub since_ts: u64,
}

impl TaskRecord {
    pub fn is_suspended_auto(&self) -> bool {
        self.suspend
            .as_ref()
            .map(|s| s.reason.recovery_policy() == RecoveryPolicy::Auto)
            .unwrap_or(false)
    }
}

/// Worker 记录
#[derive(Debug, Clone)]
pub struct WorkerRecord {
    pub id: WorkerId,
    pub task: TaskId,
    pub pid: u32,
    pub pgid: u32,
    pub state: WorkerState,
}

/// 权威状态：Core 独占（单线程，无锁）
#[derive(Default)]
pub struct Authority {
    pub tasks: HashMap<TaskId, TaskRecord>,
    pub workers: HashMap<WorkerId, WorkerRecord>,
}

impl Authority {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从事件流重建（daemon 重启 / 用例 A9）
    pub fn replay(events: &[Envelope]) -> Self {
        let mut a = Authority::new();
        for env in events {
            a.apply(&env.event, env.ts);
        }
        a
    }

    /// 应用单个事件（在线转移与重放共用同一段代码 —— 单一事实源）
    pub fn apply(&mut self, event: &Event, ts: u64) {
        use Event::*;
        match event {
            TaskCreated { task, prompt } => {
                self.tasks.insert(
                    task.id.clone(),
                    TaskRecord {
                        task: task.clone(),
                        prompt: prompt.clone(),
                        worker: None,
                        state: WorkerState::Queued,
                        blocked_kind: None,
                        suspend: None,
                        round: 0,
                        session_ref: None,
                        checkpoint_ref: None,
                        resume_attempts: 0,
                    },
                );
            }
            TaskStarted { task, worker } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    t.state = WorkerState::Working;
                    t.worker = Some(worker.clone());
                    t.blocked_kind = None;
                    t.suspend = None;
                }
                if let Some(w) = self.workers.get_mut(worker) {
                    w.state = WorkerState::Working;
                }
            }
            WorkerSpawned {
                worker,
                task,
                pid,
                pgid,
            } => {
                self.workers.insert(
                    worker.clone(),
                    WorkerRecord {
                        id: worker.clone(),
                        task: task.clone(),
                        pid: *pid,
                        pgid: *pgid,
                        state: WorkerState::Working,
                    },
                );
                if let Some(t) = self.tasks.get_mut(task) {
                    t.worker = Some(worker.clone());
                    t.state = WorkerState::Working;
                }
            }
            WorkerDied { worker, .. } => {
                if let Some(w) = self.workers.get_mut(worker) {
                    // 死亡不必然终结任务：suspended 恢复路径里 CLI 退出是预期内
                    if w.state == WorkerState::Working {
                        w.state = WorkerState::Failed;
                    }
                }
            }
            TaskCompleted { task, .. } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    t.state = WorkerState::Done;
                }
            }
            TaskFailed { task, .. } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    t.state = WorkerState::Failed;
                }
            }
            TaskCancelled { task, .. } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    t.state = WorkerState::Cancelled;
                }
            }
            Suspended {
                task,
                worker,
                reason,
                session_ref,
                checkpoint_ref,
                round,
            } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    t.state = WorkerState::Suspended;
                    t.suspend = Some(SuspendInfo {
                        reason: *reason,
                        since_ts: ts,
                    });
                    t.session_ref = Some(session_ref.clone());
                    t.checkpoint_ref = Some(checkpoint_ref.clone());
                    t.round = *round;
                    // 换挂起原因时重置退避计数
                    t.resume_attempts = 0;
                }
                if let Some(w) = self.workers.get_mut(worker) {
                    w.state = WorkerState::Suspended;
                }
            }
            Resumed {
                task,
                worker,
                new_session_ref,
                ..
            } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    t.state = WorkerState::Working;
                    t.suspend = None;
                    if let Some(ns) = new_session_ref {
                        t.session_ref = Some(ns.clone());
                    }
                }
                if let Some(w) = self.workers.get_mut(worker) {
                    w.state = WorkerState::Working;
                }
            }
            ResumeAttempt { task, attempt, .. } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    t.resume_attempts = *attempt;
                }
            }
            AutoRecoveryExhausted { task, .. } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    t.state = WorkerState::Blocked;
                    t.blocked_kind = Some(BlockedKind::Infra);
                    t.suspend = None;
                }
            }
            AcceptanceGateFailed { task, failures, .. } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    if *failures >= 3 {
                        t.state = WorkerState::Blocked;
                        t.blocked_kind = Some(BlockedKind::AcceptanceFailed);
                    }
                }
            }
            CheckpointCreated { task, cp, meta } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    t.checkpoint_ref = Some(cp.clone());
                    t.round = meta.round;
                }
            }
            SteeringDelivered { task, round, .. } => {
                if let Some(t) = self.tasks.get_mut(task) {
                    t.round = *round;
                }
            }
            // 与状态无关的事件（叙事/反馈/账本/急停快照…）—— 未来 UI 消费
            _ => {}
        }
    }

    pub fn get(&self, id: &TaskId) -> Option<&TaskRecord> {
        self.tasks.get(id)
    }

    /// 挂起中的自动恢复任务清单（恢复调度器扫描用）
    pub fn auto_resume_candidates(&self) -> Vec<(TaskId, u32, SuspendReason)> {
        self.tasks
            .values()
            .filter(|t| t.is_suspended_auto())
            .map(|t| {
                (
                    t.task.id.clone(),
                    t.resume_attempts,
                    t.suspend.as_ref().unwrap().reason,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_protocol::events::Task;

    fn task(id: &str) -> Task {
        Task {
            id: TaskId::new(id),
            title: "t".into(),
            workdir: "/tmp".into(),
            created_at: 0,
        }
    }

    /// 用例 A9（核心断言）：状态由事件重放派生，重放与在线一致
    #[test]
    fn replay_matches_live() {
        let mut live = Authority::new();
        let events = vec![
            Event::TaskCreated {
                task: task("t1"),
                prompt: "p".into(),
            },
            Event::WorkerSpawned {
                worker: WorkerId::new("w1"),
                task: TaskId::new("t1"),
                pid: 123,
                pgid: 123,
            },
            Event::Suspended {
                task: TaskId::new("t1"),
                worker: WorkerId::new("w1"),
                reason: SuspendReason::NetworkLost,
                session_ref: SessionRef::new("s1"),
                checkpoint_ref: CheckpointRef::new("cp1"),
                round: 3,
            },
        ];
        for e in &events {
            live.apply(e, 0);
        }
        let replayed = Authority::replay(
            &events
                .iter()
                .enumerate()
                .map(|(i, e)| Envelope::new(i as u64 + 1, e.clone()))
                .collect::<Vec<_>>(),
        );

        let a = live.get(&TaskId::new("t1")).unwrap();
        let b = replayed.get(&TaskId::new("t1")).unwrap();
        assert_eq!(a.state, WorkerState::Suspended);
        assert_eq!(a.state, b.state);
        assert_eq!(a.session_ref, b.session_ref);
        assert_eq!(a.round, b.round);
        assert_eq!(
            a.suspend.as_ref().unwrap().reason,
            SuspendReason::NetworkLost
        );
        // 挂起原因换过 → 退避计数清零（A8/Suspended 联动）
        assert_eq!(a.resume_attempts, 0);
    }

    /// 自动恢复候选只含 auto 策略的挂起任务（A1 联动）
    #[test]
    fn auto_resume_candidates_filter() {
        let mut a = Authority::new();
        a.apply(
            &Event::TaskCreated {
                task: task("t1"),
                prompt: "p".into(),
            },
            0,
        );
        a.apply(
            &Event::TaskCreated {
                task: task("t2"),
                prompt: "p".into(),
            },
            0,
        );
        a.apply(
            &Event::Suspended {
                task: TaskId::new("t1"),
                worker: WorkerId::new("w1"),
                reason: SuspendReason::NetworkLost,
                session_ref: SessionRef::new("s"),
                checkpoint_ref: CheckpointRef::new("c"),
                round: 1,
            },
            0,
        );
        a.apply(
            &Event::Suspended {
                task: TaskId::new("t2"),
                worker: WorkerId::new("w2"),
                reason: SuspendReason::UserPause,
                session_ref: SessionRef::new("s"),
                checkpoint_ref: CheckpointRef::new("c"),
                round: 1,
            },
            0,
        );
        let cands = a.auto_resume_candidates();
        assert_eq!(cands.len(), 1, "只有 NetworkLost 是 auto 候选");
        assert_eq!(cands[0].0, TaskId::new("t1"));
    }

    /// 验收三次失败 → blocked(AcceptanceFailed)（Goal 3 轮语义的一部分）
    #[test]
    fn acceptance_three_strikes_blocks() {
        let mut a = Authority::new();
        a.apply(
            &Event::TaskCreated {
                task: task("t1"),
                prompt: "p".into(),
            },
            0,
        );
        for i in 1..=3u32 {
            a.apply(
                &Event::AcceptanceGateFailed {
                    task: TaskId::new("t1"),
                    round: i,
                    failures: i,
                    output: "x".into(),
                },
                0,
            );
        }
        let t = a.get(&TaskId::new("t1")).unwrap();
        assert_eq!(t.state, WorkerState::Blocked);
        assert_eq!(t.blocked_kind, Some(BlockedKind::AcceptanceFailed));
    }
}
