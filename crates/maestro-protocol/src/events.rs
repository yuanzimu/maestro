//! 事件类型：事件溯源的核心。所有状态转移都是事件，状态由重放派生。
//! 对应设计文档 §2.3（Suspend/Resume）、§3.2（Emergency*）、UX 埋点（叙事/反馈/steering/断点/priority）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::*;

/// 事件信封：持久化与推送的统一格式。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Envelope {
    /// 单调递增序号（EventHub 断点续订的游标）
    pub seq: u64,
    /// Unix 毫秒
    pub ts: TsMs,
    /// 通知优先级（U9 埋点：Critical 才弹系统通知）
    pub priority: Priority,
    /// 事件负载
    pub event: Event,
}

impl Envelope {
    pub fn new(seq: u64, event: Event) -> Self {
        let priority = event.default_priority();
        Self {
            seq,
            ts: now_ms(),
            priority,
            event,
        }
    }
}

/// 事件负载。`#[serde(tag = "type", rename_all = "snake_case")]` 保持 wire 格式稳定。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    // ---- 生命周期 ----
    TaskCreated {
        task: Task,
        prompt: String,
    },
    TaskStarted {
        task: TaskId,
        worker: WorkerId,
    },
    TaskCompleted {
        task: TaskId,
        worker: WorkerId,
        summary: String,
    },
    TaskFailed {
        task: TaskId,
        worker: WorkerId,
        error: String,
    },
    TaskCancelled {
        task: TaskId,
        worker: WorkerId,
    },
    /// blocked 任务经用户确认后重新入队（验收失败/基建故障的重试路径）
    TaskRequeued {
        task: TaskId,
        from_kind: Option<BlockedKind>,
    },
    WorkerSpawned {
        worker: WorkerId,
        task: TaskId,
        pid: u32,
        pgid: u32,
    },
    WorkerDied {
        worker: WorkerId,
        exit_code: Option<i32>,
    },

    // ---- Goal / 验收门 ----
    GoalProgress {
        task: TaskId,
        round: u32,
        blocked_condition: Option<String>,
    },
    AcceptanceGatePassed {
        task: TaskId,
        round: u32,
        output: String,
    },
    AcceptanceGateFailed {
        task: TaskId,
        round: u32,
        failures: u32,
        output: String,
    },

    // ---- 挂起/恢复（设计 §2.3）----
    Suspended {
        task: TaskId,
        worker: WorkerId,
        reason: SuspendReason,
        session_ref: SessionRef,
        checkpoint_ref: CheckpointRef,
        round: u32,
    },
    Resumed {
        task: TaskId,
        worker: WorkerId,
        from_reason: SuspendReason,
        via: ResumeVia,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_session_ref: Option<SessionRef>,
    },
    /// 自动恢复尝试（退避计时到点的一次探测/续跑尝试）
    ResumeAttempt {
        task: TaskId,
        attempt: u32,
        next_backoff_secs: u64,
    },
    /// 自动恢复耗尽，升级 blocked(infra)
    AutoRecoveryExhausted {
        task: TaskId,
        attempts: u32,
    },

    // ---- 急停（设计 §3.2）----
    EmergencyStopped {
        workers: Vec<WorkerId>,
        reason: String,
    },
    EmergencySnapshotted {
        per_task: Vec<EmergencySnapshot>,
    },
    /// 冻结期间被丢弃的轻推（hold 模式，不静默 —— 用例 B7）
    SteeringDropped {
        task: TaskId,
        message: String,
    },

    // ---- steering（UX 埋点）----
    SteeringQueued {
        task: TaskId,
        message: String,
    },
    SteeringDelivered {
        task: TaskId,
        round: u32,
        message: String,
    },

    // ---- checkpoint（UX 埋点：断点状态）----
    CheckpointCreated {
        task: TaskId,
        cp: CheckpointRef,
        meta: CheckpointMeta,
    },
    CheckpointRolledBack {
        task: TaskId,
        to: CheckpointRef,
        pre_rollback: CheckpointRef,
    },

    // ---- 叙事（UX 埋点：叙事快照）----
    NarrativeSnapshot {
        task: TaskId,
        round: u32,
        milestone: String,
    },

    // ---- 反馈（UX 埋点）----
    FeedbackRecorded {
        task: TaskId,
        positive: bool,
        reason: Option<String>,
    },

    // ---- 账本（T4）----
    LedgerEntry {
        task: TaskId,
        worker: Option<WorkerId>,
        usage: UsageEntry,
    },

    /// 轮进度（0.15 多轮驱动 / U3 叙事）：每轮一条 —— 工具调用 + 回答摘要，
    /// 事件流订阅方（CLI/Desktop UI）实时渲染「它正在干什么」
    RoundProgress {
        task: TaskId,
        round: u32,
        /// 本轮工具调用名（去重）
        tools_used: Vec<String>,
        /// 回答摘要（前 120 字符 —— 全文太长，不进事件流）
        summary: String,
        tokens_in: u64,
        tokens_out: u64,
    },
    /// 费用对账漂移（R34，T4 增量对账）：CLI 自报 total_cost_usd 与
    /// daemon 牌价计费差超 25% —— 牌价表过期 / CLI usage 口径变化的信号
    CostDrift {
        task: TaskId,
        round: u32,
        model: String,
        /// daemon 侧计费（美分）
        ledger_cents: u64,
        /// CLI 自报折算（美分）
        cli_cents: u64,
    },
    /// 上下文压缩（R37 轮转，U3 叙事）：轮边界占用超阈值 → 注入
    /// /compact 指令轮完成压缩 ——「上下文已压缩，任务继续」
    ContextCompacted {
        task: TaskId,
        round: u32,
    },

    // ---- provider / batch（U11/T6）----
    ProviderSwitched {
        task: Option<TaskId>,
        from: String,
        to: String,
        cause: String,
    },
    BatchMarkedSuspended {
        batch: BatchId,
        task: TaskId,
    },
    BatchFailed {
        batch: BatchId,
        failed_items: u32,
        reason: String,
    },
}

