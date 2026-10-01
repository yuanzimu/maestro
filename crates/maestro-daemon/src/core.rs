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
    TaskCreateResult, TaskSteerParams,
};
use maestro_protocol::events::{Event, Task};
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
}

/// 默认槽位数：本地守护进程的保守起点（调研 R12 校准项）
pub const DEFAULT_MAX_PARALLEL_WORKERS: usize = 4;

/// 排队深度上限（R12 调研：溢出全排队不拒绝，但要有防风暴闸）
pub const MAX_QUEUE_DEPTH: usize = 100;

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            data_dir: std::env::temp_dir().join("maestro-data"),
            workdir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
            worker_program: "/bin/sh".into(),
            worker_args: vec!["-c".into(), "echo '(maestro placeholder worker)'".into()],
            socket_path: "/tmp/maestro.sock".into(),
            max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
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
        if let Some(s) = &store {
            let s = s.clone();
            hub.set_sink(Box::new(move |env| {
                let _ = s.lock().unwrap().append(env);
            }));
        }
        let started_at = clock.now_ms();
        let steering = SteeringQueue::open(&cfg.data_dir);
        let mut core = Self {
            ctx: Ctx::new(hub, authority, clock),
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
        };
        // 恢复时把「正在执行」的任务标记为 suspended(DaemonCrash)（仅手动恢复）
        core.mark_recovered_as_daemon_crash();
        // 崩溃前已在自动恢复退避中的任务（NetworkLost 等）：重启后继续调度，
        // 否则将永久滞留 Suspended（R11 审计发现的缺口）
        for (task, attempt, _reason) in core.ctx.authority.auto_resume_candidates() {
            core.schedule_resume(&task, attempt);
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
            Method::ServerStatus => self.api_status(),
            Method::ServerEmergencyStop => {
                let _params: EmergencyStopParams = serde_json::from_value(req.params.clone())
                    .unwrap_or(EmergencyStopParams { reason: None });
                self.api_emergency_stop()
            }
            Method::ServerResumeAll => {
                let params: ResumeAllParams =
                    serde_json::from_value(req.params.clone()).unwrap_or(ResumeAllParams {
                        steering: SteeringMode::Flush,
                    });
                self.api_resume_all(params.steering)
            }
            Method::ServerShutdown => {
                let _ = self.tx.send(CoreMsg::Shutdown);
                self.ok(&req, serde_json::json!({"shutting_down": true}))
            }
            Method::TaskCreate => self.api_task_create(&req),
            Method::TaskList => {
                let tasks: Vec<serde_json::Value> = self
                    .ctx
                    .authority
                    .tasks
                    .values()
                    .map(|t| {
                        serde_json::json!({
                            "id": t.task.id,
                            "title": t.task.title,
                            "state": t.state,
                            "round": t.round,
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
                    Some(t) => self.ok(
                        &req,
                        serde_json::json!({
                            "id": t.task.id, "title": t.task.title, "state": t.state,
                            "round": t.round, "worker": t.worker,
                            "session_ref": t.session_ref, "checkpoint_ref": t.checkpoint_ref,
                            "suspend_reason": t.suspend.as_ref().map(|s| s.reason),
                            "blocked_kind": t.blocked_kind,
                            "acceptance_failures": t.acceptance_failures,
                        }),
                    ),
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

    fn api_status(&mut self) -> Response {
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
            event_seq: self.ctx.hub.current_seq(),
        };
        Response::Ok {
            id: String::new(),
            result: serde_json::to_value(status).unwrap_or_default(),
        }
    }

    fn api_emergency_stop(&mut self) -> Response {
        let result = emergency::emergency_stop(&mut self.ctx, &self.metas, "user_panic");
        self.emergency = EmergencyPhase::Frozen;
        Response::Ok {
            id: String::new(),
            result: serde_json::to_value(&result).unwrap_or_default(),
        }
    }

    fn api_resume_all(&mut self, mode: SteeringMode) -> Response {
        let (resumed, dead_suspended) =
            emergency::resume_all(&mut self.ctx, &self.metas, &mut self.steering, mode);
        self.emergency = EmergencyPhase::None;
        // 竞态修复：急停挂起但 worker 已死的任务 —— 重拉进程而非假恢复
        for task in dead_suspended {
            if !self.slots_free() {
                break;
            }
            if let Ok(w) = self.spawn_worker_for(&task) {
                self.ctx.publish(Event::Resumed {
                    task: task.clone(),
                    worker: w,
                    from_reason: SuspendReason::EmergencyStop,
                    via: ResumeVia::ResumeAll,
                    new_session_ref: None,
                });
            }
        }
        // B12：调度器解冻 —— 补位启动冻结期入队/被搁置的任务（受槽位约束）
        self.try_start_queued();
        Response::Ok {
            id: String::new(),
            result: serde_json::json!({ "resumed": resumed }),
        }
    }

    fn api_task_create(&mut self, req: &Request) -> Response {
        let params: TaskCreateParams = match serde_json::from_value(req.params.clone()) {
            Ok(p) => p,
            Err(_) => return self.err(req, -400, "bad params"),
        };
        let workdir = params
            .workdir
            .clone()
            .unwrap_or_else(|| self.cfg.workdir.display().to_string());
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

        match self.spawn_worker_for(&task_id) {
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

    fn api_steer(&mut self, req: &Request, params: &TaskSteerParams) -> Response {
        let Some(t) = self.ctx.authority.get(&params.task).cloned() else {
            return self.err(req, -404, "task not found");
        };
        if t.state.is_terminal() {
            return self.err(req, -409, "task already finished");
        }
        let msg = self.steering.push(&params.task, params.message.clone());
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
        // blocked → 用户确认重试：重新入队（验收计数清零）；槽位空则立即启动
        if t.state == WorkerState::Blocked {
            if self.emergency == EmergencyPhase::Frozen {
                return self.err(req, -409, "emergency frozen; resume_all first");
            }
            let from_kind = t.blocked_kind;
            self.ctx.publish(Event::TaskRequeued {
                task: task.clone(),
                from_kind,
            });
            self.try_start_queued();
            return self.ok(req, serde_json::json!({ "requeued": true }));
        }
        if t.state != WorkerState::Suspended {
            return self.err(req, -409, "task not suspended");
        }
        let Some(reason) = t.suspend.as_ref().map(|s| s.reason) else {
            return self.err(req, -409, "no suspend info");
        };
        // BudgetExceeded 恢复需确认（预算语义 v0：直接允许，预算引擎 P1 接管）
        // flush steering（手动恢复走 flush 语义）
        for msg in self.steering.drain(task) {
            self.ctx.publish(Event::SteeringDelivered {
                task: task.clone(),
                round: t.round,
                message: msg.message,
            });
        }
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
            if self.emergency == EmergencyPhase::Frozen {
                return self.err(req, -409, "emergency frozen; resume_all first");
            }
            if !self.slots_free() || self.workdir_occupied(&t.task.workdir) {
                return self.err(req, -409, "no free slot for respawn");
            }
            match self.spawn_worker_for(task) {
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
            match self.spawn_worker_for(&task) {
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
        if is_disconnect_error(&exit.stderr_tail) {
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
            return; // 3 振出局，等用户 TaskRequeued
        }
        // 反思回喂（aider 模式，R16）：失败差异进 steering，下一轮 worker
        // 开工即见 —— 不静默重试同样的假完成
        let m = self.steering.push(
            task,
            format!(
                "验收门第 {failures}/3 次失败：worker 退出码 0 但 worktree 无实际产物（读回校验）。\
                 请实际产出文件后再次报告完成，不要只输出完成声明。"
            ),
        );
        self.ctx.publish(Event::SteeringQueued {
            task: task.clone(),
            message: m.message,
        });
        // 急停期间不启动新 worker（B12 语义）；resume_all 会补拉起
        if self.emergency == EmergencyPhase::Frozen {
            return;
        }
        // 重试轮开工前 flush 轻推（含上面注入的失败反馈），与手动 resume 同语义
        for msg in self.steering.drain(task) {
            self.ctx.publish(Event::SteeringDelivered {
                task: task.clone(),
                round: t.round,
                message: msg.message,
            });
        }
        match self.spawn_worker_for(task) {
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
        crate::acceptance::snapshot(std::path::Path::new(workdir), &[self.cfg.data_dir.clone()])
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
        match self.spawn_worker_for(&task) {
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

    /// 安排自动恢复：为 waiter 线程建立 WorkerExit→CoreMsg 转发通道
    fn spawn_worker_for(&mut self, task: &TaskId) -> Result<WorkerId, String> {
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
        let spec = SpawnSpec {
            worker: worker_id.clone(),
            task: task.clone(),
            program: self.cfg.worker_program.clone(),
            args: self.cfg.worker_args.clone(),
            workdir: PathBuf::from(&t.task.workdir),
            log_dir: self.cfg.data_dir.join("logs"),
            extra_env: vec![],
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

/// stderr 断连模式识别（设计 §2.4）
fn is_disconnect_error(stderr: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "connection reset",
        "connection refused",
        "timeout",
        "timed out",
        "econnrefused",
        "fetch failed",
        "network error",
        "epipe",
    ];
    let lower = stderr.to_lowercase();
    PATTERNS.iter().any(|p| lower.contains(p))
}
