//! 脚本 Worker：以独立进程组启动测试脚本（loop_write/loop_http/hanging），
//! 供 I1 冻结延迟实测与孤儿清理测试用。

use nix::sys::signal::{kill as nix_kill, Signal};
use nix::unistd::Pid;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

fn kill_pid(raw: i32, sig: Signal) -> nix::Result<()> {
    nix_kill(Pid::from_raw(raw), sig)
}

/// 已启动的脚本 Worker 句柄
pub struct ScriptWorker {
    pub child: Child,
    pub pid: u32,
    pub pgid: u32,
    script_path: PathBuf,
    cleanup: Option<Arc<SharedCleanup>>,
}

/// 测试结束兜底清理：进程组 SIGKILL（RAII，防泄漏）
pub struct SharedCleanup {
    pgids: Mutex<Vec<u32>>,
}

impl SharedCleanup {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self {
            pgids: Mutex::new(vec![]),
        })
    }

    pub fn register(&self, pgid: u32) {
        self.pgids.lock().unwrap().push(pgid);
    }

    pub fn cleanup_all(&self) {
        for pgid in self.pgids.lock().unwrap().drain(..) {
            let _ = kill_pid(-(pgid as i32), Signal::SIGKILL);
        }
    }
}

impl Drop for ScriptWorker {
    fn drop(&mut self) {
        // 先注销，避免重复 kill
        if let Some(c) = self.cleanup.take() {
            c.pgids.lock().unwrap().retain(|&p| p != self.pgid);
        }
        let _ = kill_pid(-(self.pgid as i32), Signal::SIGKILL);
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.script_path);
    }
}

impl ScriptWorker {
    /// 启动脚本（独立进程组）。脚本必须是可执行文件。
    pub fn start(script: &str, cleanup: Option<Arc<SharedCleanup>>) -> std::io::Result<Self> {
        let script_path = write_executable(script)?;
        let mut cmd = Command::new(&script_path);
        // 独立进程组：setpgid(0,0) —— 子进程自成一组
        cmd.process_group(0);
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        let child = cmd.spawn()?;
        let pid = child.id();
        let pgid = pid; // process_group(0) ⇒ pgid == pid
        if let Some(c) = &cleanup {
            c.register(pgid);
        }
        Ok(Self {
            child,
            pid,
            pgid,
            script_path,
            cleanup,
        })
    }

    /// 进程是否仍在运行（经 /proc 探测，无需 &mut self）
    pub fn is_alive(&self) -> bool {
        proc_state(self.pid).is_some()
    }

    /// 进程是否已被冻结（SIGSTOP 验证，用例 B1）。
    /// ⚠️ SIGSTOP 已发出后：T = 已停；D = stop pending（dash 对单条外部命令
    /// 用 vfork 优化，父进程卡在不可中断的 vfork-wait 直到子进程 exec ——
    /// 若 STOP 恰落在 vfork 窗口，父进程停在 D 直到子进程恢复，SIGSTOP
    /// 已投递且回用户态即生效。两种状态都代表组不再执行用户代码 = 冻结。
    /// macOS BSD ps 的 stat 带后缀标记（如 "T+"）→ 首字母匹配；
    /// BSD 的不可中断态是 U（Linux 是 D）。
    pub fn is_stopped(&self) -> bool {
        matches!(
            proc_state(self.pid).as_deref(),
            Some(s)
                if s.starts_with('T') || s.starts_with('D') || s.starts_with('U')
        )
    }

    /// 给整个进程组发信号
    pub fn signal_group(&self, sig: nix::sys::signal::Signal) -> nix::Result<()> {
        kill_pid(-(self.pgid as i32), sig)
    }
}

