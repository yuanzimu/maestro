//! suspended 状态机的恢复调度器 + 孤儿清理（0.17，设计 §2）。
//!
//! 职责：
//! - 恢复调度：对 auto 策略的挂起任务按退避表定时尝试（线程池 → 回消息给 Core）
//! - 孤儿清理：daemon 启动时扫 pidfile，绝不收养一律杀（设计 §2.5）

use crate::worker::{self};
use maestro_protocol::types::*;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// 恢复调度器发给 Core 的消息
pub enum RecoveryMsg {
    /// 退避计时到点：该尝试一次自动恢复
    TimerFired { task: TaskId },
}

/// 恢复调度器：为 auto 挂起任务安排下一次尝试。
/// 每个任务一个计时线程（MockClock 下虚拟等待，不烧 CPU）。
pub struct RecoveryScheduler {
    clock: Arc<dyn maestro_protocol::Clock>,
    tx: Sender<RecoveryMsg>,
}

impl RecoveryScheduler {
    pub fn new(clock: Arc<dyn maestro_protocol::Clock>, tx: Sender<RecoveryMsg>) -> Self {
        Self { clock, tx }
    }

    /// 安排（或重排）某任务的下一次自动恢复尝试。
    /// attempt = 已尝试次数（0 起）。到达 MAX 后由 Core 发 AutoRecoveryExhausted，不再安排。
    pub fn schedule(&self, task: &TaskId, attempt: u32) {
        if attempt >= MAX_AUTO_RESUME_ATTEMPTS {
            return; // Core 侧已升级 blocked；防御性兜底
        }
        let wait = backoff_secs(attempt as usize);
        let deadline = self.clock.now_ms() + wait * 1000;
        let tx = self.tx.clone();
        let clock = self.clock.clone();
        let task = task.clone();
        std::thread::Builder::new()
            .name(format!("resume-timer-{}", task))
            .spawn(move || {
                clock.sleep_until(deadline);
                let _ = tx.send(RecoveryMsg::TimerFired { task });
            })
            .ok();
    }
}

/// 孤儿清理结果（设计 §2.5 四象限）
#[derive(Debug, Default)]
pub struct ReapReport {
    /// 进程已死，仅清文件
    pub cleaned_files: Vec<WorkerId>,
    /// 我们进程组的存活进程：SIGKILL + 标记 suspended(DaemonCrash)
    pub killed: Vec<WorkerId>,
    /// PID 被外来进程复用：只清文件不碰进程
    pub pid_reused: Vec<WorkerId>,
}

/// daemon 启动时的孤儿清理：扫 workers/ 目录全部 pidfile。
/// 原则（设计 §2.5）：**绝不收养，一律杀** —— session ref + checkpoint 保证可恢复。
pub fn reap_orphans(workers_dir: &Path) -> ReapReport {
    let mut report = ReapReport::default();
    for pf in worker::scan_pidfiles(workers_dir) {
        if worker::is_our_process(pf.pid, pf.start_time) {
            // 我们的进程还活着（stopped 或 running）：杀（不收养）
            worker::graceful_kill_group(pf.pgid);
            if worker::group_alive(pf.pgid) {
                let _ = worker::hard_kill_group(pf.pgid);
            }
            report.killed.push(pf.worker.clone());
        } else if worker::proc_stat(pf.pid).is_some() {
            // PID 存在但 start_time 不匹配 → 被外来进程复用，不碰
            // （/proc 是 Linux 专属；其他平台该分支恒 false —— 见 worker::proc_stat）
            report.pid_reused.push(pf.worker.clone());
        } else {
            // 进程已死：仅清文件（状态以事件重放为准）
            report.cleaned_files.push(pf.worker.clone());
        }
        worker::remove_pidfile(workers_dir, &pf.worker);
    }
    report
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::PidFile;
    #[cfg(unix)]
    use crate::worker::{spawn_worker, SpawnSpec};
    use maestro_protocol::types::{TaskId, WorkerId};

    fn reap_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// 用例 A8 象限①：进程已死 → 只清文件
    #[test]
    fn reap_dead_process_cleans_file() {
        let tmp = reap_dir();
        let workers = tmp.path().join("workers");
        write_pf(&workers, "w-dead", 4_000_000, 999);
        let report = reap_orphans(&workers);
        assert_eq!(report.cleaned_files.len(), 1);
        assert!(report.killed.is_empty());
        assert!(worker::scan_pidfiles(&workers).is_empty(), "文件应清掉");
    }

    /// 用例 A8 象限②③：我们的进程还活着（stopped 或 running）→ 杀 + 报告
    #[cfg(unix)]
    #[test]
    fn reap_live_our_process_kills() {
        let tmp = reap_dir();
        let workers = tmp.path().join("workers");
        let (tx, _rx) = std::sync::mpsc::channel();
        let meta = spawn_worker(
            SpawnSpec {
                worker: WorkerId::new("w-live"),
                task: TaskId::new("t1"),
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "sleep 300".into()],
                workdir: tmp.path().into(),
                log_dir: tmp.path().join("logs"),
                extra_env: vec![],
                prompt: "p".into(),
            },
            "/tmp/x.sock",
            tx,
        )
        .unwrap();
        write_pf(&workers, "w-live", meta.pid, meta.start_time);
        let report = reap_orphans(&workers);
        assert_eq!(report.killed.len(), 1);
        assert!(!worker::group_alive(meta.pgid), "组应被杀干净");
    }

    /// 用例 A8 象限④：PID 被外来进程复用（start_time 不匹配）→ 不碰进程只清文件。
    /// 依赖 /proc 双因子判定 → Linux 专属
    #[cfg(target_os = "linux")]
    #[test]
    fn reap_pid_reuse_touches_nothing() {
        let tmp = reap_dir();
        let workers = tmp.path().join("workers");
        // PID 1 (init) 一定存在但 start_time 与伪造值不符
        write_pf(&workers, "w-reused", 1, 12345);
        let report = reap_orphans(&workers);
        // init 没被杀（我们还在跑测试就是证明），文件清了
        assert_eq!(report.pid_reused.len(), 1);
        assert!(worker::proc_stat(1).is_some(), "init 必须还活着");
    }

    fn write_pf(dir: &Path, worker: &str, pid: u32, start_time: u64) {
        let pf = PidFile {
            worker: WorkerId::new(worker),
            task: TaskId::new("t"),
            pid,
            pgid: pid,
            start_time,
            round: 1,
            started_at: 0,
        };
        worker::write_pidfile(dir, &pf).unwrap();
    }
}