/// 单条急停快照记录
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmergencySnapshot {
    pub task: TaskId,
    pub checkpoint_ref: CheckpointRef,
    pub session_ref: SessionRef,
    pub round: u32,
}

/// 任务描述
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    /// 工作目录（worktree 路径）
    pub workdir: String,
    pub created_at: TsMs,
}

/// token 用量（账本条目）
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UsageEntry {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// 写 cache 的 token（Anthropic 单独计价 1.25x/2x 输入价；OpenAI 大多免费）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    /// 执行路径（interactive/offpeak/batch/batch_fallback/offpeak_promote）
    pub path: Option<String>,
    /// 该段折扣（1.0 = 无折扣）
    pub discount: Option<f64>,
    /// 反事实成本：同内容牌价直跑的估算（美分）
    pub counterfactual_cost_cents: Option<u64>,
    /// 实际成本（美分）
    pub actual_cost_cents: Option<u64>,
}

impl Event {
    /// 事件默认通知优先级（U9 分级规则）。
    /// Critical = blocked/failed/急停/预算触顶；Warning = 升级/切换/丢弃；Info = 其余。
    pub fn default_priority(&self) -> Priority {
        use Event::*;
        match self {
            // 需要用户立即知道或行动
            TaskFailed { .. }
            | AutoRecoveryExhausted { .. }
            | EmergencyStopped { .. }
            | BatchFailed { .. }
            | AcceptanceGateFailed { .. } => Priority::Critical,

            // blocked 语义的等待项（Goal 3 轮、审批门）也是 Critical
            GoalProgress {
                blocked_condition: Some(_),
                ..
            } => Priority::Critical,

            // 值得关注但不打扰
            ProviderSwitched { .. }
            | SteeringDropped { .. }
            | ResumeAttempt { .. }
            | AcceptanceGatePassed { .. }
            | CostDrift { .. }
            | Suspended { .. } => Priority::Warning,

            // done 结果卡也弹通知（U9 三类之一）
            TaskCompleted { .. } => Priority::Critical,

            // 其余进活动流
            _ => Priority::Info,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wire 格式稳定性测试：serde tag 形状（协议冻结后不可变）
    #[test]
    fn event_wire_format() {
        let env = Envelope::new(
            1,
            Event::Suspended {
                task: TaskId::new("t1"),
                worker: WorkerId::new("w1"),
                reason: SuspendReason::NetworkLost,
                session_ref: SessionRef::new("sess-abc"),
                checkpoint_ref: CheckpointRef::new("refs/maestro/cp/t1/0003-round_start"),
                round: 7,
            },
        );
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["event"]["type"], "suspended");
        assert_eq!(json["event"]["reason"], "network_lost");
        assert_eq!(json["priority"], "warning");
        assert_eq!(json["seq"], 1);
    }

    /// 优先级分级规则（U9）
    #[test]
    fn priority_classification() {
        assert_eq!(
            Event::TaskFailed {
                task: TaskId::new("t"),
                worker: WorkerId::new("w"),
                error: "x".into()
            }
            .default_priority(),
            Priority::Critical
        );
        assert_eq!(
            Event::GoalProgress {
                task: TaskId::new("t"),
                round: 3,
                blocked_condition: Some("等权限".into())
            }
            .default_priority(),
            Priority::Critical
        );
        assert_eq!(
            Event::GoalProgress {
                task: TaskId::new("t"),
                round: 1,
                blocked_condition: None
            }
            .default_priority(),
            Priority::Info
        );
        assert_eq!(
            Event::TaskCompleted {
                task: TaskId::new("t"),
                worker: WorkerId::new("w"),
                summary: "ok".into()
            }
            .default_priority(),
            Priority::Critical
        );
    }

    /// 断连事件永不 Critical（suspended ≠ failed 的事件层体现）
    #[test]
    fn suspended_is_warning_not_critical() {
        let e = Event::Suspended {
            task: TaskId::new("t"),
            worker: WorkerId::new("w"),
            reason: SuspendReason::NetworkLost,
            session_ref: SessionRef::new("s"),
            checkpoint_ref: CheckpointRef::new("cp"),
            round: 1,
        };
        assert_eq!(e.default_priority(), Priority::Warning);
    }
}
