//! Core：单线程权威核心。
//!
//! 并发模型（herdr 验证过）：Core 独占全部状态，其他线程（连接/等待器/定时器）
//! 经 mpsc 发消息；Core 逐条处理，无锁。
//!
//! 单一事实源：**发布即应用** —— `ctx.publish(event)` 同时做 hub 广播 + Authority
//! 状态转移 + 持久化（hub 的 sink），三处永不失同步。

use crate::emergency::{self, EmergencyPhase};
use crate::eventhub::EventHub;
use crate::state::Authority;
use crate::steering::SteeringQueue;
use crate::suspend::ReapReport;
use crate::worker::{self, SpawnSpec, WorkerMeta};
use maestro_protocol::api::{
    CheckpointRollbackParams, CheckpointRollbackResult, EmergencyStopParams, Method, Request,
    Response, ResumeAllParams, RpcError, ServerStatusResult, SteeringMode, TaskCreateParams,
    TaskCreateResult, TaskFeedbackParams, TaskRoundReportParams, TaskSteerAckParams,
    TaskSteerParams,
};
use maestro_protocol::events::{Event, Task, UsageEntry};
use maestro_protocol::types::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// Core 收到的消息（全部来源汇聚于此）
pub enum CoreMsg {
    /// API 请求（连接线程）
    Api(Request, Sender<Response>),
    /// Worker 退出（waiter 线程）
    WorkerExit(worker::WorkerExit),
    /// 自动恢复计时到点（定时器线程）
    TimerFired(TaskId),
    /// 网络连通性探测结果（探测线程）
    NetworkProbe(bool),
    /// 优雅关停
    Shutdown,
}

/// Core 的运行时配置
pub struct CoreConfig {
    pub data_dir: PathBuf,
    /// 任务默认工作目录
    pub workdir: PathBuf,
    /// Worker 程序（P0 v0：外部命令或测试脚本；未来经适配器选择）
    pub worker_program: String,
    pub worker_args: Vec<String>,
    pub socket_path: String,
    /// 最大并行 worker 数（槽位上限；超出入队）。
    /// 槽位语义：活 worker（含 SIGSTOP 挂起中的）各占 1。
    pub max_parallel_workers: usize,
    /// 计价默认模型（轮账上报未带 model 时用；MAESTRO_MODEL 可覆盖。
    /// 0.6 适配器 / 0.8 路由接管前的占位）
    pub default_model: String,
    /// 透传给 Worker 的额外环境（白名单式；如 MAESTRO_CONTEXT_LIMIT
    /// —— 上下文轮转阈值，R37）
    pub worker_env: Vec<(String, String)>,
    /// 上游模型网关（CCR）配置：worker 与 daemon 自身 LLM 调用统一指向网关
    pub gateway: crate::gateway::GatewayConfig,
    /// 硬预算闸门：每轮轮账后强制执行，超限即挂起
    pub budget: crate::budget::TaskBudget,
}

/// 默认槽位数：本地守护进程的保守起点（调研 R12 校准项）
pub const DEFAULT_MAX_PARALLEL_WORKERS: usize = 4;

/// 排队深度上限（R12 调研：溢出全排队不拒绝，但要有防风暴闸）
pub const MAX_QUEUE_DEPTH: usize = 100;

/// 默认计价模型（与真实 claude CLI 默认档对齐）
pub const DEFAULT_MODEL: &str = "claude-sonnet-4";

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            data_dir: std::env::temp_dir().join("maestro-data"),
            workdir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
            worker_program: "/bin/sh".into(),
            worker_args: vec!["-c".into(), "echo '(maestro placeholder worker)'".into()],
            socket_path: "/tmp/maestro.sock".into(),
            max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
            default_model: std::env::var("MAESTRO_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
            worker_env: vec![],
            gateway: crate::gateway::GatewayConfig::default(),
            budget: crate::budget::TaskBudget::default(),
        }
    }
}

/// 发布即应用的上下文（单一事实源）
pub struct Ctx {
    pub hub: Arc<EventHub>,
    pub authority: Authority,
    pub clock: Arc<dyn maestro_protocol::Clock>,
}

impl Ctx {
    pub fn new(
        hub: Arc<EventHub>,
        authority: Authority,
        clock: Arc<dyn maestro_protocol::Clock>,
    ) -> Self {
        Self {
            hub,
            authority,
            clock,
        }
    }

    /// 单一事实源入口：广播 + 状态转移（+ hub sink 持久化）
    pub fn publish(&mut self, event: Event) -> maestro_protocol::events::Envelope {
        let env = self.hub.publish(event);
        self.authority.apply(&env.event, env.ts);
        env
    }

    fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }
}

/// Core 本体
pub struct Core {
    pub ctx: Ctx,
    pub cfg: CoreConfig,
    /// 运行中 Worker 元数据
    pub metas: HashMap<WorkerId, WorkerMeta>,
    pub steering: SteeringQueue,
    /// 急停状态
    pub emergency: EmergencyPhase,
    /// 事件库句柄（server 层订阅重放用）
    store: Option<Arc<std::sync::Mutex<crate::persist::EventStore>>>,
    /// 外部消息入口（clone 给各线程）
    tx: Sender<CoreMsg>,
    rx: Receiver<CoreMsg>,
    /// 网络连通性（NetworkProbe 驱动）
    network_ok: bool,
    started_at: u64,
    next_worker: u64,
    /// 验收门 v0：worker 启动前的 worktree 读回（按任务记）
    pre_spawn: HashMap<TaskId, crate::acceptance::Readback>,
}

impl Core {
    /// 全新启动
    pub fn new(cfg: CoreConfig, clock: Arc<dyn maestro_protocol::Clock>) -> Self {
        let (tx, rx) = channel();
        let store = crate::persist::EventStore::open(&cfg.data_dir)
            .ok()
            .map(|s| Arc::new(std::sync::Mutex::new(s)));
        let hub = Arc::new(EventHub::new());
        if let Some(s) = &store {
            let s = s.clone();
            hub.set_sink(Box::new(move |env| {
                let _ = s.lock().unwrap().append(env);
            }));
        }
        let started_at = clock.now_ms();
        let steering = SteeringQueue::open(&cfg.data_dir);
        Self {
            ctx: Ctx::new(hub, Authority::new(), clock),
            cfg,
            metas: HashMap::new(),
            steering,
            emergency: EmergencyPhase::None,
            store,
            tx,
            rx,
            network_ok: true,
            started_at,
            next_worker: 1,
            pre_spawn: HashMap::new(),
        }
    }

    /// 从事件流恢复启动（kill -9 后）
    pub fn recover(cfg: CoreConfig, clock: Arc<dyn maestro_protocol::Clock>) -> (Self, ReapReport) {
        // 1. 重放事件流重建权威状态
        let store = crate::persist::EventStore::open(&cfg.data_dir)
            .ok()
            .map(|s| Arc::new(std::sync::Mutex::new(s)));
        let events = store
            .as_ref()
            .map(|s| s.lock().unwrap().replay_all())
            .unwrap_or_default();
        let authority = Authority::replay(&events);
        let max_seq = events.last().map(|e| e.seq).unwrap_or(0);

        // 2. 孤儿清理（绝不收养）
        let report = crate::suspend::reap_orphans(&cfg.data_dir.join("workers"));

        // 3. 重建 Core
        let (tx, rx) = channel();
        let hub = Arc::new(EventHub::new());
        hub.set_seq_floor(max_seq + 1);
        // 历史事件视作已广播：新订阅的重放窗口 [from_seq, max_seq] 才能覆盖
        // 重启前的事件库（否则 GUI 首连 from_seq=1 重放为空）
        hub.set_broadcast_floor(max_seq);
        if let Some(s) = &store {
            let s = s.clone();
            hub.set_sink(Box::new(move |env| {
                let _ = s.lock().unwrap().append(env);
            }));
        }
        let started_at = clock.now_ms();
        let steering = SteeringQueue::open(&cfg.data_dir);
        // 急停相位由事件流派生（EmergencyStopped/EmergencyResumed 都已入库）：
        // 此前硬编码 None —— 急停后 daemon 重启即「忘记」急停，冻结期入队的
        // 新任务被 try_start_queued 错误启动（B12 Frozen 拦截失效）
        let emergency = if authority.emergency_frozen {
            EmergencyPhase::Frozen
        } else {
            EmergencyPhase::None
        };
        let mut core = Self {
            ctx: Ctx::new(hub, authority, clock),
            cfg,
            metas: HashMap::new(),
            steering,
            emergency,
            store,
            tx,
            rx,
            network_ok: true,
            started_at,
            next_worker: 1,
            pre_spawn: HashMap::new(),
        };
        // 恢复时把「正在执行」的任务标记为 suspended(DaemonCrash)（仅手动恢复）
        core.mark_recovered_as_daemon_crash();
        // 崩溃前已在自动恢复退避中的任务（NetworkLost 等）：重启后继续调度，
        // 否则将永久滞留 Suspended（R11 审计发现的缺口）
        for (task, attempt, _reason) in core.ctx.authority.auto_resume_candidates() {
            core.schedule_resume(&task, attempt);
        }
        // 终态任务状态目录滚动 GC（R50）：只删 Done/Failed/Cancelled 且超
        // 保留额度的旧目录；非终态（resume 凭据）与未知目录不碰
        let gc = crate::taskstate::gc_from_records(
            core.ctx
                .authority
                .tasks
                .values()
                .map(|t| (&t.task.id, t.state, t.task.workdir.as_str())),
        );
        if !gc.removed.is_empty() {
            eprintln!(
                "maestro: 状态目录 GC: removed={}（每工作目录保留最近 {} 个终态目录）",
                gc.removed.len(),
                crate::taskstate::KEEP_PER_WORKDIR
            );
        }
        (core, report)
    }

    /// 事件库句柄（server 层订阅重放用）
    pub fn event_store_handle(&self) -> Option<Arc<std::sync::Mutex<crate::persist::EventStore>>> {
        self.store.clone()
    }

