//! 基准 runner：进程内拉起 daemon Core（真实 socket + rounder + bench-worker），
//! 逐任务执行 → 等终态 → 跑验收 → 收账本。与 e2e harness 同构，但跨平台
//! （bench-worker 是 Rust 二进制，不依赖 bash）且用 SystemClock（真实墙钟）。

use crate::manifest::{materialize_fixture, run_accept, BenchTask};
use maestro_daemon::core::{Core, CoreConfig, CoreMsg, DEFAULT_MAX_PARALLEL_WORKERS};
use maestro_protocol::api::{Method, Request, Response};
use maestro_protocol::SystemClock;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// runner 的一次完整运行配置。
pub struct RunConfig {
    /// 套件根目录（含各任务子目录）
    pub suite_dir: PathBuf,
    /// rounder 二进制路径
    pub rounder: PathBuf,
    /// bench-worker 二进制路径
    pub bench_worker: PathBuf,
    /// 运行根目录（daemon 数据 + 各任务 workdir 的父目录）
    pub run_root: PathBuf,
}

/// 单任务结果（报告的原子单元）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskOutcome {
    pub id: String,
    pub title: String,
    pub kind: String,
    /// done / failed / cancelled / blocked / timeout
    pub state: String,
    pub rounds: u64,
    pub wall_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub actual_cost_cents: u64,
    pub counterfactual_cost_cents: u64,
    pub saved_cents: u64,
    pub accept_pass: bool,
    pub accept_tail: String,
}

/// 进程内 daemon 实例（套件级单例：一个 daemon 服务全部任务）。
pub struct BenchDaemon {
    tx: Sender<CoreMsg>,
    _guard: maestro_daemon::server::ServerGuard,
}

impl BenchDaemon {
    /// 起基准 daemon：rounder 驱动 bench-worker，worker_env 透传套件根
    /// （bench-worker 据此定位各任务的 solution/）。
    pub fn start(cfg: &RunConfig, data_dir: &Path) -> Self {
        let core_cfg = CoreConfig {
            data_dir: data_dir.to_path_buf(),
            workdir: data_dir.to_path_buf(),
            worker_program: cfg.rounder.display().to_string(),
            worker_args: vec!["--".into(), cfg.bench_worker.display().to_string()],
            socket_path: data_dir.join("maestro.api.sock").display().to_string(),
            max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
            default_model: "claude-sonnet-4".into(),
            worker_env: vec![(
                "MAESTRO_BENCH_SUITE".into(),
                cfg.suite_dir.display().to_string(),
            )],
            gateway: maestro_daemon::gateway::GatewayConfig {
                url: String::new(),
                token: String::new(),
            },
            budget: maestro_daemon::budget::TaskBudget {
                max_cost_cents: None,
                max_wall_ms: None,
            },
        };
        let clock = Arc::new(SystemClock);
        let (core, _) = Core::recover(core_cfg, clock);
        let core = Arc::new(Mutex::new(core));
        // 先取 sender/hub/store，再起 run 线程（run 持锁到底）
        let (tx, hub, store) = {
            let g = core.lock().unwrap();
            (g.sender(), g.hub_handle(), g.event_store_handle())
        };
        let (guard, api_addr) =
            maestro_daemon::server::serve(hub, tx.clone(), data_dir, store).expect("socket serve");
        core.lock().unwrap().cfg.socket_path = api_addr.to_env_value();
        {
            let core = core.clone();
            std::thread::spawn(move || {
                core.lock().unwrap().run();
            });
        }
        Self { tx, _guard: guard }
    }

    fn api(&self, method: Method, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let (rtx, rrx) = std::sync::mpsc::channel();
        let req = Request {
            id: "bench".into(),
            method,
            params,
        };
        self.tx
            .send(CoreMsg::Api(req, rtx))
            .map_err(|e| format!("daemon 已停: {e}"))?;
        match rrx.recv_timeout(Duration::from_secs(30)) {
            Ok(Response::Ok { result, .. }) => Ok(result),
            Ok(Response::Err { error, .. }) => Err(format!("RPC {} {}", error.code, error.message)),
            Err(e) => Err(format!("RPC 超时: {e}")),
        }
    }

    fn task_state(&self, task: &str) -> String {
        self.api(Method::TaskGet, serde_json::json!({ "task": task }))
            .ok()
            .and_then(|v| v["state"].as_str().map(String::from))
            .unwrap_or_else(|| "unknown".into())
    }