/// 读进程 state 字段（R/S/T/D/Z...）。
/// Linux：/proc/<pid>/stat 解析；macOS：无 /proc —— 经 `ps -o stat=` 查询
///（字母语义兼容：T=stopped、Z=zombie）。
pub fn proc_state(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // 形如 "1234 (bash) S 1 ..."，comm 里可能含空格/括号——取最后一个 ')' 之后
        let after = stat.rsplit(')').next()?;
        after.split_whitespace().next().map(|s| s.to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !out.status.success() {
            return None; // 进程不存在
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

/// 把脚本内容写入 tempdir 下的可执行文件（P0_SETUP 决策 2：不落仓库路径）
pub fn write_executable(script: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("maestro-testkit-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("script-{}.sh", next_counter()));
    let mut f = std::fs::File::create(&path)?;
    f.write_all(script.as_bytes())?;
    f.sync_all()?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

fn next_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// fixture 脚本库（P0_SETUP §四）
// ---------------------------------------------------------------------------

/// 每 interval_ms 追加一行到 file（I1 冻结延迟实测 / B1）
pub fn loop_write_script(file: &str, interval_ms: u64) -> String {
    format!(
        r#"#!/bin/sh
F="$MAESTRO_TEST_OUT"
while true; do
  echo "$$ $(date +%s%N)" >> "$F"
  sleep {interval_ms_ms}000
done
"#,
        interval_ms_ms = interval_ms
    )
    .replace("$MAESTRO_TEST_OUT", &format!("{file:?}"))
}

/// 挂死脚本（SIGSTOP/孤儿清理测试）
pub fn hanging_script() -> &'static str {
    "#!/bin/sh\nsleep 100000\n"
}

/// 立即退出并带指定退出码
pub fn exit_script(code: i32) -> String {
    format!("#!/bin/sh\nexit {code}\n")
}

/// 写一个文件然后退出 0（简单成功路径）
pub fn touch_and_exit(file: &str) -> String {
    format!("#!/bin/sh\necho done > {file:?}\nexit 0\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn starts_in_own_pgroup_and_dies_on_drop() {
        let before = our_pgid();
        let w = ScriptWorker::start(hanging_script(), None).expect("spawn");
        let after = our_pgid();
        // 我们自己的 pgid 没变（隔离成立）
        assert_eq!(before, after, "测试进程 pgid 不应被改变");
        // 子进程活着且独立成组
        assert!(w.is_alive());
        assert_ne!(w.pgid as i32, before, "脚本应在独立进程组");
        drop(w); // RAII 清理
    }

    #[test]
    #[serial]
    fn sigstop_freezes_the_process() {
        let w = ScriptWorker::start(hanging_script(), None).expect("spawn");
        assert!(w.is_alive());
        w.signal_group(nix::sys::signal::Signal::SIGSTOP)
            .expect("SIGSTOP");
        // 轮询等 T 态：高负载下 SIGSTOP 可能落在瞬态 D（不可中断 IO）上，
        // 需等 IO 完成才转 T（与 b1_b5 同款抖动）
        let mut stopped = false;
        for _ in 0..100 {
            if w.is_stopped() {
                stopped = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(stopped, "进程组应在 2s 内进入 T 状态");
        w.signal_group(nix::sys::signal::Signal::SIGCONT)
            .expect("SIGCONT");
        let mut resumed = false;
        for _ in 0..100 {
            if !w.is_stopped() {
                resumed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(resumed, "SIGCONT 后应恢复");
        assert!(w.is_alive());
    }

    #[test]
    fn proc_state_parses() {
        let me = std::process::id();
        let st = proc_state(me).expect("读自己");
        // 首字母匹配：macOS BSD ps 的 stat 带后缀（"S+"/"U<" 等）。
        // CI 实测：macOS runner 上测试进程可瞬时处于 U（不可中断等待）——
        // 断言"活着可读"而非具体状态
        assert!(
            st.starts_with('R') || st.starts_with('S') || st.starts_with('U'),
            "状态应为 R/S/U，实际 {st}"
        );
    }

    /// 读自己的 pgid（Linux 经 /proc/self/stat；跨平台直接 getpgrp —— macOS 无 /proc）
    fn our_pgid() -> i32 {
        nix::unistd::getpgrp().as_raw()
    }
}
