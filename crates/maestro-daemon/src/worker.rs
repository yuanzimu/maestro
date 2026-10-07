//! Worker 运行时：headless 子进程管理。
//!
//! 三个来自调研/同类项目的关键决策：
//! 1. **stdout/stderr 重定向到文件**（不接管道）：SIGSTOP 后子进程停止读管道，
//!    管道满会让父进程的读端永久阻塞、wait() 死锁 —— 落文件 + 按需读无此问题
//! 2. **spawn 即专属线程 wait**：防僵尸（herdr 决策）；kill/cancel 路径不依赖 Child
//! 3. **PID 文件带 start_time**（Linux /proc）：防 PID 复用误杀（oxo-flow 的双因子法）
//!
//! 平台矩阵（R57 + Sprint C C1）：
//! - Linux：完整语义 —— 独立进程组（killpg 三级升级/SIGSTOP 急停）+ /proc 双因子
//! - macOS：进程组信号同 Linux；无 /proc → start_time 恒 0、组探活/身份用 kill 探测
//! - Windows：Job Objects 语义 —— 每 worker 一个 kill-on-close Job（防孙进程泄漏），
//!   FREEZE 枚举 Job 内进程逐线程挂起、RESUME 逆操作，关闭两级 CTRL_BREAK →
//!   TerminateJobObject（整组含孙进程）；快照前 workdir 静止确认（C1-5）
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

/// SIGKILL 硬杀（graceful 后仍存活的兜底）。Windows：TerminateJobObject 整组
pub fn hard_kill_group(pgid: u32) -> bool {
    imp::hard_kill_group(pgid)
}

