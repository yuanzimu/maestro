//! Worker 运行时：headless 子进程管理。
//!
//! 三个来自调研/同类项目的关键决策：
//! 1. **stdout/stderr 重定向到文件**（不接管道）：SIGSTOP 后子进程停止读管道，
//!    管道满会让父进程的读端永久阻塞、wait() 死锁 —— 落文件 + 按需读无此问题
//! 2. **spawn 即专属线程 wait**：防僵尸（herdr 决策）；kill/cancel 路径不依赖 Child
//! 3. **PID 文件带 start_time**（Linux /proc）：防 PID 复用误杀（oxo-flow 的双因子法）
//!
//! 平台矩阵（R57）：
//! - Linux：完整语义 —— 独立进程组（killpg 三级升级/SIGSTOP 急停）+ /proc 双因子
//! - macOS：进程组信号同 Linux；无 /proc → start_time 恒 0、组探活/身份用 kill 探测
//! - Windows：降级语义 —— 无进程组/SIGSTOP（占位 no-op），kill 走 Child 注册表，
//!   完整语义待 Job Objects（后续迭代）
//!
//! 对外门面函数签名跨平台一致（`imp` 模块按 cfg 提供实现）。

use maestro_protocol::types::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

/// Worker 启动配置
pub struct SpawnSpec {
    pub worker: WorkerId,
    pub task: TaskId,
    /// argv[0]（如 claude 或测试脚本路径）
    pub program: String,
    pub args: Vec<String>,
    /// 工作目录
    pub workdir: PathBuf,
    /// 日志目录（stdout/stderr 落盘于此）
    pub log_dir: PathBuf,
    /// 环境变量三元组之外的额外环境
    pub extra_env: Vec<(String, String)>,
    /// 任务 prompt（多轮驱动 Worker 用；ENV_PROMPT 注入）
    pub prompt: String,
}

/// 运行中的 Worker 元数据（Core 独占；Child 在 waiter 线程/注册表里）
#[derive(Debug, Clone)]
pub struct WorkerMeta {
    pub id: WorkerId,
    pub task: TaskId,
    pub pid: u32,
    /// 进程组 id。Unix：process_group(0) ⇒ pgid == pid；Windows：pid 占位
    pub pgid: u32,
    /// 进程 start_time（Linux /proc 字段 22，防 PID 复用；其他平台恒 0）
    pub start_time: u64,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

/// Core 收到的退出通知
pub struct WorkerExit {
    pub worker: WorkerId,
    pub task: TaskId,
    pub exit_code: Option<i32>,
    /// stderr 尾部（错误分类用；从文件读，无管道）
    pub stderr_tail: String,
}

/// 三元组环境变量（herdr 集成协议）
pub const ENV_WORKER_ID: &str = "MAESTRO_WORKER_ID";
pub const ENV_TASK_ID: &str = "MAESTRO_TASK_ID";
pub const ENV_SOCKET_PATH: &str = "MAESTRO_SOCKET_PATH";
/// 任务 prompt（多轮驱动 Worker 模式）
pub const ENV_PROMPT: &str = "MAESTRO_PROMPT";

/// 启动 Worker：独立进程组（Unix）+ stdout/stderr 落文件 + waiter 线程防僵尸
pub fn spawn_worker(
    spec: SpawnSpec,
    socket_path: &str,
    on_exit: Sender<WorkerExit>,
) -> std::io::Result<WorkerMeta> {
    std::fs::create_dir_all(&spec.log_dir)?;

    let stdout_path = spec.log_dir.join(format!("{}.stdout.log", spec.worker));
    let stderr_path = spec.log_dir.join(format!("{}.stderr.log", spec.worker));
    let out_f = std::fs::File::create(&stdout_path)?;
    let err_f = std::fs::File::create(&stderr_path)?;

    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.workdir)
        .env(ENV_WORKER_ID, spec.worker.as_str())
        .env(ENV_TASK_ID, spec.task.as_str())
        .env(ENV_SOCKET_PATH, socket_path)
        .env(ENV_PROMPT, &spec.prompt)
        .envs(spec.extra_env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_f))
        .stderr(Stdio::from(err_f));
    imp::decorate(&mut cmd); // Unix：独立进程组（killpg 语义的基石）

    let child = cmd.spawn()?;
    let pid = child.id();
    let pgid = pid; // Unix：process_group(0) ⇒ pgid == pid；Windows：占位
    let start_time = imp::proc_start_time(pid);

    let meta = WorkerMeta {
        id: spec.worker.clone(),
        task: spec.task.clone(),
        pid,
        pgid,
        start_time,
        stdout_path,
        stderr_path,
    };

    // spawn 即 wait（防僵尸）：waiter 线程独占 Child（Windows：注册表交接），退出即回报 Core
    let waiter_meta = meta.clone();
    imp::spawn_waiter(waiter_meta, on_exit, child)?;

    Ok(meta)
}

