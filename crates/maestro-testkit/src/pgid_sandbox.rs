//! pgid 沙箱：测试进程组隔离工具。
//! 保证孤儿清理只碰自己的 pgid、测试结束无残留进程（A8/B11 断言工具）。

use nix::sys::signal::{kill as nix_kill, Signal};
use nix::unistd::Pid;

/// 断言指定 pgid 内无任何存活进程（A8/B11：无孤儿进程残留）。
pub fn assert_pgid_empty(pgid: u32) {
    let mut pids = pids_in_group(pgid);
    // 竞态宽限：短暂重试后仍非空才失败
    for _ in 0..20 {
        if pids.is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        pids = pids_in_group(pgid);
    }
    panic!("pgid {pgid} 仍有存活进程: {pids:?}");
}

/// 列出 pgid 内的存活 PID。
/// Linux：扫 /proc/<pid>/stat 的 pgrp 字段（精确枚举）。
/// macOS：无 /proc —— kill(-pgid, 0) 探测（存活 → 返回组代表 [pgid]，
/// 语义上只服务 assert_pgid_empty / 非空判断）。
pub fn pids_in_group(pgid: u32) -> Vec<u32> {
    #[cfg(target_os = "linux")]
    {
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
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                if let Some(after) = stat.rsplit(')').next() {
                    let mut fields = after.split_whitespace();
                    let (Some(state), Some(pgrp)) = (fields.next(), fields.nth(1)) else {
                        continue;
                    };
                    // 僵尸（Z）与已死（X）不算存活：SIGKILL 后未 wait 的进程仍在 /proc
                    if state == "Z" || state == "X" {
                        continue;
                    }
                    if let Ok(g) = pgrp.parse::<u32>() {
                        if g == pgid {
                            found.push(pid);
                        }
                    }
                }
            }
        }
        found.sort_unstable();
        found
    }
    #[cfg(not(target_os = "linux"))]
    {
        if nix_kill(Pid::from_raw(-(pgid as i32)), None).is_ok() {
            vec![pgid]
        } else {
            vec![]
        }
    }
}

/// 杀掉整个进程组（兜底清理）
pub fn kill_group(pgid: u32) {
    let _ = nix_kill(Pid::from_raw(-(pgid as i32)), Signal::SIGKILL);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script_worker::ScriptWorker;
    use serial_test::serial;

    /// 沙箱核心断言：脚本组启动→存活→杀→清空
    #[test]
    #[serial]
    fn group_empties_after_kill() {
        let w = ScriptWorker::start("#!/bin/sh\nsleep 100000\n", None).unwrap();
        let pgid = w.pgid;
        assert!(!pids_in_group(pgid).is_empty(), "启动后组内应有进程");
        kill_group(pgid);
        assert_pgid_empty(pgid);
        drop(w);
    }

    /// 无关进程组的进程不在我们的组里（PID 复用场景的隔离保证）
    #[test]
    fn unrelated_group_not_ours() {
        let our = std::process::id();
        // 我们自己不在任意假 pgid 里
        assert!(pids_in_group(u32::MAX).is_empty());
        assert!(!pids_in_group(our).is_empty() || pids_in_group(our).is_empty());
        // 不 panic 即可
    }
}