    /// EventHub 共享句柄（server 事件线程订阅用 —— 不需要 Core 锁）
    pub fn hub_handle(&self) -> Arc<EventHub> {
        self.ctx.hub.clone()
    }

    fn mark_recovered_as_daemon_crash(&mut self) {
        let crashed: Vec<TaskId> = self
            .ctx
            .authority
            .tasks
            .values()
            .filter(|t| {
                t.state == WorkerState::Working
                    || t.state == WorkerState::Queued && t.worker.is_some()
            })
            .map(|t| t.task.id.clone())
            .collect();
        for task in crashed {
            let (worker, session_ref, cp, round) = {
                let t = self.ctx.authority.get(&task).unwrap();
                (
                    t.worker.clone().unwrap_or_else(|| WorkerId::new("unknown")),
                    t.session_ref.clone().unwrap_or_else(|| SessionRef::new("")),
                    t.checkpoint_ref
                        .clone()
                        .unwrap_or_else(|| CheckpointRef::new("")),
                    t.round,
                )
            };
            self.ctx.publish(Event::Suspended {
                task,
                worker,
                reason: SuspendReason::DaemonCrash,
                session_ref,
                checkpoint_ref: cp,
                round,
            });
        }
    }

    /// 外部消息入口（连接线程/定时器持有 clone）
    pub fn sender(&self) -> Sender<CoreMsg> {
        self.tx.clone()
    }

    /// 主循环（单线程权威）
    pub fn run(&mut self) {
        while let Ok(msg) = self.rx.recv() {
            match msg {
                CoreMsg::Api(req, reply) => {
                    let resp = self.handle_api(req);
                    let _ = reply.send(resp);
                }
                CoreMsg::WorkerExit(exit) => self.on_worker_exit(exit),
                CoreMsg::TimerFired(task) => self.on_timer(task),
                CoreMsg::NetworkProbe(ok) => self.network_ok = ok,
                CoreMsg::Shutdown => break,
            }
        }
    }

    // -----------------------------------------------------------------------
    // API 分发
    // -----------------------------------------------------------------------

    fn handle_api(&mut self, req: Request) -> Response {
        match req.method {
            Method::ServerStatus => self.api_status(&req),
            Method::ServerEmergencyStop => {
                let _params: EmergencyStopParams = serde_json::from_value(req.params.clone())
                    .unwrap_or(EmergencyStopParams { reason: None });
                self.api_emergency_stop(&req)
            }
            Method::ServerResumeAll => {
                let params: ResumeAllParams =
                    serde_json::from_value(req.params.clone()).unwrap_or(ResumeAllParams {
                        steering: SteeringMode::Flush,
                    });
                self.api_resume_all(&req, params.steering)
            }
            Method::ServerShutdown => {
                let _ = self.tx.send(CoreMsg::Shutdown);
                self.ok(&req, serde_json::json!({"shutting_down": true}))
            }
            Method::TaskCreate => self.api_task_create(&req),
            Method::TaskList => {
                // B1 叙事（R54）：列表项也带一句话进度 —— UI 任务卡的主数据源。
                // 每任务一次事件重放开销在 P0 规模（任务数×事件量小）可接受；
                // 大规模时改增量缓存
                let tasks: Vec<serde_json::Value> = self
                    .ctx
                    .authority
                    .tasks
                    .values()
                    .map(|t| {
                        let vitals = self.task_vitals(&t.task.id);
                        serde_json::json!({
                            "id": t.task.id,
                            "title": t.task.title,
                            "state": t.state,
                            "round": t.round,
                            "narrative": crate::narrative::progress_line_with_state(&vitals, &t.state),
                            "blocked_kind": t.blocked_kind,
                            "suspend_reason": t.suspend.as_ref().map(|s| s.reason),
                            "acceptance_failures": t.acceptance_failures,
                        })
                    })
                    .collect();
                self.ok(&req, serde_json::json!({ "tasks": tasks }))
            }
            Method::TaskGet => {
                let task_id = req
                    .params
                    .get("task")
                    .and_then(|v| v.as_str())
                    .map(TaskId::new);
                match task_id.and_then(|id| self.ctx.authority.get(&id).cloned()) {
                    Some(t) => {
                        // B1 叙事降级模板（R46）：事件流单遍聚合 → 一句话进度
                        let vitals = self.task_vitals(&t.task.id);
                        self.ok(
                            &req,
                            serde_json::json!({
                                "id": t.task.id, "title": t.task.title, "state": t.state,
                                "round": t.round, "worker": t.worker,
                                "session_ref": t.session_ref, "checkpoint_ref": t.checkpoint_ref,
                                // U5 结果卡：变更明细（task_diff）需要 workdir 定位 git 仓库。
                                // 增量字段 —— 旧客户端宽松解析，安全忽略
                                "workdir": t.task.workdir,
                                "suspend_reason": t.suspend.as_ref().map(|s| s.reason),
                                "blocked_kind": t.blocked_kind,
                                "acceptance_failures": t.acceptance_failures,
                                "narrative": crate::narrative::progress_line_with_state(&vitals, &t.state),
                            }),
                        )
                    }
                    None => self.err(&req, -404, "task not found"),
                }
            }
            Method::TaskSteer => {
                let params: TaskSteerParams = match serde_json::from_value(req.params.clone()) {
                    Ok(p) => p,
                    Err(_) => return self.err(&req, -400, "bad params"),
                };
                self.api_steer(&req, &params)
            }
            Method::TaskSteerAck => {
                let params: TaskSteerAckParams = match serde_json::from_value(req.params.clone()) {
                    Ok(p) => p,
                    Err(_) => return self.err(&req, -400, "bad params"),
                };
                self.api_steer_ack(&req, &params)
            }
            Method::TaskRoundReport => {
                let params: TaskRoundReportParams = match serde_json::from_value(req.params.clone())
                {
                    Ok(p) => p,
                    Err(_) => return self.err(&req, -400, "bad params"),
                };
                self.api_round_report(&req, &params)
            }
            Method::TaskLedger => {
                let task_id = req
                    .params
                    .get("task")
                    .and_then(|v| v.as_str())
                    .map(TaskId::new);
                match task_id {
                    Some(id) => self.api_ledger(&req, &id),
                    None => self.err(&req, -400, "missing task"),
                }
            }
            Method::TaskFeedback => {
                // 👎 必填理由：负面反馈无理由 = 无法沉淀教训
                let params: TaskFeedbackParams = match serde_json::from_value(req.params.clone())
                {
                    Ok(p) => p,
                    Err(e) => return self.err(&req, -400, &format!("bad params: {e}")),
                };
                if !params.positive && params.reason.as_deref().map(str::trim).unwrap_or("").is_empty() {
                    return self.err(&req, -400, "negative feedback requires a reason");
                }
                self.api_task_feedback(&req, params)
            }
            Method::TaskSteerPoll => {
                let task_id = req
                    .params
                    .get("task")
                    .and_then(|v| v.as_str())
                    .map(TaskId::new);
                let worker_id = req
                    .params
                    .get("worker")
                    .and_then(|v| v.as_str())
                    .map(WorkerId::new);
                match (task_id, worker_id) {
                    (Some(t), Some(w)) => self.api_steer_poll(&req, &t, &w),
                    _ => self.err(&req, -400, "missing task/worker"),
                }
            }
            Method::TaskPause => {
                let task_id = req
                    .params
                    .get("task")
                    .and_then(|v| v.as_str())
                    .map(TaskId::new);
                match task_id {
                    Some(id) => self.api_pause(&req, &id),
                    None => self.err(&req, -400, "missing task"),
                }
            }
            Method::TaskResume => {
                let task_id = req
                    .params
                    .get("task")
                    .and_then(|v| v.as_str())
                    .map(TaskId::new);
                match task_id {
                    Some(id) => self.api_resume(&req, &id),
                    None => self.err(&req, -400, "missing task"),
                }
            }
            Method::TaskCancel => {
                let task_id = req
                    .params
                    .get("task")
                    .and_then(|v| v.as_str())
                    .map(TaskId::new);
                match task_id {
                    Some(id) => {
                        let ok = emergency::cancel_task(&mut self.ctx, &self.metas, &id);
                        if ok {
                            self.drop_pending_steering(&id);
                            self.ok(&req, serde_json::json!({"cancelled": true}))
                        } else {
                            self.err(&req, -404, "task not found or not cancellable")
                        }
                    }
                    None => self.err(&req, -400, "missing task"),
                }
            }
            Method::WorkerList => {
                let workers: Vec<serde_json::Value> = self
                    .ctx
                    .authority
                    .workers
                    .values()
                    .map(|w| {
                        serde_json::json!({ "id": w.id, "task": w.task, "state": w.state, "pid": w.pid })
                    })
                    .collect();
                self.ok(&req, serde_json::json!({ "workers": workers }))
            }
            Method::WorkerGet => self.err(&req, -501, "not implemented yet"),
            Method::InboxList => {
                // blocked 任务的清单（收件箱 v0）
                let items: Vec<serde_json::Value> = self
                    .ctx
                    .authority
                    .tasks
                    .values()
                    .filter(|t| t.state == WorkerState::Blocked)
                    .map(|t| {
                        serde_json::json!({
                            "task": t.task.id,
                            "kind": t.blocked_kind,
                            "title": t.task.title,
                        })
                    })
                    .collect();
                self.ok(&req, serde_json::json!({ "items": items }))
            }
            Method::EventsSubscribe => {
                // 订阅由 server 层处理（需要回传 receiver）；这里回 ack
                self.ok(&req, serde_json::json!({"subscribe": "via-event-socket"}))
            }
            Method::CheckpointList => {
                let params: Result<maestro_protocol::api::CheckpointListParams, _> =
                    serde_json::from_value(req.params.clone());
                match params {
                    Ok(p) => {
                        let task = self.ctx.authority.get(&p.task).cloned();
                        match task {
                            Some(t) => {
                                let cps = crate::checkpoints::list(
                                    std::path::Path::new(&t.task.workdir),
                                    &p.task,
                                );
                                let out: Vec<serde_json::Value> = cps
                                    .iter()
                                    .map(|c| {
                                        serde_json::json!({
                                            "seq": c.seq, "reason": c.reason,
                                            "ref": c.full_ref, "commit": c.commit,
                                        })
                                    })
                                    .collect();
                                self.ok(&req, serde_json::json!({ "checkpoints": out }))
                            }
                            None => self.err(&req, -404, "task not found"),
                        }
                    }
                    Err(_) => self.err(&req, -400, "bad params"),
                }
            }
            Method::CheckpointCreate => self.err(&req, -501, "not implemented yet"),
            Method::CheckpointRollback => {
                let params: CheckpointRollbackParams =
                    match serde_json::from_value(req.params.clone()) {
                        Ok(p) => p,
                        Err(_) => return self.err(&req, -400, "bad params"),
                    };
                self.api_rollback(&req, params)
            }
        }
    }

