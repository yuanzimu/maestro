//! JSON-RPC API schema（P0 面：~12 方法）。
//! 扁平枚举 + schemars 自动生成 schema，对应 herdr api/schema.rs 的模式。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::Task;
use crate::types::*;

/// JSON-RPC 请求
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    pub id: String,
    pub method: Method,
    pub params: serde_json::Value,
}

/// JSON-RPC 响应
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Ok {
        id: String,
        result: serde_json::Value,
    },
    Err {
        id: String,
        error: RpcError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

/// API 方法（P0 全集 + P1 预留）。新方法只追加不删除（协议双轨制）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    // ---- server ----
    ServerStatus,
    ServerShutdown,
    /// 全局急停：FREEZE→SNAPSHOT→DECIDE 的入口（设计 §3.2）
    ServerEmergencyStop,
    /// 急停恢复（steering: flush=投递冻结期积压轻推 | hold=丢弃）
    ServerResumeAll,

    // ---- task ----
    TaskCreate,
    TaskList,
    TaskGet,
    TaskPause,
    TaskResume,
    TaskCancel,
    /// 轻推：向运行中任务注入补充指示，下一轮生效
    TaskSteer,
    /// 轻推拉取：多轮驱动 Worker 在轮边界拉走积压轻推（并标记已投递）
    TaskSteerPoll,
    /// 轻推确认：worker 用过消息后上报（at-least-once —— 未确认的重投）
    TaskSteerAck,
    /// 轮账上报：多轮驱动 Worker 每轮上报 usage → LedgerEntry 事件入账
    TaskRoundReport,
    /// 账本：任务的轮数/耗时/成本汇总（token 经济口径）
    TaskLedger,

    // ---- worker ----
    WorkerList,
    WorkerGet,

    // ---- inbox ----
    InboxList,

    // ---- events ----
    /// 订阅事件流（from_seq 断点续订）
    EventsSubscribe,

    // ---- checkpoint ----
    CheckpointList,
    CheckpointCreate,
    CheckpointRollback,
}

// ---------------------------------------------------------------------------
// 方法参数/结果类型（强类型化高频方法；其余经 serde_json::Value）
// ---------------------------------------------------------------------------

/// `server.emergency_stop` 参数
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmergencyStopParams {
    /// 审计用原因（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `server.emergency_stop` 结果
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmergencyStopResult {
    pub frozen_workers: Vec<WorkerId>,
    pub suspended_tasks: Vec<TaskId>,
    pub checkpoints: Vec<CheckpointRef>,
    /// 现场是否完整保全（SNAPSHOT 完成）
    pub sessions_preserved: bool,
    /// FREEZE 阶段耗时（毫秒，不变量 I1：< 100）
    pub freeze_ms: u64,
}

/// `server.resume_all` 参数
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResumeAllParams {
    /// flush=投递冻结期间积压的轻推 | hold=丢弃（每条发 SteeringDropped 事件）
    #[serde(default = "default_steering_mode")]
    pub steering: SteeringMode,
}

fn default_steering_mode() -> SteeringMode {
    SteeringMode::Flush
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SteeringMode {
    /// 投递冻结期间积压的轻推
    Flush,
    /// 丢弃（不静默，每条发 SteeringDropped 事件）
    Hold,
}

/// `task.steer` 参数
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskSteerParams {
    pub task: TaskId,
    pub message: String,
}

/// `task.steer_poll` 结果项 / `task.steer_ack` 参数共用 seq 口径
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskSteerAckParams {
    pub task: TaskId,
    pub worker: WorkerId,
    /// 已消费的消息 seq（poll 返回值里带的）
    pub seqs: Vec<u64>,
}

/// `task.round_report` 参数（rounder 每轮上报）
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskRoundReportParams {
    pub task: TaskId,
    pub worker: WorkerId,
    pub round: u32,
    /// 本轮底层 CLI 的 usage（stream-json result 行）
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// 写 cache 的 token（单独计价项）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    /// 本轮模型（stream-json result.model；缺省用 daemon 默认 —— 计价用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 本轮工具调用名（去重；U3 叙事 → RoundProgress 事件）
    #[serde(default)]
    pub tools_used: Vec<String>,
    /// 本轮回答摘要（rounder 侧截断；daemon 再钳 120 字符）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// CLI 自报本轮费用（result.total_cost_usd；daemon 与牌价计费对账用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
}

/// `task.create` 参数
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskCreateParams {
    pub title: String,
    pub prompt: String,
    /// 工作目录（默认当前目录）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

/// `task.create` 结果
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskCreateResult {
    pub task: Task,
    /// None = 已入队（并发满/急停中），未分配 worker
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerId>,
}

/// `events.subscribe` 参数
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EventsSubscribeParams {
    /// 断点续订游标（0 = 从现在开始）
    #[serde(default)]
    pub from_seq: u64,
}

/// `server.status` 结果
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ServerStatusResult {
    pub version: String,
    pub pid: u32,
    pub uptime_secs: u64,
    pub tasks_total: u64,
    pub workers_active: u64,
    pub event_seq: u64,
}

/// `checkpoint.rollback` 参数
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointRollbackParams {
    pub task: TaskId,
    pub to: CheckpointRef,
}

/// `checkpoint.rollback` 结果：pre-rollback 安全垫（不变量 I4）
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointRollbackResult {
    pub rolled_back_to: CheckpointRef,
    pub pre_rollback: CheckpointRef,
}

/// `checkpoint.list` 参数
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointListParams {
    pub task: TaskId,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 方法名 wire 稳定性
    #[test]
    fn method_names() {
        let m = serde_json::to_string(&Method::ServerEmergencyStop).unwrap();
        assert_eq!(m, "\"server_emergency_stop\"");
        let m = serde_json::to_string(&Method::TaskSteer).unwrap();
        assert_eq!(m, "\"task_steer\"");
    }

    /// Request 信封
    #[test]
    fn request_envelope() {
        let req = Request {
            id: "req-1".into(),
            method: Method::ServerEmergencyStop,
            params: serde_json::to_value(EmergencyStopParams {
                reason: Some("user_panic".into()),
            })
            .unwrap(),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["method"], "server_emergency_stop");
        assert_eq!(json["params"]["reason"], "user_panic");
    }

    /// steering 默认 flush
    #[test]
    fn resume_all_default_flush() {
        let params: ResumeAllParams = serde_json::from_str("{}").expect("空对象应可解析");
        assert_eq!(params.steering, SteeringMode::Flush);
    }

    /// EmergencyStopResult 形状（含 freeze_ms 计量）
    #[test]
    fn emergency_stop_result_shape() {
        let r = EmergencyStopResult {
            frozen_workers: vec![WorkerId::new("w1")],
            suspended_tasks: vec![TaskId::new("t1")],
            checkpoints: vec![CheckpointRef::new("cp:t1:r7")],
            sessions_preserved: true,
            freeze_ms: 47,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["freeze_ms"], 47);
        assert_eq!(json["sessions_preserved"], true);
    }
}
