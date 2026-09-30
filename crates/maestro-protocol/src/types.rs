//! 基础数据模型：ID、状态、枚举。
//! 对应设计文档 U10_T6_DESIGN.md §2（suspended 状态机）与 §3（急停协议）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ID 类型 —— 全部用 String 承载（序列化友好），newtype 保证不混用。
// ---------------------------------------------------------------------------

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
        )]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(TaskId, "任务唯一标识");
id_type!(WorkerId, "Worker 唯一标识");
id_type!(
    SessionRef,
    "CLI 会话引用（--resume 用，永远当 argv 数据不当 shell 文本）"
);
id_type!(
    CheckpointRef,
    "checkpoint 引用（refs/maestro/cp/<task>/<seq>）"
);
id_type!(BatchId, "provider batch 标识");

// ---------------------------------------------------------------------------
// 任务/Worker 状态
// ---------------------------------------------------------------------------

/// Worker 运行状态。见设计 §2.1 状态图。
/// 关键语义：`failed` 仅保留给不可恢复错误；断连/断供是 `suspended` 而非 `failed`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    /// 已入队未启动
    Queued,
    /// 规划中
    Planning,
    /// 执行中
    Working,
    /// 等待用户决策（权限/方案选择/验收三次失败）
    Blocked,
    /// 执行暂停（基础设施/人为），恢复策略由 SuspendReason 决定
    Suspended,
    /// 完成（验收门通过）
    Done,
    /// 不可恢复失败（重试预算耗尽/配置错误/验收结构性失败）
    Failed,
    /// 用户取消
    Cancelled,
}

impl WorkerState {
    /// 终态判定（不再发生转移）
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            WorkerState::Done | WorkerState::Failed | WorkerState::Cancelled
        )
    }
}

/// 挂起原因。见设计 §2.2 自动恢复矩阵。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SuspendReason {
    /// 用户手动暂停任务 —— 仅手动恢复
    UserPause,
    /// 全局急停波及 —— 仅手动恢复（resume_all/task.resume）
    EmergencyStop,
    /// 网络不可达 —— 自动恢复（退避 30s→5m，10 次后升 blocked）
    NetworkLost,
    /// 供应商 429/5xx 风暴（熔断打开）—— 自动恢复
    ProviderOutage,
    /// Worker 预算耗尽 —— 仅手动恢复（提额或批准）
    BudgetExceeded,
    /// 宿主机休眠/唤醒 —— 自动恢复
    SystemSleep,
    /// daemon 异常退出时的孤儿清理路径 —— 仅手动恢复
    DaemonCrash,
}

impl SuspendReason {
    /// 恢复策略：自动 or 仅手动。未知 reason 兜底 manual（安全默认，用例 A1）。
    pub fn recovery_policy(self) -> RecoveryPolicy {
        match self {
            SuspendReason::NetworkLost
            | SuspendReason::ProviderOutage
            | SuspendReason::SystemSleep => RecoveryPolicy::Auto,
            SuspendReason::UserPause
            | SuspendReason::EmergencyStop
            | SuspendReason::BudgetExceeded
            | SuspendReason::DaemonCrash => RecoveryPolicy::Manual,
        }
    }
}

/// 恢复策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPolicy {
    /// 自动恢复（退避重试）
    Auto,
    /// 仅手动恢复（task.resume / server.resume_all）
    Manual,
}

/// 恢复途径（ResumeEvent.via）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResumeVia {
    /// 自动恢复（定时器/网络恢复/唤醒）
    Auto,
    /// 用户手动 task.resume
    User,
    /// server.resume_all
    ResumeAll,
}

/// 挂起时的自动恢复退避表（秒）。索引 = 已尝试次数。
/// 30s → 1m → 2m → 5m → 5m（封顶）。见设计 §2.2 与用例 A2。
pub const RESUME_BACKOFF_SECS: &[u64] = &[30, 60, 120, 300];

/// 退避表查询：第 n 次尝试（从 0 起）等待多少秒。
pub fn backoff_secs(attempt: usize) -> u64 {
    if RESUME_BACKOFF_SECS.is_empty() {
        30
    } else {
        RESUME_BACKOFF_SECS
            .get(attempt)
            .copied()
            .unwrap_or(*RESUME_BACKOFF_SECS.last().unwrap())
    }
}

/// 自动恢复尝试上限：10 次（约 30 分钟）后升级 blocked(infra)。
pub const MAX_AUTO_RESUME_ATTEMPTS: u32 = 10;

// ---------------------------------------------------------------------------
// blocked 子类型（区分「等用户决策」的不同来源）
// ---------------------------------------------------------------------------

/// blocked 等待内容的分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockedKind {
    /// 权限请求（工具调用需批准）
    Permission,
    /// 方案选择（Plan/Spec 审批门）
    PlanApproval,
    /// Goal 3 轮无进展自动停（同一阻塞条件连续 3 轮）
    GoalStalled,
    /// 验收门三次失败
    AcceptanceFailed,
    /// 基础设施持续不可达（自动恢复 10 次耗尽后升级）
    Infra,
}

