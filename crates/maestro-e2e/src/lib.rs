//! e2e 测试基建：进程内拉起 Core（带 MockClock + 真实子进程），API 直调。
//! 比走 socket 快且确定；socket 层有独立冒烟。
//!
//! 平台（R57）：e2e 依赖 Unix 进程组语义 + bash mock CLI → cfg(unix)。
//! Windows 的测试面 = 平台无关层单测（protocol/client/cli），见 CI 矩阵。

#![cfg(unix)]

use maestro_daemon::core::{Core, CoreConfig, CoreMsg, DEFAULT_MAX_PARALLEL_WORKERS};
use maestro_protocol::api::{Method, Request, Response};
use maestro_protocol::types::*;
use maestro_testkit::MockClock;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// 测试用 daemon 实例（Core 在专属线程跑主循环 + 真实双 socket 服务）。
/// socket 层必须真实存在：rounder 等 worker 子进程经 MAESTRO_SOCKET_PATH
/// 连 daemon（TaskSteerPoll 等），进程内直调通道覆盖不到这条路径。
pub struct TestDaemon {
    tx: Sender<CoreMsg>,
    pub clock: Arc<MockClock>,
    pub data_dir: PathBuf,
    _guard: maestro_daemon::server::ServerGuard,
    _tmp: tempfile::TempDir,
}

impl TestDaemon {
    /// 起一个测试 daemon：临时数据目录 + MockClock + 指定 worker 命令
    pub fn start(worker_program: &str, worker_args: &[&str]) -> Self {
        Self::start_with(worker_program, worker_args, None)
    }

    /// data_dir 可指定（persistence 测试跨实例共享）
    pub fn start_with(
        worker_program: &str,
        worker_args: &[&str],
        data_dir: Option<PathBuf>,
    ) -> Self {
        Self::build(
            worker_program,
            worker_args,
            data_dir,
            DEFAULT_MAX_PARALLEL_WORKERS,
            vec![],
        )
    }

    /// 并发上限可指定（队列调度测试用）
    pub fn start_limited(
        worker_program: &str,
        worker_args: &[&str],
        data_dir: Option<PathBuf>,
        max_parallel: usize,
    ) -> Self {
        Self::build(worker_program, worker_args, data_dir, max_parallel, vec![])
    }

    /// Worker 额外环境可指定（R37 上下文轮转阈值等，经 CoreConfig.worker_env
    /// 白名单透传给 worker 进程）
    pub fn start_with_worker_env(
        worker_program: &str,
        worker_args: &[&str],
        worker_env: Vec<(String, String)>,
    ) -> Self {
        Self::build(
            worker_program,
            worker_args,
            None,
            DEFAULT_MAX_PARALLEL_WORKERS,
            worker_env,
        )
    }

    /// 并发上限 + Worker 环境同时指定（负载混沌用，R44）
    pub fn start_limited_with_env(
        worker_program: &str,
        worker_args: &[&str],
        max_parallel: usize,
        worker_env: Vec<(String, String)>,
    ) -> Self {
        Self::build(worker_program, worker_args, None, max_parallel, worker_env)
    }