    fn api_status(&mut self, req: &Request) -> Response {
        let status = ServerStatusResult {
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            uptime_secs: (self.ctx.now_ms().saturating_sub(self.started_at)) / 1000,
            tasks_total: self.ctx.authority.tasks.len() as u64,
            workers_active: self
                .ctx
                .authority
                .workers
                .values()
                .filter(|w| w.state == WorkerState::Working)
                .count() as u64,
            // last_seq（含语义）：= 已发布事件数，客户端可安全作续订游标基准
            event_seq: self.ctx.hub.last_seq(),
        };
        Response::Ok {
            id: req.id.clone(),
            result: serde_json::to_value(status).unwrap_or_default(),
        }
    }

    fn api_emergency_stop(&mut self, req: &Request) -> Response {
        let result = emergency::emergency_stop(&mut self.ctx, &self.metas, "user_panic");
        self.emergency = EmergencyPhase::Frozen;
        Response::Ok {
            id: req.id.clone(),
            result: serde_json::to_value(&result).unwrap_or_default(),
        }
    }

    fn api_resume_all(&mut self, req: &Request, mode: SteeringMode) -> Response {
        let (resumed, dead_suspended) =
            emergency::resume_all(&mut self.ctx, &self.metas, &mut self.steering, mode);
        self.emergency = EmergencyPhase::None;
        // 竞态修复：急停挂起但 worker 已死的任务 —— 重拉进程而非假恢复
        for task in &dead_suspended {
            if !self.slots_free() {
                break;
            }
            // 急停期间积压的轻推随 respawn 注入（投递语义 v0.15）
            let prefix = self.steering_prefix(task);
            if let Ok(w) = self.spawn_worker_for(task, prefix) {
                self.ctx.publish(Event::Resumed {
                    task: task.clone(),
                    worker: w,
                    from_reason: SuspendReason::EmergencyStop,
                    via: ResumeVia::ResumeAll,
                    new_session_ref: None,
                });
            }
        }
        // 全局解除标记：急停时若无任务被冻结，上面一条 Resumed 都不会有 ——
        // 只有本事件能让 recover() 派生出「急停已解除」（否则空冻结场景
        // 重启后永远卡 Frozen，新任务全被 B12 拦截）
        let mut all_resumed = resumed.clone();
        all_resumed.extend(dead_suspended.iter().cloned());
        self.ctx.publish(Event::EmergencyResumed {
            resumed: all_resumed,
        });
        // B12：调度器解冻 —— 补位启动冻结期入队/被搁置的任务（受槽位约束）
        self.try_start_queued();
        Response::Ok {
            id: req.id.clone(),
            result: serde_json::json!({ "resumed": resumed }),
        }
    }