/// 急停快照静止确认（C1-5）：FREEZE 后、SNAPSHOT 前调用。
/// Windows：短轮询 workdir 目录项指纹（名字/大小/mtime）直至连续两轮稳定 ——
/// 真冻结应瞬时静止，不稳说明有外部写入者（另一 worktree/杀毒扫描）；
/// Unix：SIGSTOP 已静止文件系统，恒 true（零开销直通）。
/// 返回是否静止；调用方 best-effort 继续（快照失败路径已有如实上报）。
pub fn settle_workdir(workdir: &Path) -> bool {
    imp::settle_workdir(workdir)
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
        // 先起线程，再经 channel 交接 Child：这样线程创建失败时 Child
        // 仍在调用方手里，可干净 kill+wait（std 的 Child drop 既不 kill
        // 也不 wait，直接 move 进失败闭包会泄漏进程并留僵尸）。
        let (tx, rx) = std::sync::mpsc::channel::<Child>();
        let h = std::thread::Builder::new()
            .name(format!("waiter-{}", meta.id))
            .spawn(move || {
                // Unix：kill 走进程组（pgid），不依赖 Child 句柄，
                // waiter 本地持有并阻塞 wait 即可
                if let Ok(mut child) = rx.recv() {
                    let exit_code = child.wait().ok().and_then(|s| s.code());
                    finish_exit(meta, on_exit, exit_code);
                }
            });
        match h {
            Ok(_) => {
                if let Err(mut send_err) = tx.send(child) {
                    // 线程在 recv 前死亡（极端）：兜底回收。
                    // SendError<T> 把未送达的值放在 .0
                    let _ = send_err.0.kill();
                    let _ = send_err.0.wait();
                }
                Ok(())
            }
            Err(e) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                Err(e)
            }
        }
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

    /// Unix：SIGSTOP 冻结后文件系统已静止，无需轮询（C1-5 平台直通）
    pub fn settle_workdir(_workdir: &std::path::Path) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Windows 实现：Job Objects（Sprint C C1）+ Child 注册表
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use super::{finish_exit, WorkerExit, WorkerMeta};
    use std::collections::HashMap;
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command};
    use std::sync::mpsc::Sender;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenThread, ResumeThread, SuspendThread, THREAD_SUSPEND_RESUME,
    };

    // windows-sys 0.52 的 HANDLE = isize：Job/线程句柄直接以 isize 存储（跨线程
    // 进 static 容器安全，无需 Send 包装）
    type Raw = isize;

    /// 每 worker 的 Job 单元：Job 句柄 + 冻结中线程句柄（freeze/unfreeze 配对）
    #[derive(Default)]
    struct JobCell {
        job: Raw,
        frozen: Vec<Raw>,
    }

    fn jobs() -> &'static Mutex<HashMap<u32, JobCell>> {
        static J: OnceLock<Mutex<HashMap<u32, JobCell>>> = OnceLock::new();
        J.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Child 注册表：waiter 线程 take 走所有权；kill/alive 路径 get_mut。
    fn registry() -> &'static Mutex<HashMap<u32, Child>> {
        static REG: OnceLock<Mutex<HashMap<u32, Child>>> = OnceLock::new();
        REG.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn decorate(cmd: &mut Command) {
        // 子进程作为进程组长（CREATE_NEW_PROCESS_GROUP）——CTRL_BREAK 可定向投递
        // 到该组（graceful 第一级），且不影响 daemon 自身所在组
        cmd.creation_flags(0x0000_0200);
    }

    pub fn spawn_waiter(
        meta: WorkerMeta,
        on_exit: Sender<WorkerExit>,
        child: Child,
    ) -> std::io::Result<()> {
        let pid = child.id();
        // C1-2：绑定 kill-on-close Job（best-effort —— 失败退回注册表单进程语义，
        // 但仍保留冻结/整组终止之外的兜底 kill）
        attach_job(&child);
        // child 必须**保留**在注册表：kill/alive 全靠 Child 句柄。若 waiter 一启动
        // 就 remove 走，之后 kill_pid 永远找不到，取消/急停实际从不生效。
        registry().lock().unwrap().insert(pid, child);
        let thread = std::thread::Builder::new()
            .name(format!("waiter-{}", meta.id))
            .spawn(move || {
                // 周期性 try_wait 轮询（每轮短暂持锁，不阻塞 kill/alive）：
                // 自然退出或被终止都会在此被发现
                loop {
                    let exited = {
                        let mut reg = registry().lock().unwrap();
                        match reg.get_mut(&pid) {
                            Some(c) => c.try_wait().ok().flatten(),
                            None => None,
                        }
                    };
                    match exited {
                        Some(status) => {
                            // 进程已退出：锁内 remove 取出（锁立即释放），
                            // 再收尾 Job（关 kill-on-close 句柄 → 兜底清仍在组的孙进程）
                            let removed = registry().lock().unwrap().remove(&pid);
                            let exit_code = removed.and_then(|_| status.code());
                            reap_job(pid);
                            finish_exit(meta, on_exit, exit_code);
                            break;
                        }
                        None => std::thread::sleep(Duration::from_millis(100)),
                    }
                }
            });
        match thread {
            Ok(_) => Ok(()),
            Err(e) => {
                // 线程创建失败：回滚注册表/Job 并显式 kill+wait，不泄漏句柄/进程
                if let Some(mut c) = registry().lock().unwrap().remove(&pid) {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                reap_job(pid);
                Err(e)
            }
        }
    }

    /// C1-2：每 worker 一个 kill-on-close Job —— daemon 崩溃即整组清理
    /// （堵住「孙进程泄漏」：worker CLI 派生的子进程自动继承 Job 成员资格）
    fn attach_job(child: &Child) {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job == 0 {
                eprintln!("maestro: Job 创建失败，该 worker 降级为单进程语义");
                return;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                eprintln!("maestro: Job kill-on-close 设置失败，降级单进程语义");
                CloseHandle(job);
                return;
            }
            if AssignProcessToJobObject(job, child.as_raw_handle() as Raw) == 0 {
                eprintln!("maestro: Job 绑定失败（进程可能已退出），降级单进程语义");
                CloseHandle(job);
                return;
            }
            jobs().lock().unwrap().insert(
                child.id(),
                JobCell {
                    job: job as Raw,
                    frozen: vec![],
                },
            );
        }
    }

    /// waiter 收尾：释放冻结线程句柄并关 Job —— 关闭动作触发 kill-on-close，
    /// 兜底清掉主进程已退出但仍在组内滞留的孙进程
    fn reap_job(pid: u32) {
        if let Some(cell) = jobs().lock().unwrap().remove(&pid) {
            for th in cell.frozen {
                unsafe { CloseHandle(th as _) };
            }
            unsafe { CloseHandle(cell.job as _) };
        }
    }

    /// Job 内全部进程 pid（含孙进程 —— C1 的核心收益：枚举以 Job 为准，
    /// 不靠父子关系推断，CLI 换任何派生方式都逃不出组）。
    /// 走 BasicProcessIdList：直接返回 pid 数组，无句柄管理负担
    unsafe fn job_pids(job: Raw) -> Vec<u32> {
        let mut slots = 64usize;
        for _ in 0..2 {
            // 布局 = JOBOBJECT_BASIC_PROCESS_ID_LIST：[assigned, in_list, pid...]
            let mut buf: Vec<u32> = vec![0; slots + 2];
            let ok = QueryInformationJobObject(
                job,
                JobObjectBasicProcessIdList,
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                (buf.len() * size_of::<u32>()) as u32,
                std::ptr::null_mut(),
            );
            let assigned = buf[0] as usize;
            let in_list = buf[1] as usize;
            if ok != 0 {
                return buf[2..2 + in_list].to_vec();
            }
            // 失败：组已终止/空（返回空）或缓冲不足（按 assigned 扩容重试一次）
            if assigned > slots && slots < 8192 {
                slots = assigned + 8;
                continue;
            }
            return vec![];
        }
        vec![]
    }

    /// 逐线程挂起 Job 内全部进程（C1-3 真冻结）。返回被挂起的线程句柄
    /// （unfreeze 逐个 Resume 一次 + 关闭 —— Suspend 计数精确配对）
    unsafe fn suspend_threads_of(pids: &[u32]) -> Vec<Raw> {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return vec![];
        }
        let mut te: THREADENTRY32 = std::mem::zeroed();
        te.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut out = vec![];
        if Thread32First(snap, &mut te) != 0 {
            loop {
                if pids.contains(&te.th32OwnerProcessID) {
                    let th = OpenThread(THREAD_SUSPEND_RESUME, 0, te.th32ThreadID);
                    if th != 0 {
                        if SuspendThread(th) != 0xFFFF_FFFF {
                            out.push(th as Raw);
                        } else {
                            // 线程已退出等：直接关闭，不留悬空计数
                            CloseHandle(th);
                        }
                    }
                }
                if Thread32Next(snap, &mut te) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        out
    }

    pub fn freeze_group(pgid: u32) -> std::io::Result<()> {
        // 句柄先复制出锁再 FFI（不持锁做系统调用；reap_job 并发安全）
        let job = {
            jobs()
                .lock()
                .unwrap()
                .get(&pgid)
                .map(|c| c.job)
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no job for worker")
                })?
        };
        unsafe {
            let pids = job_pids(job);
            // 空组 = 主进程已退出（竞态窗口）：vacuously frozen，交给 waiter 路径
            if !pids.is_empty() {
                let suspended = suspend_threads_of(&pids);
                if let Some(cell) = jobs().lock().unwrap().get_mut(&pgid) {
                    cell.frozen.extend(suspended);
                }
            }
        }
        Ok(())
    }

    pub fn unfreeze_group(pgid: u32) -> std::io::Result<()> {
        let threads: Vec<Raw> = {
            match jobs().lock().unwrap().get_mut(&pgid) {
                Some(cell) => std::mem::take(&mut cell.frozen),
                // 无 Job（attach 失败的降级路径）：冻结本就是 no-op，恢复亦然
                None => vec![],
            }
        };
        for th in threads {
            unsafe {
                ResumeThread(th as _);
                CloseHandle(th as _);
            }
        }
        Ok(())
    }

    /// C1-4 两级关闭：CTRL_BREAK（共享控制台时 CLI 有保存现场窗口）→
    /// 超时后 TerminateJobObject 整组硬杀。原「直接 Terminate」无优雅期。
    pub fn graceful_kill_group(pgid: u32) {
        // 第一级：定向投递到子进程组（daemon 与子组不同组，自身不受影响；
        // 无共享控制台/子进程不处理时调用失败，自然落到第二级）
        unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pgid) };
        if wait_gone(pgid, Duration::from_millis(2000)) {
            return;
        }
        let job = jobs().lock().unwrap().get(&pgid).map(|c| c.job);
        match job {
            Some(job) => unsafe {
                TerminateJobObject(job as _, 1);
            },
            None => {
                let _ = kill_pid(pgid);
            }
        }
        let _ = wait_gone(pgid, Duration::from_millis(1000));
    }

    pub fn hard_kill_group(pgid: u32) -> bool {
        let job = jobs().lock().unwrap().get(&pgid).map(|c| c.job);
        match job {
            Some(job) => unsafe { TerminateJobObject(job as _, 1) != 0 },
            None => kill_pid(pgid),
        }
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

    /// C1-5：workdir 静止确认 —— 目录项（名字/大小/mtime）指纹连续两轮一致。
    /// 真冻结后应 1 轮即稳；不稳定说明有外部写入者，返回 false 由调用方决策。
    pub fn settle_workdir(workdir: &Path) -> bool {
        let mut prev = fingerprint(workdir);
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(80));
            let cur = fingerprint(workdir);
            if cur == prev {
                return true;
            }
            prev = cur;
        }
        false
    }

    fn fingerprint(dir: &Path) -> Vec<(String, u64, u64)> {
        let mut out = vec![];
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if let Ok(md) = e.metadata() {
                    let mtime = md
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    out.push((
                        e.file_name().to_string_lossy().into_owned(),
                        md.len(),
                        mtime,
                    ));
                }
            }
        }
        out.sort();
        out
    }

    fn kill_pid(pid: u32) -> bool {
        let mut reg = registry().lock().unwrap();
        match reg.get_mut(&pid) {
            Some(c) => c.kill().is_ok(),
            None => false,
        }
    }

    /// 轮询等待组清空（带超时；组 = 注册表里的主进程，孙进程随 Job 一并终止）
    fn wait_gone(pgid: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !group_alive(pgid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        !group_alive(pgid)
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

    // -------------------------------------------------------------------
    // Windows 进程面测试（Sprint C C1-8：把急停/孤儿回收关键行为做成
    // Win 可跑的自动化，脱离 e2e 的 #![cfg(unix)] 限制）
    // -------------------------------------------------------------------

    #[cfg(windows)]
    use std::time::{Duration, Instant};

    #[cfg(windows)]
    use std::sync::mpsc;

    #[cfg(windows)]
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

    /// 测试专用进程枚举（与 imp 同一 windows-sys 依赖，不走 FFI 封装）
    #[cfg(windows)]
    mod winproc {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        /// 指定 pid 的**直接子进程**清单（找 start /b 派生的孙进程）
        pub fn children_of(parent: u32) -> Vec<u32> {
            unsafe {
                let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
                if snap == INVALID_HANDLE_VALUE {
                    return vec![];
                }
                let mut pe: PROCESSENTRY32W = std::mem::zeroed();
                pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
                let mut out = vec![];
                if Process32FirstW(snap, &mut pe) != 0 {
                    loop {
                        if pe.th32ParentProcessID == parent && pe.th32ProcessID != parent {
                            out.push(pe.th32ProcessID);
                        }
                        if Process32NextW(snap, &mut pe) == 0 {
                            break;
                        }
                    }
                }
                CloseHandle(snap);
                out
            }
        }

        /// pid 是否存活（OpenProcess 探测；进程退出且句柄全关后即失效）
        pub fn alive(pid: u32) -> bool {
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if h == 0 {
                    return false;
                }
                CloseHandle(h);
                true
            }
        }
    }

    /// 等待文件持续增长（写循环在跑的证据）；超时即 panic
    #[cfg(windows)]
    fn wait_grow(f: &std::path::Path, ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut last = 0u64;
        while Instant::now() < deadline {
            let cur = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
            if last > 0 && cur > last {
                return;
            }
            last = cur;
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("文件未持续增长（写循环没跑起来？）: {f:?}");
    }

    /// 间隔 ms 两次采样大小是否一致（冻结后写入停止的证据）
    #[cfg(windows)]
    fn size_stable(f: &std::path::Path, ms: u64) -> bool {
        let a = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(ms));
        let b = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        a == b
    }

    /// 等待 pid 消失（含 daemon 持句柄的注册表收尾延迟）
    #[cfg(windows)]
    fn wait_pid_gone(pid: u32, ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            if !winproc::alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !winproc::alive(pid)
    }

    /// 生命周期对齐 unix 用例：spawn → 退出码 → stdout 落盘（Windows 基线）
    #[cfg(windows)]
    #[test]
    fn win_spawn_exit_report() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let meta = spawn_worker(spec("cmd", &["/c", "echo hi"], tmp.path()), "pipe", tx).unwrap();
        // Windows pgid 语义：pid 占位（Job 才是真正的「组」）
        assert_eq!(meta.pgid, meta.pid);
        let exit = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(exit.exit_code, Some(0));
        let out = std::fs::read_to_string(&meta.stdout_path).unwrap();
        assert!(out.contains("hi"), "stdout 应落盘: {out}");
    }

    /// C1-3 真冻结主径：FREEZE 后写入停止 → RESUME 后继续 → hard kill 整组清空。
    /// 写循环 = cmd 的 for /l 死循环追加 tick.txt（cmd 自身单进程执行，挂起其
    /// 主线程即冻结全部写入 —— 与急停对真实 CLI 的冻结路径一致）
    #[cfg(windows)]
    #[test]
    fn win_freeze_halts_writes_and_resume_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::channel();
        let script = "for /l %i in (1,0,1) do (echo x >> tick.txt)";
        let meta = spawn_worker(spec("cmd", &["/c", script], tmp.path()), "pipe", tx).unwrap();
        let tick = tmp.path().join("tick.txt");
        wait_grow(&tick, 5000);

        freeze_group(meta.pgid).expect("真冻结应成功");
        assert!(
            size_stable(&tick, 250),
            "FREEZE 后 250ms 内仍无新写入应成立（快照静止前提）"
        );

        unfreeze_group(meta.pgid).expect("恢复应成功");
        wait_grow(&tick, 5000);

        assert!(hard_kill_group(meta.pgid), "Terminate Job 应成功");
        assert!(wait_pid_gone(meta.pid, 3000), "整组应清空");
        assert!(!group_alive(meta.pgid));
    }

    /// C1-2 核心：Terminate Job 整组含孙进程 —— 堵住「孙进程泄漏」。
    /// 孙进程 = start /b 派生的后台 cmd（跑 ping 保持存活），与主进程同 Job
    #[cfg(windows)]
    #[test]
    fn win_job_terminate_kills_grandchildren() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::channel();
        let script =
            "start /b cmd /c ping -n 60 127.0.0.1 & for /l %i in (1,0,1) do (echo x >> tick.txt)";
        let meta = spawn_worker(spec("cmd", &["/c", script], tmp.path()), "pipe", tx).unwrap();
        let tick = tmp.path().join("tick.txt");
        wait_grow(&tick, 5000);

        // 等孙进程出现（start /b 是异步的）
        let mut grand = None;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(p) = winproc::children_of(meta.pid).first().cloned() {
                grand = Some(p);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let grand = grand.expect("start /b 应派生孙进程");

        assert!(hard_kill_group(meta.pgid), "Terminate Job 应成功");
        assert!(wait_pid_gone(meta.pid, 3000), "主进程应被终止");
        assert!(
            wait_pid_gone(grand, 3000),
            "孙进程应随 Job 一并终止（此前泄漏到任务结束）"
        );
        assert!(!group_alive(meta.pgid));
    }

    /// C1-4 两级关闭收敛性：CTRL_BREAK 无效/无共享控制台时，
    /// 第二级 Terminate Job 兜底，最终组清空（超时窗口内）
    #[cfg(windows)]
    #[test]
    fn win_graceful_kill_terminates_stubborn_process() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::channel();
        let script = "for /l %i in (1,0,1) do (echo x >> tick.txt)";
        let meta = spawn_worker(spec("cmd", &["/c", script], tmp.path()), "pipe", tx).unwrap();
        wait_grow(&tmp.path().join("tick.txt"), 5000);

        graceful_kill_group(meta.pgid);

        let deadline = Instant::now() + Duration::from_secs(6);
        while group_alive(meta.pgid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!group_alive(meta.pgid), "两级关闭后组应清空");
        assert!(wait_pid_gone(meta.pid, 3000));
    }

    /// is_our_process 注册表语义（Windows 单因子）：活=ours，kill 后即非
    #[cfg(windows)]
    #[test]
    fn win_is_our_process_registry_semantics() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::channel();
        let script = "for /l %i in (1,0,1) do (echo x >> tick.txt)";
        let meta = spawn_worker(spec("cmd", &["/c", script], tmp.path()), "pipe", tx).unwrap();
        wait_grow(&tmp.path().join("tick.txt"), 5000);
        assert!(is_our_process(meta.pid, meta.start_time));
        assert!(hard_kill_group(meta.pgid));
        assert!(wait_pid_gone(meta.pid, 3000));
        assert!(!is_our_process(meta.pid, meta.start_time), "死后不得再认领");
    }
}