// ---------------------------------------------------------------------------
// 通知优先级（U9 通知治理：事件 schema 的 priority 埋点）
// ---------------------------------------------------------------------------

/// 事件/通知优先级。仅 Critical 级弹系统通知，其余进活动流。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// 普通事件（进活动流，digest 批处理）
    Info,
    /// 值得关注但不打扰
    Warning,
    /// 弹系统通知：blocked（需要你）/ done（结果卡）/ failed / 预算触顶
    Critical,
}

// ---------------------------------------------------------------------------
// checkpoint
// ---------------------------------------------------------------------------

/// checkpoint 创建原因。见设计 §4.2。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CpReason {
    /// 任务起点（永久保留）
    Baseline,
    /// 每轮开始前（滚动保留）
    RoundStart,
    /// 急停时刻（§3 SNAPSHOT 阶段）
    Emergency,
    /// 手动创建
    Manual,
    /// 回滚前的安全垫（永久保留，保证回滚本身可撤销 —— 不变量 I4）
    PreRollback,
    /// 验收通过的点（永久保留）
    AcceptancePassed,
    /// 合并前（P2）
    PreMerge,
}

impl CpReason {
    /// 是否永久保留（否则滚动 GC）。见设计 §4.2 保留策略。
    pub fn is_pinned(self) -> bool {
        matches!(
            self,
            CpReason::Baseline
                | CpReason::AcceptancePassed
                | CpReason::PreMerge
                | CpReason::PreRollback
        )
    }
}

/// checkpoint 元数据（commit message 的 JSON 内容）
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointMeta {
    pub task: TaskId,
    pub round: u32,
    pub reason: CpReason,
    /// 父 checkpoint（时间线上一个）
    pub parent_cp: Option<CheckpointRef>,
    /// Unix 毫秒时间戳
    pub ts: u64,
}

// ---------------------------------------------------------------------------
// 时间戳：统一 Unix 毫秒（u64），与 MockClock 对齐
// ---------------------------------------------------------------------------

pub type TsMs = u64;

/// 生成时间戳的默认实现（生产用 SystemClock；测试用 MockClock 虚拟推进）
pub fn now_ms() -> TsMs {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 单元测试：状态机契约（用例组 A 的协议层部分）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 用例 A1（协议层）：reason → 恢复策略全表
    #[test]
    fn a1_reason_recovery_policy_matrix() {
        let auto = [
            SuspendReason::NetworkLost,
            SuspendReason::ProviderOutage,
            SuspendReason::SystemSleep,
        ];
        let manual = [
            SuspendReason::UserPause,
            SuspendReason::EmergencyStop,
            SuspendReason::BudgetExceeded,
            SuspendReason::DaemonCrash,
        ];
        for r in auto {
            assert_eq!(
                r.recovery_policy(),
                RecoveryPolicy::Auto,
                "{r:?} 应为自动恢复"
            );
        }
        for r in manual {
            assert_eq!(
                r.recovery_policy(),
                RecoveryPolicy::Manual,
                "{r:?} 应为仅手动恢复"
            );
        }
    }

    /// 用例 A2（协议层）：退避表形状 30→60→120→300→封顶
    #[test]
    fn a2_backoff_table_shape() {
        assert_eq!(backoff_secs(0), 30);
        assert_eq!(backoff_secs(1), 60);
        assert_eq!(backoff_secs(2), 120);
        assert_eq!(backoff_secs(3), 300);
        assert_eq!(backoff_secs(4), 300);
        assert_eq!(backoff_secs(99), 300, "封顶 5 分钟");
    }

    /// 保留策略：pinned 与滚动
    #[test]
    fn checkpoint_retention_policy() {
        assert!(CpReason::Baseline.is_pinned());
        assert!(CpReason::PreRollback.is_pinned());
        assert!(CpReason::AcceptancePassed.is_pinned());
        assert!(!CpReason::RoundStart.is_pinned());
        assert!(!CpReason::Emergency.is_pinned());
    }

    /// 终态判定
    #[test]
    fn terminal_states() {
        assert!(WorkerState::Done.is_terminal());
        assert!(WorkerState::Failed.is_terminal());
        assert!(WorkerState::Cancelled.is_terminal());
        assert!(!WorkerState::Suspended.is_terminal());
        assert!(!WorkerState::Working.is_terminal());
    }

    /// 序列化形状（协议冻结的一部分，防意外变更）
    #[test]
    fn serde_shapes() {
        let s = serde_json::to_string(&SuspendReason::NetworkLost).unwrap();
        assert_eq!(s, "\"network_lost\"");
        let s = serde_json::to_string(&Priority::Critical).unwrap();
        assert_eq!(s, "\"critical\"");
        let s = serde_json::to_string(&CpReason::PreRollback).unwrap();
        assert_eq!(s, "\"pre_rollback\"");
    }
}