    /// 等任务进入终态（done/failed/cancelled/blocked）；超时先 cancel 再标记。
    fn wait_terminal(&self, task: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let st = self.task_state(task);
            if matches!(st.as_str(), "done" | "failed" | "cancelled" | "blocked") {
                return st;
            }
            if Instant::now() >= deadline {
                // 收尾兜底：cancel 让 daemon 负责清理 worker（跨平台语义）
                let _ = self.api(Method::TaskCancel, serde_json::json!({ "task": task }));
                let grace = Instant::now() + Duration::from_secs(10);
                loop {
                    let st = self.task_state(task);
                    if matches!(st.as_str(), "done" | "failed" | "cancelled" | "blocked") {
                        return st;
                    }
                    if Instant::now() >= grace {
                        return "timeout".into();
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 执行单个基准任务：物化 fixture → 建任务 → 等终态 → 验收 → 收账本。
    pub fn run_task(&self, task: &BenchTask, cfg: &RunConfig) -> TaskOutcome {
        let m = &task.manifest;
        let workdir = cfg.run_root.join("tasks").join(&m.id).join("repo");
        let mut out = TaskOutcome {
            id: m.id.clone(),
            title: m.title.clone(),
            kind: m.kind.clone(),
            state: String::new(),
            rounds: 0,
            wall_ms: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            actual_cost_cents: 0,
            counterfactual_cost_cents: 0,
            saved_cents: 0,
            accept_pass: false,
            accept_tail: String::new(),
        };

        let prep = materialize_fixture(task, &workdir);
        if let Err(e) = prep {
            out.state = "fixture_error".into();
            out.accept_tail = e;
            return out;
        }

        // 建任务（workdir 即业务仓库；prompt = 清单原文）
        let created = self.api(
            Method::TaskCreate,
            serde_json::json!({
                "title": m.title,
                "prompt": m.prompt,
                "workdir": workdir.display().to_string(),
            }),
        );
        let task_id = match created {
            Ok(v) => match v["task"]["id"].as_str() {
                Some(id) => id.to_string(),
                None => {
                    out.state = "create_error".into();
                    out.accept_tail = "TaskCreate 结果缺 task.id".into();
                    return out;
                }
            },
            Err(e) => {
                out.state = "create_error".into();
                out.accept_tail = e;
                return out;
            }
        };

        // 等终态（mock 两轮 ~2s；真实模式轮次多，留足余量）
        out.state = self.wait_terminal(&task_id, Duration::from_secs(300));

        // 账本（失败也收 —— 失败任务的用量同样进报告）
        if let Ok(v) = self.api(Method::TaskLedger, serde_json::json!({ "task": task_id })) {
            let u64f = |k: &str| v[k].as_u64().unwrap_or(0);
            out.rounds = u64f("rounds");
            out.wall_ms = u64f("wall_ms");
            out.input_tokens = u64f("input_tokens");
            out.output_tokens = u64f("output_tokens");
            out.cache_read_tokens = u64f("cache_read_tokens");
            out.actual_cost_cents = u64f("actual_cost_cents");
            out.counterfactual_cost_cents = u64f("counterfactual_cost_cents");
            out.saved_cents = u64f("saved_cents");
        }

        // 验收（仅 done 有意义；其余状态照跑一次取证据亦可，但口径按 done 判定）
        if out.state == "done" {
            let (pass, tail) = run_accept(m, &workdir);
            out.accept_pass = pass;
            out.accept_tail = tail;
        }
        out
    }

    /// C6 省 token 报告（套件运行后的全量口径）。
    pub fn ledger_summary(&self) -> serde_json::Value {
        self.api(Method::LedgerSummary, serde_json::json!({}))
            .unwrap_or(serde_json::Value::Null)
    }

    /// 停 daemon（run 线程 + socket accept 循环）。
    pub fn shutdown(&self) {
        let _ = self.tx.send(CoreMsg::Shutdown);
    }
}

impl Drop for BenchDaemon {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 定位或构建依赖二进制（rounder / bench-worker）。
/// 查找顺序：current_exe 同目录（cargo run 产物）→ 上两级（测试二进制在
/// deps/ 下）→ CARGO_TARGET_DIR/debug（本地重定向时）→ 仓库 target/debug；
/// 仍缺则现场构建一次（e2e 同款自举）。
pub fn ensure_bin(exe_name: &str, package: &str) -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        format!("{exe_name}.exe")
    } else {
        exe_name.to_string()
    };
    let mut candidates: Vec<PathBuf> = vec![];
    if let Ok(cur) = std::env::current_exe() {
        // cargo run：target/debug/maestro-bench.exe → 同目录
        if let Some(dir) = cur.parent() {
            candidates.push(dir.join(&name));
            // cargo test：target/debug/deps/<hash>.exe → 上溯到 target/debug
            if let Some(up) = dir.parent() {
                candidates.push(up.join(&name));
            }
        }
    }
    if let Ok(td) = std::env::var("CARGO_TARGET_DIR") {
        candidates.push(Path::new(&td).join("debug").join(&name));
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("debug")
            .join(&name),
    );
    for c in &candidates {
        if c.is_file() {
            return Ok(c.clone());
        }
    }
    let st = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", package, "--bin", exe_name])
        .status()
        .map_err(|e| format!("启动 cargo 构建 {exe_name} 失败: {e}"))?;
    for c in &candidates {
        if c.is_file() {
            return Ok(c.clone());
        }
    }
    let _ = st;
    Err(format!(
        "构建 {exe_name} 后仍未找到（候选: {:?}）",
        candidates
    ))
}