    /// 结果反馈（U7 v1）：FeedbackRecorded 事件（入库+广播，source of truth）
    /// 与追加写 workdir/MAESTRO_MEMORY.md 项目记忆（C4 prompt 注入锚点）。
    /// 先写文件后 publish：文件失败即返回错误（反馈未落账，可重试），
    /// 避免事件入账但记忆缺失的半态。
    fn api_task_feedback(&mut self, req: &Request, params: TaskFeedbackParams) -> Response {
        let Some(t) = self.ctx.authority.get(&params.task).cloned() else {
            return self.err(req, -404, "task not found");
        };
        let reason = params
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        // 项目记忆（人可读 append-only 日志）。标题/理由压平单行 ——
        // 标题含换行会伪造条目结构（markdown 注入）
        let flat = |s: &str| s.replace(['\n', '\r'], " ");
        let memory = std::path::Path::new(&t.task.workdir).join("MAESTRO_MEMORY.md");
        let ts = self.ctx.now_ms();
        let entry = format!(
            "\n## {} {} · {} · ts {}\n{}\n",
            if params.positive { "[👍]" } else { "[👎]" },
            t.task.id.as_str(),
            flat(&t.task.title),
            ts,
            reason
                .as_deref()
                .map(|r| format!("理由：{}", flat(r)))
                .unwrap_or_else(|| {
                    if params.positive { "（无备注）" } else { "（未提供）" }.to_string()
                }),
        );
        let write = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&memory)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(entry.as_bytes())
            });
        if let Err(e) = write {
            return self.err(req, -500, &format!("项目记忆写入失败（{e}）"));
        }

        // 事件（发布即应用 + 持久化 + 广播）
        self.ctx.publish(Event::FeedbackRecorded {
            task: params.task.clone(),
            positive: params.positive,
            reason,
        });
        self.ok(req, serde_json::json!({ "recorded": true }))
    }

    fn api_task_create(&mut self, req: &Request) -> Response {
        let params: TaskCreateParams = match serde_json::from_value(req.params.clone()) {
            Ok(p) => p,
            Err(_) => return self.err(req, -400, "bad params"),
        };
        // 安全基线（0.11）：输入长度 + workdir 敏感路径
        if let Err(e) = crate::security::validate_task_input(&params.title, &params.prompt) {
            return self.err(req, -400, &e);
        }
        let workdir = params
            .workdir
            .clone()
            .unwrap_or_else(|| self.cfg.workdir.display().to_string());
        if let Err(e) = crate::security::validate_workdir(&workdir) {
            return self.err(req, -403, &e);
        }
        let task_id = TaskId::new(format!("t-{}", self.next_seq_hint()));
        let task = Task {
            id: task_id.clone(),
            title: params.title.clone(),
            workdir: workdir.clone(),
            created_at: self.ctx.now_ms(),
        };
        self.ctx.publish(Event::TaskCreated {
            task: task.clone(),
            prompt: params.prompt.clone(),
        });
        // baseline checkpoint：入队即锚点（spawn 前 —— 防快 worker 抢先落盘）
        let _ = crate::checkpoints::capture(
            std::path::Path::new(&workdir),
            &task_id,
            0,
            CpReason::Baseline,
            self.ctx.clock.as_ref(),
        );

        // 防风暴闸：排队深度超限直接拒绝（R12 调研建议）
        let queued_depth = self
            .ctx
            .authority
            .tasks
            .values()
            .filter(|t| t.state == WorkerState::Queued && t.worker.is_none())
            .count();
        if queued_depth >= MAX_QUEUE_DEPTH {
            return self.err(req, -429, "queue depth limit reached");
        }

        // 急停期间新任务只入队不启动（用例 B12）；并发满 / 同 workdir 被
        // 活 worker 占用，同样入队（R13 调度器 + workdir 互斥）
        if self.emergency == EmergencyPhase::Frozen
            || self.metas.len() >= self.cfg.max_parallel_workers
            || self.workdir_occupied(&workdir)
        {
            let reason = if self.emergency == EmergencyPhase::Frozen {
                "emergency_frozen"
            } else if self.metas.len() >= self.cfg.max_parallel_workers {
                "slots_full"
            } else {
                "workdir_busy"
            };
            return self.ok(
                req,
                serde_json::json!({ "task": task, "queued": true, "reason": reason }),
            );
        }

        match self.spawn_worker_for(&task_id, None) {
            Ok(worker_id) => self.ok(
                req,
                serde_json::to_value(TaskCreateResult {
                    task,
                    worker: Some(worker_id),
                })
                .unwrap_or_default(),
            ),
            Err(e) => self.err(req, -500, &format!("spawn failed: {e}")),
        }
    }

    /// 轻推拉取（多轮驱动 Worker 轮边界调用）：取走积压 + 重投未确认
    /// （at-least-once，R32）。仅当前 worker 可拉（防过期驱动进程抽走消息）。
    fn api_steer_poll(&mut self, req: &Request, task: &TaskId, worker: &WorkerId) -> Response {
        let Some(t) = self.ctx.authority.get(task).cloned() else {
            return self.err(req, -404, "task not found");
        };
        if t.state != WorkerState::Working {
            return self.err(req, -409, "task not working");
        }
        if t.worker.as_ref() != Some(worker) {
            return self.err(req, -403, "not the current worker");
        }
        let msgs = self.steering.poll(task);
        let out: Vec<serde_json::Value> = msgs
            .iter()
            .map(|m| {
                // 重投也发 Delivered（审计可见投递次数）
                self.ctx.publish(Event::SteeringDelivered {
                    task: task.clone(),
                    round: t.round,
                    message: m.message.clone(),
                });
                serde_json::json!({ "seq": m.seq, "message": m.message })
            })
            .collect();
        self.ok(req, serde_json::json!({ "messages": out }))
    }

    /// 轻推确认（at-least-once）：worker 用过后上报，未确认的会重投。
    /// 仅当前 worker 可确认。
    fn api_steer_ack(&mut self, req: &Request, params: &TaskSteerAckParams) -> Response {
        let Some(t) = self.ctx.authority.get(&params.task).cloned() else {
            return self.err(req, -404, "task not found");
        };
        if t.worker.as_ref() != Some(&params.worker) {
            return self.err(req, -403, "not the current worker");
        }
        let n = self.steering.ack(&params.task, &params.seqs);
        self.ok(
            req,
            serde_json::json!({ "acked": n, "remaining": self.steering.pending(&params.task) }),
        )
    }

    /// 轮账上报（rounder 每轮）：usage → 牌价计价 → LedgerEntry 事件
    /// （token 计量 + 成本闭环，R24）。仅当前 worker 可报（防过期驱动进程灌账）。
    /// 模型不在牌价表 → 条目仍入账但 cents 留空（不阻塞计量）。
    fn api_round_report(&mut self, req: &Request, params: &TaskRoundReportParams) -> Response {
        let Some(t) = self.ctx.authority.get(&params.task).cloned() else {
            return self.err(req, -404, "task not found");
        };
        if t.worker.as_ref() != Some(&params.worker) {
            return self.err(req, -403, "not the current worker");
        }
        let model = params
            .model
            .clone()
            .unwrap_or_else(|| self.cfg.default_model.clone());
        // 上下文压缩（R37 轮转）：压缩轮上报 → U3 叙事「上下文已压缩，任务继续」
        if params.compacted {
            self.ctx.publish(Event::ContextCompacted {
                task: params.task.clone(),
                round: params.round,
            });
        }
        // 轮进度事件（U3 叙事）：CLI/Desktop 实时渲染「它正在干什么」
        self.ctx.publish(Event::RoundProgress {
            task: params.task.clone(),
            round: params.round,
            tools_used: params.tools_used.clone(),
            summary: params
                .summary
                .clone()
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect(),
            tokens_in: params.input_tokens,
            tokens_out: params.output_tokens,
        });
        let usage = crate::llm::priced_usage_entry(
            &model,
            params.input_tokens,
            params.output_tokens,
            params.cache_read_tokens,
            params.cache_creation_tokens,
            "round",
        )
        .unwrap_or(UsageEntry {
            input_tokens: params.input_tokens,
            output_tokens: params.output_tokens,
            cache_read_tokens: params.cache_read_tokens,
            cache_creation_tokens: params.cache_creation_tokens,
            path: Some("round".into()),
            discount: None,
            counterfactual_cost_cents: None,
            actual_cost_cents: None,
        });
        // 费用对账（R34，T4 增量对账）：CLI 自报 total_cost_usd vs 牌价计费。
        // 漂移 > 25% → CostDrift 事件（牌价表过期 / usage 口径变化的信号）。
        // 不影响入账金额（daemon 计费为准）；双方都 > 0 才有对账意义
        if let (Some(usd), Some(ledger)) = (params.total_cost_usd, usage.actual_cost_cents) {
            // NaN/负数 → 0 → 跳过；超大值 as u64 饱和（对账仍成立，漂移 100%）
            let cli = (usd * 100.0).max(0.0).round() as u64;
            if cli > 0 && ledger > 0 {
                let hi = cli.max(ledger);
                let lo = cli.min(ledger);
                // u128 中间量：极端值（usd 饱和到 u64::MAX）下 (hi-lo)*100 不溢出
                let drift_pct = ((hi - lo) as u128 * 100 / hi as u128) as u64;
                if drift_pct > 25 {
                    self.ctx.publish(Event::CostDrift {
                        task: params.task.clone(),
                        round: params.round,
                        model: model.clone(),
                        ledger_cents: ledger,
                        cli_cents: cli,
                    });
                }
            }
        }
        self.ctx.publish(Event::LedgerEntry {
            task: params.task.clone(),
            worker: Some(params.worker.clone()),
            usage,
        });
        // C3 断点续跑锚点：轮完成即快照（= 下一轮起点，含 .maestro session）。
        // 失败任务 resume 时回滚到最近锚点：已完成轮保留、失败轮半成品随
        // clean -fd 清除。滚动 GC 最近 3 个（baseline/AcceptancePassed 为
        // pinned 不受影响）；capture 失败不阻塞轮循环（缺锚点时 resume 退
        // baseline 或不回滚直接续跑）。
        {
            let workdir = std::path::Path::new(&t.task.workdir);
            if crate::checkpoints::capture(
                workdir,
                &params.task,
                params.round,
                CpReason::RoundStart,
                self.ctx.clock.as_ref(),
            )
            .is_ok()
            {
                let _ = crate::checkpoints::gc(workdir, &params.task, 3);
            }
        }
        // 硬预算闸门（v2.5）：本轮入账后聚合花费/耗时，超限即冻结 + 挂起。
        // 挂起后直接返回，worker 已被 SIGSTOP 静止（现场完整，仅手动恢复）。
        if let Some(limit) = self.budget_violation(&params.task) {
            self.enforce_budget(&params.task, limit);
            return self.ok(
                req,
                serde_json::json!({ "recorded": true, "round": params.round, "budget_exceeded": true }),
            );
        }
        self.ok(
            req,
            serde_json::json!({ "recorded": true, "round": params.round }),
        )
    }

    /// 计算任务当前的预算命中（花费/耗时），无命中返回 None。
    fn budget_violation(&self, task: &TaskId) -> Option<crate::budget::BudgetLimit> {
        let v = self.task_vitals(task);
        self.cfg.budget.check(v.cost_cents, v.wall_ms)
    }

    /// 执行预算超限：SIGSTOP 冻结当前 worker → Suspended(BudgetExceeded)
    /// （仅手动恢复）+ 叙事快照记录原因。
    fn enforce_budget(&mut self, task: &TaskId, limit: crate::budget::BudgetLimit) {
        let Some(t) = self.ctx.authority.get(task).cloned() else {
            return;
        };
        if let Some(w) = &t.worker {
            if let Some(m) = self.metas.get(w) {
                let _ = worker::freeze_group(m.pgid);
            }
        }
        let worker = t.worker.clone().unwrap_or_else(|| WorkerId::new("none"));
        let why = limit.describe();
        self.ctx.publish(Event::NarrativeSnapshot {
            task: task.clone(),
            round: t.round,
            milestone: format!("预算超限已挂起：{why}"),
        });
        self.ctx.publish(Event::Suspended {
            task: task.clone(),
            worker,
            reason: SuspendReason::BudgetExceeded,
            session_ref: t.session_ref.clone().unwrap_or_else(|| SessionRef::new("")),
            checkpoint_ref: t.checkpoint_ref.clone().unwrap_or_else(|| CheckpointRef::new("")),
            round: t.round,
        });
    }

    /// 账本（0.9）：轮数/耗时/成本汇总 —— 「这个任务花了多少」一句话回答。
    /// 数据源 = 事件流重放（与恢复同一口径）；rounder 每轮经 TaskRoundReport
    /// 入账 LedgerEntry（token 计量闭环，R24）。
    fn api_ledger(&mut self, req: &Request, task: &TaskId) -> Response {
        if self.ctx.authority.get(task).is_none() {
            return self.err(req, -404, "task not found");
        }
        let events = self
            .store
            .as_ref()
            .map(|s| s.lock().unwrap().replay_all())
            .unwrap_or_default();
        let mut first_ts: Option<u64> = None;
        let mut last_ts: Option<u64> = None;
        let mut input_tokens = 0u64;
        let mut output_tokens = 0u64;
        let mut cache_read_tokens = 0u64;
        let mut cache_creation_tokens = 0u64;
        let mut actual_cents = 0u64;
        let mut counterfactual_cents = 0u64;
        let mut entries = 0u64;
        let mut spawns = 0u64;
        let mut compactions = 0u64;
        for env in &events {
            let mine = match &env.event {
                Event::TaskCreated { task: t, .. } => &t.id == task,
                Event::WorkerSpawned { task: t, .. } | Event::LedgerEntry { task: t, .. } => {
                    t == task
                }
                Event::ContextCompacted { task: t, .. } => t == task,
                _ => false,
            };
            if !mine {
                continue;
            }
            first_ts.get_or_insert(env.ts);
            last_ts = Some(env.ts);
            match &env.event {
                Event::WorkerSpawned { .. } => spawns += 1,
                Event::ContextCompacted { .. } => compactions += 1,
                Event::LedgerEntry { usage, .. } => {
                    entries += 1;
                    input_tokens += usage.input_tokens;
                    output_tokens += usage.output_tokens;
                    cache_read_tokens += usage.cache_read_tokens.unwrap_or(0);
                    cache_creation_tokens += usage.cache_creation_tokens.unwrap_or(0);
                    actual_cents += usage.actual_cost_cents.unwrap_or(0);
                    counterfactual_cents += usage.counterfactual_cost_cents.unwrap_or(0);
                }
                _ => {}
            }
        }
        // 轮数口径：单发 worker = spawn 次数；rounder = LedgerEntry 数
        // （spawn 即第 1 轮，后续轮每轮一条）—— 取两者较大值兼容两种模式
        let rounds = spawns.max(entries);
        let wall_ms = match (first_ts, last_ts) {
            (Some(a), Some(b)) => b.saturating_sub(a),
            _ => 0,
        };
        let saved_cents = counterfactual_cents.saturating_sub(actual_cents);
        self.ok(
            req,
            serde_json::json!({
                "task": task,
                "rounds": rounds,
                "wall_ms": wall_ms,
                "ledger_entries": entries,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "cache_read_tokens": cache_read_tokens,
                "cache_creation_tokens": cache_creation_tokens,
                "actual_cost_cents": actual_cents,
                "counterfactual_cost_cents": counterfactual_cents,
                "saved_cents": saved_cents,
                "compactions": compactions,
            }),
        )
    }

    /// B1 叙事素材（R46）：事件流单遍聚合该任务的执行概况。
    /// 与 api_ledger 同一口径（store 重放），只取叙事需要的字段。
    fn task_vitals(&self, task: &TaskId) -> crate::narrative::TaskVitals {
        let events = self
            .store
            .as_ref()
            .map(|s| s.lock().unwrap().replay_all())
            .unwrap_or_default();
        let mut v = crate::narrative::TaskVitals::default();
        let mut tool_counts: HashMap<String, u32> = HashMap::new();
        let mut first_ts: Option<u64> = None;
        let mut last_ts: Option<u64> = None;
        for env in &events {
            let mine = match &env.event {
                Event::TaskCreated { task: t, .. } => &t.id == task,
                Event::RoundProgress { task: t, .. }
                | Event::LedgerEntry { task: t, .. }
                | Event::ContextCompacted { task: t, .. } => t == task,
                _ => false,
            };
            if !mine {
                continue;
            }
            first_ts.get_or_insert(env.ts);
            last_ts = Some(env.ts);
            match &env.event {
                Event::RoundProgress {
                    tools_used,
                    summary,
                    ..
                } => {
                    v.rounds += 1;
                    for name in tools_used {
                        *tool_counts.entry(name.clone()).or_insert(0) += 1;
                    }
                    // 摘要钳 40 字（事件里已钳 120，这里再压一层 —— 一句话进度容不下全文）
                    v.last_summary = summary.chars().take(40).collect();
                }
                Event::LedgerEntry { usage, .. } => {
                    v.tokens_in += usage.input_tokens;
                    v.tokens_out += usage.output_tokens;
                    v.cost_cents += usage.actual_cost_cents.unwrap_or(0);
                }
                Event::ContextCompacted { .. } => v.compactions += 1,
                _ => {}
            }
        }
        // 工具按次数降序、并列按名字稳定序（top3 截取交给 progress_line）
        v.tools = tool_counts.into_iter().collect();
        v.tools.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.wall_ms = match (first_ts, last_ts) {
            (Some(a), Some(b)) => b.saturating_sub(a),
            _ => 0,
        };
        v
    }

    fn api_steer(&mut self, req: &Request, params: &TaskSteerParams) -> Response {
        let Some(t) = self.ctx.authority.get(&params.task).cloned() else {
            return self.err(req, -404, "task not found");
        };
        if let Err(e) = crate::security::validate_steer_message(&params.message) {
            return self.err(req, -400, &e);
        }
        if t.state.is_terminal() {
            return self.err(req, -409, "task already finished");
        }
        let msg = match self.steering.push(&params.task, params.message.clone()) {
            Ok(m) => m,
            Err(e) => return self.err(req, -400, &e),
        };
        self.ctx.publish(Event::SteeringQueued {
            task: params.task.clone(),
            message: msg.message,
        });
        self.ok(req, serde_json::json!({ "queued": true, "seq": msg.seq }))
    }

    fn api_pause(&mut self, req: &Request, task: &TaskId) -> Response {
        let Some(t) = self.ctx.authority.get(task).cloned() else {
            return self.err(req, -404, "task not found");
        };
        if t.state != WorkerState::Working {
            return self.err(req, -409, "task not running");
        }
        // SIGSTOP + Suspended(UserPause)
        if let Some(w) = &t.worker {
            if let Some(m) = self.metas.get(w) {
                let _ = worker::freeze_group(m.pgid);
            }
            self.ctx.publish(Event::Suspended {
                task: task.clone(),
                worker: w.clone(),
                reason: SuspendReason::UserPause,
                session_ref: t.session_ref.clone().unwrap_or_else(|| SessionRef::new("")),
                checkpoint_ref: t
                    .checkpoint_ref
                    .clone()
                    .unwrap_or_else(|| CheckpointRef::new("")),
                round: t.round,
            });
            return self.ok(req, serde_json::json!({ "paused": true }));
        }
        self.err(req, -409, "no worker")
    }

    fn api_resume(&mut self, req: &Request, task: &TaskId) -> Response {
        let Some(t) = self.ctx.authority.get(task).cloned() else {
            return self.err(req, -404, "task not found");
        };
        // 全局急停中：单任务 resume 一律拒绝（F60 场景抓到）—— 原 worker
        // 存活分支缺守卫，会解冻个别进程破坏「Frozen=全冻结」不变量。
        // 解冻统一走 resume_all（含积压轻推的 flush/hold 决策）。
        if self.emergency == EmergencyPhase::Frozen {
            return self.err(req, -409, "emergency frozen; resume_all first");
        }
        // blocked → 用户确认重试：重新入队（验收计数清零）；槽位空则立即启动
        if t.state == WorkerState::Blocked {
            let from_kind = t.blocked_kind;
            self.ctx.publish(Event::TaskRequeued {
                task: task.clone(),
                from_kind,
            });
            self.try_start_queued();
            return self.ok(req, serde_json::json!({ "requeued": true }));
        }
        // C3 v1 断点续跑：failed → 回滚到最近轮锚点（round_start；首轮失败
        // 退 baseline）后重新入队。锚点快照含 .maestro session，回滚即恢复
        // 「上一完成轮」完整现场（失败轮半成品清除；pre_rollback 安全垫
        // 保证可撤销）；respawn 的 rounder 从 session 续接（LLM 记得已完成
        // 轮），只重做失败轮 —— 已完成部分保留。
        if t.state == WorkerState::Failed {
            if !self.slots_free() || self.workdir_occupied(&t.task.workdir) {
                return self.err(req, -409, "no free slot for respawn");
            }
            let workdir = std::path::Path::new(&t.task.workdir);
            // 最近 round_start 锚点；无（首轮失败 / 锚点 capture 曾失败）退 baseline
            let anchor = crate::checkpoints::list(workdir, task)
                .into_iter()
                .filter(|c| {
                    matches!(c.reason, Some(CpReason::RoundStart) | Some(CpReason::Baseline))
                })
                .max_by_key(|c| c.seq);
            let mut rolled_back = false;
            if let Some(cp) = anchor {
                // restore 失败不阻塞续跑（无锚点语义：worker 自行清理失败轮残留）
                rolled_back = crate::checkpoints::restore(
                    workdir,
                    task,
                    &CheckpointRef::new(cp.full_ref.clone()),
                    self.ctx.clock.as_ref(),
                )
                .is_ok();
            }
            self.ctx.publish(Event::TaskRequeued {
                task: task.clone(),
                from_kind: None,
            });
            self.try_start_queued();
            return self.ok(
                req,
                serde_json::json!({
                    "requeued": true,
                    "resumed_from": "failed",
                    "rolled_back": rolled_back,
                }),
            );
        }
        if t.state != WorkerState::Suspended {
            return self.err(req, -409, "task not suspended");
        }
        let Some(reason) = t.suspend.as_ref().map(|s| s.reason) else {
            return self.err(req, -409, "no suspend info");
        };
        // BudgetExceeded 恢复需确认（预算语义 v0：直接允许，预算引擎 P1 接管）
        // 轻推不在此 drain：活 worker → rounder 轮边界 TaskSteerPoll 取走；
        // 死 worker → respawn 路径前置注入（投递语义 v0.15）
        // 活 worker：解冻续跑
        if let Some(w) = &t.worker {
            if let Some(m) = self.metas.get(w) {
                let _ = worker::unfreeze_group(m.pgid);
            }
        }
        // worker 已死（竞态窗口：退出消息晚于暂停/急停到达，on_worker_exit
        // 按 Suspended 早退不再补位）→ 直接重拉进程，防「无进程的 Working」
        let worker_alive = t
            .worker
            .as_ref()
            .is_some_and(|w| self.metas.contains_key(w));
        if worker_alive {
            self.ctx.publish(Event::Resumed {
                task: task.clone(),
                worker: t.worker.clone().unwrap(),
                from_reason: reason,
                via: ResumeVia::User,
                new_session_ref: None,
            });
        } else {
            // 急停守卫已在函数顶部统一拦截（此处必非 Frozen）
            if !self.slots_free() || self.workdir_occupied(&t.task.workdir) {
                return self.err(req, -409, "no free slot for respawn");
            }
            // 挂起期间积压的轻推随 respawn 前置注入
            let prefix = self.steering_prefix(task);
            match self.spawn_worker_for(task, prefix) {
                Ok(w) => {
                    self.ctx.publish(Event::Resumed {
                        task: task.clone(),
                        worker: w,
                        from_reason: reason,
                        via: ResumeVia::User,
                        new_session_ref: None,
                    });
                }
                Err(e) => return self.err(req, -500, &format!("respawn failed: {e}")),
            }
        }
        self.ok(req, serde_json::json!({ "resumed": true }))
    }

    fn api_rollback(&mut self, req: &Request, params: CheckpointRollbackParams) -> Response {
        // 安全基线（0.11）：只允许回滚到 maestro checkpoint 命名空间
        if let Err(e) = crate::security::validate_rollback_ref(params.to.as_str()) {
            return self.err(req, -403, &e);
        }
        let Some(t) = self.ctx.authority.get(&params.task).cloned() else {
            return self.err(req, -404, "task not found");
        };
        match crate::checkpoints::restore(
            std::path::Path::new(&t.task.workdir),
            &params.task,
            &params.to,
            self.ctx.clock.as_ref(),
        ) {
            Ok(pre) => {
                self.ctx.publish(Event::CheckpointRolledBack {
                    task: params.task.clone(),
                    to: params.to.clone(),
                    pre_rollback: pre.clone(),
                });
                self.ok(
                    req,
                    serde_json::to_value(CheckpointRollbackResult {
                        rolled_back_to: params.to,
                        pre_rollback: pre,
                    })
                    .unwrap_or_default(),
                )
            }
            Err(e) => self.err(req, -500, &e),
        }
    }

    // -----------------------------------------------------------------------
    // 队列调度器（R13）
    // -----------------------------------------------------------------------

    /// 槽位判定：活 worker（含 SIGSTOP 挂起中的）各占 1。
    fn slots_free(&self) -> bool {
        self.metas.len() < self.cfg.max_parallel_workers
    }

    /// workdir 是否被某个活 worker 占用（互斥：并发写工作区 = 产物互踩）
    fn workdir_occupied(&self, workdir: &str) -> bool {
        self.metas.values().any(|m| {
            self.ctx
                .authority
                .workers
                .get(&m.id)
                .and_then(|w| self.ctx.authority.tasks.get(&w.task))
                .is_some_and(|t| t.task.workdir == workdir)
        })
    }

    /// 补位启动：槽位有空就按序启动排队任务。
    /// 优先级：先续跑（Working 但 worker 已死 —— 急停期被搁置的验收重试），
    /// 再 FIFO 启动 Queued（queue_seq 最老优先）。事件驱动，无轮询。
    /// 同一 workdir 同时只允许一个活 worker（R12 调研：并发写工作区 =
    /// checkpoint 锁冲突 + 产物互踩）。
    fn try_start_queued(&mut self) {
        if self.emergency == EmergencyPhase::Frozen {
            return;
        }
        loop {
            if !self.slots_free() {
                return;
            }
            // 被 live worker 占用的 workdir 集合
            let occupied: std::collections::HashSet<String> = self
                .metas
                .values()
                .filter_map(|m| {
                    self.ctx
                        .authority
                        .workers
                        .get(&m.id)
                        .and_then(|w| self.ctx.authority.tasks.get(&w.task))
                        .map(|t| t.task.workdir.clone())
                })
                .collect();
            let mut candidates: Vec<(bool, u64, TaskId, String)> = self
                .ctx
                .authority
                .tasks
                .values()
                .filter_map(|t| match t.state {
                    WorkerState::Queued if t.worker.is_none() => Some((
                        false,
                        t.queue_seq,
                        t.task.id.clone(),
                        t.task.workdir.clone(),
                    )),
                    // Working 但 worker 已退出：验收重试被急停拦下的续跑
                    WorkerState::Working
                        if t.worker
                            .as_ref()
                            .is_none_or(|w| !self.metas.contains_key(w)) =>
                    {
                        Some((true, t.queue_seq, t.task.id.clone(), t.task.workdir.clone()))
                    }
                    _ => None,
                })
                .filter(|(_, _, _, wd)| !occupied.contains(wd))
                .collect();
            candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let Some((is_retry, _, task, _)) = candidates.first().cloned() else {
                return;
            };
            // 排队启动无挂起语义，轻推留队（rounder poll / 终态 Dropped）
            match self.spawn_worker_for(&task, None) {
                Ok(w) => {
                    self.ctx.publish(Event::TaskStarted {
                        task: task.clone(),
                        worker: w,
                    });
                }
                Err(e) => {
                    // spawn 失败：可见地失败，任务出队（避免坏程序热循环）
                    self.ctx.publish(Event::TaskFailed {
                        task,
                        worker: WorkerId::new("none"),
                        error: format!("queue start failed (retry={is_retry}): {e}"),
                    });
                    return;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // 后台事件
    // -----------------------------------------------------------------------

    /// Worker 退出处理：按 stderr 分类（断连→suspended 自动恢复；真错→failed）
    fn on_worker_exit(&mut self, exit: worker::WorkerExit) {
        self.on_worker_exit_inner(exit);
        // 槽位已释放：补位启动排队任务（R13 事件驱动调度）
        self.try_start_queued();
    }

    fn on_worker_exit_inner(&mut self, exit: worker::WorkerExit) {
        // 先清元数据
        self.metas.remove(&exit.worker);
        crate::worker::remove_pidfile(&self.cfg.data_dir.join("workers"), &exit.worker);

        let Some(t) = self.ctx.authority.get(&exit.task).cloned() else {
            self.pre_spawn.remove(&exit.task);
            return;
        };
        // 挂起恢复路径中 CLI 退出是预期内（freeze 后 wait 线程仍可能报退出）
        if t.state == WorkerState::Suspended {
            // 快照随 worker 而死：下次 spawn 会重新采样
            self.pre_spawn.remove(&exit.task);
            return;
        }
        if t.state != WorkerState::Working {
            // 取消/失败等终态：清快照防滞留（R11 审计）
            self.pre_spawn.remove(&exit.task);
            return;
        }
        // 过期退出（R14）：该 worker 已被替换（重试/重拉），晚到的退出
        // 消息不得误杀已易主的任务 —— 只认当前 worker 的退出
        if t.worker.as_ref().is_some_and(|cur| cur != &exit.worker) {
            return;
        }

        // 断连模式 → Suspended(NetworkLost)，自动恢复（设计 §2.4）
        if crate::adapter::classify_exit(exit.exit_code, &exit.stderr_tail)
            == crate::adapter::ExitClass::Disconnect
        {
            self.ctx.publish(Event::Suspended {
                task: exit.task.clone(),
                worker: exit.worker.clone(),
                reason: SuspendReason::NetworkLost,
                session_ref: t.session_ref.clone().unwrap_or_else(|| SessionRef::new("")),
                checkpoint_ref: t
                    .checkpoint_ref
                    .clone()
                    .unwrap_or_else(|| CheckpointRef::new("")),
                round: t.round,
            });
            self.schedule_resume(&exit.task, 0);
        } else if exit.exit_code == Some(5) && exit.stderr_tail.contains("ROUNDS_EXHAUSTED") {
            // 轮数预算耗尽（R42）：半途任务不标 Done —— blocked 等用户
            // resume（session 已持久化，respawn 续接）或放弃。steering 不
            // drop（Blocked 非终态：requeue 后 respawn 前置注入积压）
            self.pre_spawn.remove(&exit.task);
            self.ctx.publish(Event::RoundsExhausted {
                task: exit.task.clone(),
                worker: exit.worker.clone(),
                rounds: t.round,
            });
        } else if exit.exit_code == Some(0) {
            self.acceptance_gate(&exit.task, &t);
        } else {
            self.ctx.publish(Event::TaskFailed {
                task: exit.task.clone(),
                worker: exit.worker.clone(),
                error: format!(
                    "exit={:?} stderr={}",
                    exit.exit_code,
                    exit.stderr_tail.chars().take(200).collect::<String>()
                ),
            });
            self.drop_pending_steering(&exit.task);
        }
    }

    /// 验收门 v0（DEV_PLAN 0.10）：exit 0 之后做读回校验 ——
    /// worktree 与启动前快照无差异 = 假完成 → AcceptanceGateFailed。
    /// 连败 3 次 → blocked(AcceptanceFailed)（Goal 3 轮语义）；否则立即重试。
    fn acceptance_gate(&mut self, task: &TaskId, t: &crate::state::TaskRecord) {
        let before = self
            .pre_spawn
            .remove(task)
            .unwrap_or(crate::acceptance::Readback::Unverifiable);
        let after = self.workdir_readback(&t.task.workdir);
        let readback = match (&before, &after) {
            // 不可观测（workdir 缺失/与数据目录重叠）→ 门退化为仅退出码
            (crate::acceptance::Readback::Unverifiable, _)
            | (_, crate::acceptance::Readback::Unverifiable) => None,
            (
                crate::acceptance::Readback::Verifiable(b),
                crate::acceptance::Readback::Verifiable(a),
            ) => Some((b.clone(), a.clone())),
        };
        let Some((before, after)) = readback else {
            self.ctx.publish(Event::AcceptanceGatePassed {
                task: task.clone(),
                round: t.round,
                output: "readback unavailable; passed on exit code only".into(),
            });
            self.ctx.publish(Event::TaskCompleted {
                task: task.clone(),
                worker: t.worker.clone().unwrap_or_else(|| WorkerId::new("none")),
                summary: "worker exited 0 (gate: exit-code only)".into(),
            });
            self.drop_pending_steering(task);
            return;
        };
        if crate::acceptance::changed(&before, &after) {
            let output = crate::acceptance::diff_summary(&before, &after);
            self.ctx.publish(Event::AcceptanceGatePassed {
                task: task.clone(),
                round: t.round,
                output: output.clone(),
            });
            // 验收通过点 = 永久 checkpoint（回滚锚点，CpReason::AcceptancePassed pinned）
            let _ = crate::checkpoints::capture(
                std::path::Path::new(&t.task.workdir),
                task,
                t.round + 1,
                CpReason::AcceptancePassed,
                self.ctx.clock.as_ref(),
            );
            self.ctx.publish(Event::TaskCompleted {
                task: task.clone(),
                worker: t.worker.clone().unwrap_or_else(|| WorkerId::new("none")),
                summary: format!("acceptance passed: {output}"),
            });
            self.drop_pending_steering(task);
            return;
        }

        // 假完成：计数 +1
        let failures = t.acceptance_failures + 1;
        self.ctx.publish(Event::AcceptanceGateFailed {
            task: task.clone(),
            round: t.round,
            failures,
            output: "no worktree changes detected (fake completion)".into(),
        });
        // publish 即应用：读回最新状态决定是否出局
        let blocked = self
            .ctx
            .authority
            .get(task)
            .map(|t| t.state == WorkerState::Blocked)
            .unwrap_or(true);
        if blocked {
            // Goal 3 轮（U10 设计 §6）：同一阻塞条件连续 3 轮无进展 → 上报 GoalProgress
            self.ctx.publish(Event::GoalProgress {
                task: task.clone(),
                round: t.round,
                blocked_condition: Some("fake_completion_x3".into()),
            });
            self.drop_pending_steering(task);
            return; // 3 振出局，等用户 TaskRequeued
        }
        // 反思回喂（aider 模式，R16）：失败差异进 steering，下一轮 worker
        // 开工即见 —— 不静默重试同样的假完成
        let m = match self.steering.push(
            task,
            format!(
                "验收门第 {failures}/3 次失败：worker 退出码 0 但 worktree 无实际产物（读回校验）。\
                 请实际产出文件后再次报告完成，不要只输出完成声明。"
            ),
        ) {
            Ok(m) => m,
            Err(_) => return, // 反思入队失败不阻断验收流程
        };
        self.ctx.publish(Event::SteeringQueued {
            task: task.clone(),
            message: m.message,
        });
        // 急停期间不启动新 worker（B12 语义）；resume_all 会补拉起
        if self.emergency == EmergencyPhase::Frozen {
            return;
        }
        // 重试轮开工前投递积压轻推（含上面注入的失败反馈）：respawn 前置注入
        // （单发 worker 经 prompt 真收到；rounder 第 1 轮即见）
        let prefix = self.steering_prefix(task);
        match self.spawn_worker_for(task, prefix) {
            Ok(w) => {
                self.ctx.publish(Event::TaskStarted {
                    task: task.clone(),
                    worker: w,
                });
            }
            Err(e) => {
                self.ctx.publish(Event::TaskFailed {
                    task: task.clone(),
                    worker: WorkerId::new("none"),
                    error: format!("acceptance retry spawn failed: {e}"),
                });
            }
        }
    }

    /// workdir 读回采样（统一排除 daemon 数据目录）
    fn workdir_readback(&self, workdir: &str) -> crate::acceptance::Readback {
        crate::acceptance::snapshot(std::path::Path::new(workdir), std::slice::from_ref(&self.cfg.data_dir))
    }

    /// 自动恢复计时到点：探测网络 → 恢复 or 记尝试次数并重排
    fn on_timer(&mut self, task: TaskId) {
        let Some(t) = self.ctx.authority.get(&task).cloned() else {
            return;
        };
        if t.state != WorkerState::Suspended || !t.is_suspended_auto() {
            return; // 已恢复/已升级，忽略过期计时
        }
        let attempt = t.resume_attempts;
        if !self.network_ok {
            if attempt + 1 >= MAX_AUTO_RESUME_ATTEMPTS {
                // 用例 A2：耗尽 → blocked(infra)，不再安排
                self.ctx.publish(Event::AutoRecoveryExhausted {
                    task: task.clone(),
                    attempts: attempt + 1,
                });
                return;
            }
            self.ctx.publish(Event::ResumeAttempt {
                task: task.clone(),
                attempt: attempt + 1,
                next_backoff_secs: backoff_secs((attempt + 1) as usize),
            });
            self.schedule_resume(&task, attempt + 1);
            return;
        }
        // 网络恢复：续跑（v0：重启 worker 进程 = 简化版 --resume；真正的 --resume 续接在多轮驱动任务接手）
        if self.emergency == EmergencyPhase::Frozen {
            return; // 急停期间不自动恢复
        }
        // 槽位满：不烧尝试次数，稍后再探（等别人释放槽位）
        if !self.slots_free() {
            self.schedule_resume(&task, attempt);
            return;
        }
        // 挂起期间积压的轻推随 respawn 前置注入（投递语义 v0.15）
        let prefix = self.steering_prefix(&task);
        match self.spawn_worker_for(&task, prefix) {
            Ok(w) => {
                self.ctx.publish(Event::Resumed {
                    task: task.clone(),
                    worker: w,
                    from_reason: t
                        .suspend
                        .as_ref()
                        .map(|s| s.reason)
                        .unwrap_or(SuspendReason::NetworkLost),
                    via: ResumeVia::Auto,
                    new_session_ref: None,
                });
            }
            Err(_) => {
                self.schedule_resume(&task, attempt + 1);
            }
        }
    }

    /// 安排自动恢复（RecoveryScheduler 语义内联实现）
    fn schedule_resume(&self, task: &TaskId, attempt: u32) {
        if attempt >= MAX_AUTO_RESUME_ATTEMPTS {
            let _ = task;
            return;
        }
        let wait = backoff_secs(attempt as usize);
        let deadline = self.ctx.clock.now_ms() + wait * 1000;
        let clock = self.ctx.clock.clone();
        let tx = self.tx.clone();
        let task = task.clone();
        std::thread::Builder::new()
            .name(format!("resume-timer-{task}"))
            .spawn(move || {
                clock.sleep_until(deadline);
                let _ = tx.send(CoreMsg::TimerFired(task));
            })
            .ok();
    }

    // -----------------------------------------------------------------------
    // 内部工具
    // -----------------------------------------------------------------------

    /// 投递语义 v0.15/0.32（rounder 接管后的统一口径）：
    /// - **respawn 路径**（resume 死 worker / 验收重试 / 自动恢复 / resume_all 补拉）
    ///   → 本方法：取走积压+未确认（take_all）+ 事件留痕 + 拼成下一轮 prompt
    ///   前缀（daemon 确认的投递 —— 不要求 worker 再 ack）
    /// - **活 worker**：poll（at-least-once）→ worker 用过后 TaskSteerAck
    /// - **终态**：drop_pending_steering（SteeringDropped，不静默）
    fn steering_prefix(&mut self, task: &TaskId) -> Option<String> {
        let msgs = self.steering.take_all(task);
        if msgs.is_empty() {
            return None;
        }
        let round = self.ctx.authority.get(task).map(|t| t.round).unwrap_or(0);
        for m in &msgs {
            self.ctx.publish(Event::SteeringDelivered {
                task: task.clone(),
                round,
                message: m.message.clone(),
            });
        }
        Some(
            msgs.iter()
                .map(|m| m.message.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// 任务终态时清空未投递轻推：每条发 SteeringDropped（审计可见，不静默丢失）
    fn drop_pending_steering(&mut self, task: &TaskId) {
        for m in self.steering.drop_all(task) {
            self.ctx.publish(Event::SteeringDropped {
                task: task.clone(),
                message: m.message,
            });
        }
    }

    /// 安排自动恢复：为 waiter 线程建立 WorkerExit→CoreMsg 转发通道。
    /// `prefix`：respawn 时前置注入的轻推内容（投递语义 v0.15 —— 单发 worker
    /// 经 prompt 真收到，rounder 在第 1 轮即见；None = 原始 prompt）。
    fn spawn_worker_for(
        &mut self,
        task: &TaskId,
        prefix: Option<String>,
    ) -> Result<WorkerId, String> {
        let t = self
            .ctx
            .authority
            .get(task)
            .cloned()
            .ok_or("task missing")?;
        // 验收门 v0：spawn 前采样（快 worker 可能在毫秒内落盘，晚采就漏判）
        let snap = self.workdir_readback(&t.task.workdir);
        let worker_id = WorkerId::new(format!("w-{}", self.next_worker));
        self.next_worker += 1;
        let prompt = match prefix {
            Some(p) if !p.is_empty() => format!("{p}\n\n{}", t.prompt),
            _ => t.prompt.clone(),
        };
        // 网关（CCR）环境优先：剔除 worker_env 中的同名键后追加，保证指向网关
        let mut extra_env = self.cfg.worker_env.clone();
        let gw_env = self.cfg.gateway.worker_env();
        extra_env.retain(|(k, _)| !gw_env.iter().any(|(gk, _)| gk == k));
        extra_env.extend(gw_env);
        let spec = SpawnSpec {
            worker: worker_id.clone(),
            task: task.clone(),
            program: self.cfg.worker_program.clone(),
            args: self.cfg.worker_args.clone(),
            workdir: PathBuf::from(&t.task.workdir),
            log_dir: self.cfg.data_dir.join("logs"),
            extra_env,
            prompt,
        };
        // WorkerExit → CoreMsg 转发（waiter 线程只懂 WorkerExit）
        let (wx, wrx) = std::sync::mpsc::channel::<worker::WorkerExit>();
        let core_tx = self.tx.clone();
        std::thread::spawn(move || {
            while let Ok(exit) = wrx.recv() {
                let _ = core_tx.send(CoreMsg::WorkerExit(exit));
            }
        });
        let meta =
            worker::spawn_worker(spec, &self.cfg.socket_path, wx).map_err(|e| e.to_string())?;
        // pidfile（孤儿清理元数据）
        let pf = worker::PidFile {
            worker: worker_id.clone(),
            task: task.clone(),
            pid: meta.pid,
            pgid: meta.pgid,
            start_time: meta.start_time,
            round: t.round,
            started_at: self.ctx.now_ms(),
        };
        worker::write_pidfile(&self.cfg.data_dir.join("workers"), &pf)
            .map_err(|e| e.to_string())?;
        self.metas.insert(worker_id.clone(), meta.clone());
        self.pre_spawn.insert(task.clone(), snap);
        self.ctx.publish(Event::WorkerSpawned {
            worker: worker_id.clone(),
            task: task.clone(),
            pid: meta.pid,
            pgid: meta.pgid,
        });
        Ok(worker_id)
    }

    fn next_seq_hint(&self) -> u64 {
        self.ctx.hub.current_seq()
    }

    fn ok(&self, req: &Request, result: serde_json::Value) -> Response {
        Response::Ok {
            id: req.id.clone(),
            result,
        }
    }

    fn err(&self, req: &Request, code: i32, msg: &str) -> Response {
        Response::Err {
            id: req.id.clone(),
            error: RpcError {
                code,
                message: msg.into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_protocol::SystemClock;
    use std::sync::Arc;

    fn cfg_for(dir: &std::path::Path) -> CoreConfig {
        CoreConfig {
            data_dir: dir.to_path_buf(),
            ..CoreConfig::default()
        }
    }

    /// 急停相位重启持久化（不持久化修复的回归）：
    /// 空冻结场景（急停时无运行任务 → 无任何 Resumed 事件可派）下，
    /// EmergencyStopped 入库 → recover 派生 Frozen（此前硬编码 None，
    /// 重启后 B12 冻结拦截失效）；EmergencyResumed 入库 → recover 派生 None。
    #[test]
    fn emergency_phase_survives_recover() {
        let tmp = tempfile::tempdir().unwrap();
        let clock = Arc::new(SystemClock);

        // 第一代：急停（空冻结 —— 恰是无 Resumed 可派的盲区）
        {
            let mut c = Core::new(cfg_for(tmp.path()), clock.clone());
            c.ctx.publish(Event::EmergencyStopped {
                workers: vec![],
                reason: "test-panic".into(),
            });
        }
        // daemon 重启：recover 应派生 Frozen
        {
            let (c, _) = Core::recover(cfg_for(tmp.path()), clock.clone());
            assert_eq!(
                c.emergency,
                EmergencyPhase::Frozen,
                "急停后重启，emergency 应派生为 Frozen"
            );
        }
        // 第二代：解除急停（直接 publish EmergencyResumed —— api 层同款路径）
        {
            let mut c = Core::new(cfg_for(tmp.path()), clock.clone());
            c.ctx.publish(Event::EmergencyResumed { resumed: vec![] });
        }
        // 再重启：应派生 None
        let (c, _) = Core::recover(cfg_for(tmp.path()), clock);
        assert_eq!(
            c.emergency,
            EmergencyPhase::None,
            "resume_all 后重启，emergency 应派生为 None"
        );
    }

    /// U7 v1 反馈闭环：task_feedback 落事件 + 项目记忆（换行压平防
    /// markdown 注入）；负面无理由 400；未知任务 404。
    #[test]
    fn feedback_records_event_and_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let clock = Arc::new(SystemClock);
        let mut c = Core::new(cfg_for(tmp.path()), clock.clone());

        // 建任务（authority 需有记录；workdir 用独立子目录）
        let wd = tmp.path().join("wd");
        std::fs::create_dir_all(&wd).unwrap();
        c.ctx.publish(Event::TaskCreated {
            task: Task {
                id: TaskId::new("t-1"),
                title: "标题\n带换行".into(),
                workdir: wd.display().to_string(),
                created_at: 0,
            },
            prompt: "p".into(),
        });

        let fb = |c: &mut Core, params: serde_json::Value| {
            c.handle_api(Request {
                id: "r".into(),
                method: Method::TaskFeedback,
                params,
            })
        };

        // 负面无理由 → 400（教训无法沉淀）
        assert!(matches!(
            fb(&mut c, serde_json::json!({"task": "t-1", "positive": false})),
            Response::Err { .. }
        ));
        // 未知任务 → 404
        assert!(matches!(
            fb(&mut c, serde_json::json!({"task": "t-x", "positive": true})),
            Response::Err { .. }
        ));

        // 负面带理由 → ok；事件 + 记忆落盘
        assert!(matches!(
            fb(
                &mut c,
                serde_json::json!({"task": "t-1", "positive": false, "reason": "格式\n不对"})
            ),
            Response::Ok { .. }
        ));

        // 事件：最后一条 FeedbackRecorded（发布即入库）
        let envs = c
            .event_store_handle()
            .expect("event store")
            .lock()
            .unwrap()
            .replay_all();
        assert!(matches!(
            envs.last().map(|e| &e.event),
            Some(Event::FeedbackRecorded { positive: false, .. })
        ));

        // 记忆：条目落 MAESTRO_MEMORY.md，标题/理由换行已压平
        let memory = std::fs::read_to_string(wd.join("MAESTRO_MEMORY.md")).unwrap();
        assert!(memory.contains("[👎] t-1"));
        assert!(memory.contains("标题 带换行"));
        assert!(memory.contains("理由：格式 不对"));
        assert!(!memory.contains("标题\n带换行"));

        // 👍 无理由 → ok（理由可选）
        assert!(matches!(
            fb(&mut c, serde_json::json!({"task": "t-1", "positive": true})),
            Response::Ok { .. }
        ));
        let memory = std::fs::read_to_string(wd.join("MAESTRO_MEMORY.md")).unwrap();
        assert!(memory.contains("[👍] t-1"));
        assert!(memory.contains("（无备注）"));
    }

    // -------------------------------------------------------------------------
    // C3 断点续跑 v1
    // -------------------------------------------------------------------------

    /// 测试用 git 仓库 workdir（checkpoint 依赖 git plumbing）
    fn git_wd(tmp: &std::path::Path) -> std::path::PathBuf {
        let wd = tmp.join("wd");
        std::fs::create_dir_all(&wd).unwrap();
        let g = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&wd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        g(&["init", "-q"]);
        g(&["config", "user.email", "t@t"]);
        g(&["config", "user.name", "t"]);
        g(&["commit", "--allow-empty", "-q", "-m", "init"]);
        wd
    }

    /// spawn 必败的配置（worker_program 指向不存在的二进制）—— 单测只验证
    /// requeue/回滚语义，spawn 机制另有覆盖；必败让三平台行为确定一致。
    fn cfg_spawn_fail(dir: &std::path::Path) -> CoreConfig {
        let mut cfg = cfg_for(dir);
        cfg.worker_program = "maestro-test-no-such-binary".into();
        cfg
    }

    /// 建任务 + 起跑（直接 publish，绕过 api_task_create 的真实 spawn）
    fn mk_working_task(c: &mut Core, id: &str, workdir: &std::path::Path) {
        c.ctx.publish(Event::TaskCreated {
            task: Task {
                id: TaskId::new(id),
                title: "t".into(),
                workdir: workdir.display().to_string(),
                created_at: 0,
            },
            prompt: "p".into(),
        });
        c.ctx.publish(Event::TaskStarted {
            task: TaskId::new(id),
            worker: WorkerId::new("w-1"),
        });
    }

    fn round_report_ok(c: &mut Core, id: &str, round: u32) {
        let resp = c.handle_api(Request {
            id: "r".into(),
            method: Method::TaskRoundReport,
            params: serde_json::json!({
                "task": id, "worker": "w-1", "round": round,
                "input_tokens": 10, "output_tokens": 5,
            }),
        });
        assert!(
            matches!(resp, Response::Ok { .. }),
            "round_report({round}) 应成功"
        );
    }

    /// C3 锚点：每轮 round_report 落 RoundStart checkpoint，滚动 GC 最近 3 个
    #[test]
    fn round_report_captures_round_start_anchor() {
        let tmp = tempfile::tempdir().unwrap();
        let wd = git_wd(tmp.path());
        let clock = Arc::new(SystemClock);
        let mut c = Core::new(cfg_spawn_fail(tmp.path()), clock.clone());
        let t = TaskId::new("t-1");
        mk_working_task(&mut c, "t-1", &wd);

        for r in 1..=5 {
            round_report_ok(&mut c, "t-1", r);
        }
        let cps = crate::checkpoints::list(&wd, &t);
        let rs: Vec<_> = cps.iter().filter(|c| c.reason == Some(CpReason::RoundStart)).collect();
        assert_eq!(rs.len(), 3, "滚动 GC 后应只剩最近 3 个锚点: {cps:?}");
        assert_eq!(
            rs.iter().map(|c| c.seq).max().unwrap(),
            cps.iter().map(|c| c.seq).max().unwrap(),
            "最近锚点应为全任务最新 checkpoint"
        );
    }

    /// C3 断点续跑主径：failed → resume 回滚到最近轮锚点（已完成轮工作 +
    /// .maestro session 一并恢复），失败轮半成品清除，任务重新入队。
    #[test]
    fn resume_failed_rolls_back_to_anchor_and_requeues() {
        let tmp = tempfile::tempdir().unwrap();
        let wd = git_wd(tmp.path());
        let clock = Arc::new(SystemClock);
        let mut c = Core::new(cfg_spawn_fail(tmp.path()), clock.clone());
        let t = TaskId::new("t-1");
        mk_working_task(&mut c, "t-1", &wd);

        // 第 1 轮完成：a.txt=v1 + session=sid-1 → round_report 落锚点
        std::fs::write(wd.join("a.txt"), "v1").unwrap();
        let sdir = wd.join(".maestro").join(crate::taskstate::dir_name("t-1"));
        std::fs::create_dir_all(&sdir).unwrap();
        std::fs::write(sdir.join("session"), "sid-1").unwrap();
        round_report_ok(&mut c, "t-1", 1);

        // 第 2 轮失败：留下半成品（a.txt 改坏 + junk.txt + session 漂移）
        std::fs::write(wd.join("a.txt"), "v2-broken").unwrap();
        std::fs::write(wd.join("junk.txt"), "half-done").unwrap();
        std::fs::write(sdir.join("session"), "sid-2-broken").unwrap();
        c.ctx.publish(Event::TaskFailed {
            task: t.clone(),
            worker: WorkerId::new("w-1"),
            error: "boom".into(),
        });

        // 断点续跑：resume 放行 failed
        let resp = c.handle_api(Request {
            id: "r".into(),
            method: Method::TaskResume,
            params: serde_json::json!({ "task": "t-1" }),
        });
        let Response::Ok { result, .. } = resp else {
            panic!("failed 任务 resume 应放行（C3 v1）");
        };
        assert_eq!(result["resumed_from"], "failed");
        assert_eq!(result["rolled_back"], true, "有锚点应已回滚");

        // 失败轮半成品已清除，第 1 轮成果完整保留
        assert_eq!(
            std::fs::read_to_string(wd.join("a.txt")).unwrap(),
            "v1",
            "已修改文件应回滚到锚点内容"
        );
        assert!(!wd.join("junk.txt").exists(), "失败轮 untracked 半成品应被 clean -fd 清除");
        assert_eq!(
            std::fs::read_to_string(sdir.join("session")).unwrap(),
            "sid-1",
            "session 应回滚到锚点（上一完成轮），respawn 从此续接"
        );

        // 重新入队已入账（事件流为证；spawn 必败会再次 TaskFailed，不影响本断言）
        let envs = c
            .event_store_handle()
            .unwrap()
            .lock()
            .unwrap()
            .replay_all();
        assert!(
            envs.iter()
                .any(|e| matches!(&e.event, Event::TaskRequeued { task, from_kind: None } if task == &t)),
            "应发布 TaskRequeued（from_kind=None）"
        );
    }

    /// C3 边界：非 git workdir（无锚点可用）→ 不回滚但照常重新入队
    #[test]
    fn resume_failed_without_anchor_still_requeues() {
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path().join("wd");
        std::fs::create_dir_all(&wd).unwrap();
        let clock = Arc::new(SystemClock);
        let mut c = Core::new(cfg_spawn_fail(tmp.path()), clock.clone());
        let t = TaskId::new("t-1");
        mk_working_task(&mut c, "t-1", &wd);
        c.ctx.publish(Event::TaskFailed {
            task: t.clone(),
            worker: WorkerId::new("w-1"),
            error: "boom".into(),
        });

        let resp = c.handle_api(Request {
            id: "r".into(),
            method: Method::TaskResume,
            params: serde_json::json!({ "task": "t-1" }),
        });
        let Response::Ok { result, .. } = resp else {
            panic!("无锚点也应放行续跑（worker 自行清理残留）");
        };
        assert_eq!(result["rolled_back"], false);
        let envs = c
            .event_store_handle()
            .unwrap()
            .lock()
            .unwrap()
            .replay_all();
        assert!(
            envs.iter()
                .any(|e| matches!(&e.event, Event::TaskRequeued { task, .. } if task == &t)),
            "无锚点也应重新入队"
        );
    }
}