/// waiter 线程公共收尾：读 stderr 尾部 → 回报 Core
fn finish_exit(meta: WorkerMeta, on_exit: Sender<WorkerExit>, exit_code: Option<i32>) {
    let stderr_tail = read_tail(&meta.stderr_path, 2048);
    let _ = on_exit.send(WorkerExit {
        worker: meta.id.clone(),
        task: meta.task.clone(),
        exit_code,
        stderr_tail,
    });
}

// ---------------------------------------------------------------------------
// 平台门面（签名跨平台一致）
// ---------------------------------------------------------------------------

/// 关闭信号三级升级（herdr 决策）：HUP(250ms)→TERM(250ms)→KILL(250ms)。
/// Unix 对整个进程组；Windows 降级为单进程 Terminate。
pub fn graceful_kill_group(pgid: u32) {
    imp::graceful_kill_group(pgid)
}

/// SIGSTOP 整组（急停 FREEZE）。Windows：no-op（降级，待 Job Objects）
pub fn freeze_group(pgid: u32) -> std::io::Result<()> {
    imp::freeze_group(pgid)
}

/// SIGCONT 整组（恢复）。Windows：no-op（降级）
pub fn unfreeze_group(pgid: u32) -> std::io::Result<()> {
    imp::unfreeze_group(pgid)
}

/// 组内是否还有存活进程
pub fn group_alive(pgid: u32) -> bool {
    imp::group_alive(pgid)
}

/// SIGKILL 硬杀（graceful 后仍存活的兜底）。Windows：TerminateProcess
pub fn hard_kill_group(pgid: u32) -> bool {
    imp::hard_kill_group(pgid)
}

/// PID 是否仍是我们启动的那个进程（Linux：pid + start_time 双因子防复用；
/// macOS：kill 探活；Windows：注册表查 Child 状态）
pub fn is_our_process(pid: u32, start_time: u64) -> bool {
    imp::is_our_process(pid, start_time)
}

/// 读 /proc/<pid>/stat → (state, pgrp, start_time)。
/// Linux 专属（无 /proc 的平台返回 None；调用方见 suspend.rs 孤儿清理四象限）
pub fn proc_stat(pid: u32) -> Option<(String, u32, u64)> {
    imp::proc_stat(pid)
}