    fn build(
        worker_program: &str,
        worker_args: &[&str],
        data_dir: Option<PathBuf>,
        max_parallel: usize,
        worker_env: Vec<(String, String)>,
    ) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let dir = data_dir.unwrap_or_else(|| tmp.path().to_path_buf());
        let cfg = CoreConfig {
            data_dir: dir.clone(),
            workdir: dir.clone(),
            worker_program: worker_program.to_string(),
            worker_args: worker_args.iter().map(|s| s.to_string()).collect(),
            socket_path: dir.join("maestro.api.sock").display().to_string(),
            max_parallel_workers: max_parallel,
            default_model: "claude-sonnet-4".into(),
            worker_env,
        };
        let clock = Arc::new(MockClock::new());
        let (core, _) = Core::recover(cfg, clock.clone());
        let core = Arc::new(Mutex::new(core));
        // ⚠️ 先取 sender 再起 run 线程（run 持锁到底，外部不得再 lock）
        let (tx, hub, store) = {
            let g = core.lock().unwrap();
            (g.sender(), g.hub_handle(), g.event_store_handle())
        };
        // 真实双 socket（worker 子进程的 IPC 路径）。socket_path 由 serve
        // bind 后回填 —— 与生产 main.rs 相同的时序（Windows TCP 端口 bind 才确定）
        let (guard, api_addr) =
            maestro_daemon::server::serve(hub, tx.clone(), &dir, store).expect("socket serve");
        core.lock().unwrap().cfg.socket_path = api_addr.to_env_value();
        {
            let core = core.clone();
            std::thread::spawn(move || {
                core.lock().unwrap().run();
            });
        }
        Self {
            tx,
            clock,
            data_dir: dir,
            _guard: guard,
            _tmp: tmp,
        }
    }

    /// 直调 API（经 Core 的消息循环 —— 真实路径）；错误时 panic
    pub fn api(&self, method: Method, params: serde_json::Value) -> serde_json::Value {
        let (rtx, rrx) = std::sync::mpsc::channel();
        let req = Request {
            id: "test".into(),
            method,
            params,
        };
        self.tx.send(CoreMsg::Api(req, rtx)).unwrap();
        match rrx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Response::Ok { result, .. }) => result,
            Ok(Response::Err { error, .. }) => {
                panic!("api error {}: {}", error.code, error.message)
            }
            Err(e) => panic!("api timeout: {e}"),
        }
    }

    /// 直调 API，错误不 panic（安全测试断言错误码用）
    pub fn try_api(
        &self,
        method: Method,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, (i32, String)> {
        let (rtx, rrx) = std::sync::mpsc::channel();
        let req = Request {
            id: "test".into(),
            method,
            params,
        };
        self.tx.send(CoreMsg::Api(req, rtx)).unwrap();
        match rrx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Response::Ok { result, .. }) => Ok(result),
            Ok(Response::Err { error, .. }) => Err((error.code, error.message)),
            Err(e) => panic!("api timeout: {e}"),
        }
    }

    /// 喂裸消息（模拟 WorkerExit 等）
    pub fn feed(&self, msg: CoreMsg) {
        self.tx.send(msg).unwrap();
    }

    /// 创建任务并返回 task_id
    pub fn create_task(&self, title: &str, workdir: &Path) -> TaskId {
        self.create_task_with_prompt(title, "p", workdir)
    }

    /// prompt 可显式指定（mock CLI 的分支按 prompt 路由 —— 如「费用自洽」）
    pub fn create_task_with_prompt(&self, title: &str, prompt: &str, workdir: &Path) -> TaskId {
        let v = self.api(
            Method::TaskCreate,
            serde_json::json!({ "title": title, "prompt": prompt, "workdir": workdir.display().to_string() }),
        );
        let id = v["task"]["id"].as_str().expect("task id").to_string();
        TaskId::new(id)
    }

    /// 读任务当前状态
    pub fn task_state(&self, task: &TaskId) -> WorkerState {
        let v = self.api(
            Method::TaskGet,
            serde_json::json!({ "task": task.as_str() }),
        );
        serde_json::from_value(v["state"].clone()).expect("state")
    }

    /// 等任务到达某状态（真实时间轮询）
    pub fn wait_state(&self, task: &TaskId, want: WorkerState, timeout_ms: u64) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        while std::time::Instant::now() < deadline {
            if self.task_state(task) == want {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        // 测试卫生（R11 审计）：不留孤儿 worker、不漏 timer 线程。
        // 1. 停 socket accept 循环（释放路径给同 data_dir 的下一实例）
        self._guard.stop();
        // 2. 停 Core 循环
        let _ = self.tx.send(CoreMsg::Shutdown);
        // 3. 杀光 data_dir 名下全部 worker 进程组（pidfile 是权威清单）
        for pf in maestro_daemon::worker::scan_pidfiles(&self.data_dir.join("workers")) {
            maestro_testkit::pgid_sandbox::kill_group(pf.pgid);
        }
        // 4. 大幅推进虚拟时钟：唤醒卡在 sleep_until 的恢复 timer 线程
        self.clock.advance_ms(1 << 40);
    }
}

/// 初始化测试 git 仓库
pub fn git_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().to_path_buf();
    run(&p, &["init", "-q"]);
    run(&p, &["config", "user.email", "t@t"]);
    run(&p, &["config", "user.name", "t"]);
    run(&p, &["commit", "--allow-empty", "-q", "-m", "init"]);
    (tmp, p)
}

pub fn run(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    if out.status.success() {
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        panic!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr))
    }
}