// ---------------------------------------------------------------------------
// Unix 实现：进程组信号 + /proc（Linux）
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod imp {
    use super::{finish_exit, WorkerExit, WorkerMeta};
    use nix::sys::signal::{kill as nix_kill, Signal};
    use nix::unistd::Pid;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    pub fn decorate(cmd: &mut Command) {
        cmd.process_group(0);
    }

    pub fn spawn_waiter(
        meta: WorkerMeta,
        on_exit: Sender<WorkerExit>,
        child: Child,
    ) -> std::io::Result<()> {
        std::thread::Builder::new()
            .name(format!("waiter-{}", meta.id))
            .spawn(move || {
                let mut child = child;
                let exit_code = child.wait().ok().and_then(|s| s.code());
                finish_exit(meta, on_exit, exit_code);
            })?;
        Ok(())
    }

    pub fn graceful_kill_group(pgid: u32) {
        let g = Pid::from_raw(-(pgid as i32));
        let _ = nix_kill(g, Signal::SIGHUP);
        if wait_group_gone(pgid, Duration::from_millis(250)) {
            return;
        }
        let _ = nix_kill(g, Signal::SIGTERM);
        if wait_group_gone(pgid, Duration::from_millis(250)) {
            return;
        }
        let _ = nix_kill(g, Signal::SIGKILL);
        let _ = wait_group_gone(pgid, Duration::from_millis(250));
    }

    pub fn freeze_group(pgid: u32) -> std::io::Result<()> {
        nix_kill(Pid::from_raw(-(pgid as i32)), Signal::SIGSTOP)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    pub fn unfreeze_group(pgid: u32) -> std::io::Result<()> {
        nix_kill(Pid::from_raw(-(pgid as i32)), Signal::SIGCONT)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    pub fn hard_kill_group(pgid: u32) -> bool {
        nix_kill(Pid::from_raw(-(pgid as i32)), Signal::SIGKILL).is_ok()
    }

    pub fn group_alive(pgid: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            !pids_in_group_alive(pgid).is_empty()
        }
        // macOS 无 /proc：kill(-pgid, 0) 探测（ESRCH = 组空）
        #[cfg(not(target_os = "linux"))]
        {
            nix_kill(Pid::from_raw(-(pgid as i32)), None).is_ok()
        }
    }

    pub fn is_our_process(pid: u32, start_time: u64) -> bool {
        // Linux：/proc start_time 双因子（防 PID 复用误杀）
        #[cfg(target_os = "linux")]
        {
            matches!(proc_stat(pid), Some((_, _, st)) if st == start_time)
        }
        // macOS：无 /proc —— kill 探活（start_time 恒 0，退化为单因子）
        #[cfg(not(target_os = "linux"))]
        {
            let _ = start_time;
            nix_kill(Pid::from_raw(pid as i32), None).is_ok()
        }
    }

    /// 轮询等待组清空（带超时）
    fn wait_group_gone(pgid: u32, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if !group_alive(pgid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        !group_alive(pgid)
    }

    /// Linux：扫 /proc 列组内非僵尸 PID
    #[cfg(target_os = "linux")]
    fn pids_in_group_alive(pgid: u32) -> Vec<u32> {
        let mut found = vec![];
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return found;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(pid) = name.parse::<u32>() else {
                continue;
            };
            if let Some((state, pgrp, _st)) = proc_stat(pid) {
                if pgrp == pgid && state != "Z" && state != "X" {
                    found.push(pid);
                }
            }
        }
        found
    }

    /// 读 /proc/<pid>/stat → (state, pgrp, start_time)
    /// stat 格式：pid (comm) state ppid pgrp session tty_nr tpgid flags minflt ... starttime(22)
    #[cfg(target_os = "linux")]
    pub fn proc_stat(pid: u32) -> Option<(String, u32, u64)> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = stat.rsplit(')').next()?;
        let f: Vec<&str> = after.split_whitespace().collect();
        // f[0]=state f[1]=ppid f[2]=pgrp ... f[19]=starttime（comm 后第 22 字段整体）
        let state = f.first()?.to_string();
        let pgrp = f.get(2)?.parse().ok()?;
        let start_time = f.get(19)?.parse().ok()?;
        Some((state, pgrp, start_time))
    }

    #[cfg(not(target_os = "linux"))]
    pub fn proc_stat(_pid: u32) -> Option<(String, u32, u64)> {
        None
    }

    pub fn proc_start_time(pid: u32) -> u64 {
        proc_stat(pid).map(|(_, _, st)| st).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Windows 实现：Child 注册表（降级语义，待 Job Objects）
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use super::{finish_exit, WorkerExit, WorkerMeta};
    use std::collections::HashMap;
    use std::process::{Child, Command};
    use std::sync::mpsc::Sender;
    use std::sync::{Mutex, OnceLock};

    /// Child 注册表：Windows 无进程组信号 —— kill/alive 经 Child 句柄操作。
    /// waiter 线程 take 走所有权；kill 路径 get_mut。
    fn registry() -> &'static Mutex<HashMap<u32, Child>> {
        static REG: OnceLock<Mutex<HashMap<u32, Child>>> = OnceLock::new();
        REG.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn decorate(_cmd: &mut Command) {
        // Windows 无 process_group —— 完整组语义待 Job Objects
    }

    pub fn spawn_waiter(
        meta: WorkerMeta,
        on_exit: Sender<WorkerExit>,
        child: Child,
    ) -> std::io::Result<()> {
        let pid = child.id();
        registry().lock().unwrap().insert(pid, child);
        std::thread::Builder::new()
            .name(format!("waiter-{}", meta.id))
            .spawn(move || {
                // 从注册表取回：kill 可能已先一步操作过（已退出 → None → exit_code None）
                let exit_code = registry()
                    .lock()
                    .unwrap()
                    .remove(&pid)
                    .and_then(|mut c| c.wait().ok().and_then(|s| s.code()));
                finish_exit(meta, on_exit, exit_code);
            })?;
        Ok(())
    }

    pub fn graceful_kill_group(pgid: u32) {
        // 降级：单进程 Terminate（无三级升级 —— Windows 无信号语义）
        let _ = kill_pid(pgid);
    }

    pub fn freeze_group(_pgid: u32) -> std::io::Result<()> {
        // 降级 no-op：Windows 挂起需 NtSuspendProcess/Job Objects（后续迭代）
        Ok(())
    }

    pub fn unfreeze_group(_pgid: u32) -> std::io::Result<()> {
        Ok(())
    }

    pub fn hard_kill_group(pgid: u32) -> bool {
        kill_pid(pgid)
    }

    pub fn group_alive(pgid: u32) -> bool {
        let mut reg = registry().lock().unwrap();
        reg.get_mut(&pgid)
            .map(|c| c.try_wait().map(|s| s.is_none()).unwrap_or(false))
            .unwrap_or(false)
    }

    pub fn is_our_process(pid: u32, _start_time: u64) -> bool {
        group_alive(pid)
    }

    pub fn proc_stat(_pid: u32) -> Option<(String, u32, u64)> {
        None // 无 /proc
    }

    pub fn proc_start_time(_pid: u32) -> u64 {
        0 // 无 /proc 双因子 —— 恒 0（is_our_process 走注册表）
    }

    fn kill_pid(pid: u32) -> bool {
        let mut reg = registry().lock().unwrap();
        match reg.get_mut(&pid) {
            Some(c) => c.kill().is_ok(),
            None => false,
        }
    }
}

fn read_tail(path: &Path, max: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return String::new();
    }
    let start = len.saturating_sub(max as u64);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = vec![0u8; (len - start) as usize];
    if f.read_exact(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

// ---------------------------------------------------------------------------
// PID 文件（孤儿清理的元数据来源，设计 §2.5）
// ---------------------------------------------------------------------------

/// PID 文件内容
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PidFile {
    pub worker: WorkerId,
    pub task: TaskId,
    pub pid: u32,
    pub pgid: u32,
    /// start_time（Linux /proc 双因子；其他平台 0）
    pub start_time: u64,
    pub round: u32,
    pub started_at: u64,
}

pub fn write_pidfile(dir: &Path, pf: &PidFile) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", pf.worker));
    std::fs::write(path, serde_json::to_vec(pf).unwrap())
}

pub fn read_pidfile(dir: &Path, worker: &WorkerId) -> Option<PidFile> {
    let path = dir.join(format!("{worker}.json"));
    let raw = std::fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

pub fn remove_pidfile(dir: &Path, worker: &WorkerId) {
    let _ = std::fs::remove_file(dir.join(format!("{worker}.json")));
}

/// 扫描 workers 目录的全部 pidfile
pub fn scan_pidfiles(dir: &Path) -> Vec<PidFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let raw = std::fs::read(e.path()).ok()?;
            serde_json::from_slice(&raw).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::sync::mpsc;
    #[cfg(unix)]
    use std::time::Duration;

    #[cfg(unix)]
    fn spec(program: &str, args: &[&str], tmp: &Path) -> SpawnSpec {
        SpawnSpec {
            worker: WorkerId::new("w-test"),
            task: TaskId::new("t-test"),
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            workdir: tmp.to_path_buf(),
            log_dir: tmp.join("logs"),
            extra_env: vec![],
            prompt: "p".into(),
        }
    }

    /// 基本生命周期：spawn → 元数据 → 退出回报 → 无僵尸
    #[cfg(unix)]
    #[test]
    fn spawn_exit_report() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let meta = spawn_worker(
            spec("/bin/sh", &["-c", "echo hi; exit 0"], tmp.path()),
            "/tmp/maestro.sock",
            tx,
        )
        .unwrap();
        assert!(meta.pgid == meta.pid);
        #[cfg(target_os = "linux")] // macOS 无 /proc → start_time 恒 0
        assert!(meta.start_time > 0, "应记录 start_time");
        let exit = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(exit.exit_code, Some(0));
        // stdout 落文件（不是管道）
        let out = std::fs::read_to_string(&meta.stdout_path).unwrap();
        assert!(out.contains("hi"), "stdout 应落盘: {out}");
    }

    /// 三级升级：对不响应 TERM 的进程最终 KILL 干净
    #[cfg(unix)]
    #[test]
    fn graceful_kill_escalates() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::channel();
        // trap HUP/TERM，只被 KILL 杀死
        let meta = spawn_worker(
            spec(
                "/bin/sh",
                &["-c", "trap '' HUP TERM; while true; do sleep 1; done"],
                tmp.path(),
            ),
            "/tmp/maestro.sock",
            tx,
        )
        .unwrap();
        assert!(group_alive(meta.pgid));
        graceful_kill_group(meta.pgid);
        assert!(!group_alive(meta.pgid), "三级升级后组应清空");
    }

    /// PID 复用防护：start_time 不匹配即非我们的进程（Linux /proc 双因子）
    #[cfg(target_os = "linux")]
    #[test]
    fn pid_reuse_protection() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::channel();
        let meta = spawn_worker(
            spec("/bin/sh", &["-c", "sleep 60"], tmp.path()),
            "/tmp/maestro.sock",
            tx,
        )
        .unwrap();
        // 我们自己的进程：start_time 匹配
        assert!(is_our_process(meta.pid, meta.start_time));
        // start_time 不匹配（模拟复用）：不是我们的
        assert!(!is_our_process(meta.pid, meta.start_time + 999));
        graceful_kill_group(meta.pgid);
    }

    /// pidfile 往返 + 扫描（平台无关）
    #[test]
    fn pidfile_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let pf = PidFile {
            worker: WorkerId::new("w1"),
            task: TaskId::new("t1"),
            pid: 42,
            pgid: 42,
            start_time: 777,
            round: 3,
            started_at: 123456,
        };
        write_pidfile(tmp.path(), &pf).unwrap();
        let back = read_pidfile(tmp.path(), &WorkerId::new("w1")).unwrap();
        assert_eq!(back.start_time, 777);
        assert_eq!(scan_pidfiles(tmp.path()).len(), 1);
        remove_pidfile(tmp.path(), &WorkerId::new("w1"));
        assert!(scan_pidfiles(tmp.path()).is_empty());
    }

    /// 环境三元组注入验证
    #[cfg(unix)]
    #[test]
    fn env_triple_injected() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let meta = spawn_worker(
            spec(
                "/bin/sh",
                &["-c", "env | grep MAESTRO_ | sort > env.txt; exit 0"],
                tmp.path(),
            ),
            "/tmp/maestro.sock",
            tx,
        )
        .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let env = std::fs::read_to_string(tmp.path().join("env.txt")).unwrap();
        assert!(env.contains("MAESTRO_WORKER_ID=w-test"));
        assert!(env.contains("MAESTRO_TASK_ID=t-test"));
        assert!(env.contains("MAESTRO_SOCKET_PATH=/tmp/maestro.sock"));
        let _ = meta;
    }

    /// stderr 尾部读取（错误分类用）
    #[cfg(unix)]
    #[test]
    fn stderr_tail_captured() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        spawn_worker(
            spec(
                "/bin/sh",
                &["-c", "echo 'connection reset by peer' >&2; exit 1"],
                tmp.path(),
            ),
            "/tmp/maestro.sock",
            tx,
        )
        .unwrap();
        let exit = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(exit.exit_code, Some(1));
        assert!(
            exit.stderr_tail.contains("connection reset"),
            "stderr 尾部应含错误: {}",
            exit.stderr_tail
        );
    }
}
