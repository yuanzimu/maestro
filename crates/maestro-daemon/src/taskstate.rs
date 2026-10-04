//! 任务状态目录（workdir/.maestro/<task_id>/）的命名与 GC（R50）。
//!
//! 命名规则与 rounder 共用同一函数（分歧 = GC 找错目录）。
//! GC 语义保守（零惊喜）：
//! - 只删「本 daemon 权威库里已终态」（Done/Failed/Cancelled）任务的目录
//! - 每 workdir 按 mtime 保留最近 KEEP_PER_WORKDIR 个终态目录
//! - 非终态（Blocked/Suspended 的 resume 凭据）与未知目录一律不碰

use maestro_protocol::types::{TaskId, WorkerState};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 每 workdir 保留的终态目录数（滚动；对齐 checkpoint 滚动保留的精神，
/// 轮账体积小、调试价值随时间衰减快，取更紧的 10）
pub const KEEP_PER_WORKDIR: usize = 10;

/// 任务状态目录名：非法字符替换 _（与 rounder 历史行为一致，防御性 ——
/// 正常 id 为 t-N 形）
pub fn dir_name(task_id: &str) -> String {
    task_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 状态目录路径（rounder 写入侧同一定位逻辑）
pub fn state_dir(workdir: &Path, task_id: &str) -> PathBuf {
    workdir.join(".maestro").join(dir_name(task_id))
}

/// GC 结果
#[derive(Debug, Default)]
pub struct GcReport {
    /// 已删除的终态目录（超保留额度的旧目录）
    pub removed: Vec<PathBuf>,
}

/// 单 workdir GC：terminal_ids 为该 workdir 下已终态的任务 id。
/// 返回删除的目录。保留 mtime 最新的 keep 个；目录不存在/未知 id 不碰。
pub fn gc_workdir(workdir: &Path, terminal_ids: &[&str], keep: usize) -> Vec<PathBuf> {
    let maestro = workdir.join(".maestro");
    // 收集实际存在的终态目录（路径, mtime）
    let mut cand: Vec<(PathBuf, SystemTime)> = terminal_ids
        .iter()
        .map(|id| maestro.join(dir_name(id)))
        .filter_map(|p| {
            let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((p, mtime))
        })
        .collect();
    if cand.len() <= keep {
        return vec![];
    }
    // mtime 降序（最新在前），删除第 keep 个之后的
    cand.sort_by(|a, b| b.1.cmp(&a.1));
    cand.into_iter()
        .skip(keep)
        .map(|(p, _)| {
            // 递归删除目录本身（里面只有 maestro 自己写的轮账/session 文件；
            // 未知目录不在 terminal_ids 名单内，永远不会走到这里）
            let _ = std::fs::remove_dir_all(&p);
            p
        })
        .collect()
}

/// 从权威状态记录做 GC（recover 路径）：按 workdir 聚合终态任务 id。
/// 非终态任务不进名单（Blocked/RoundsExhausted 的 resume 凭据不得清）。
pub fn gc_from_records<'a>(
    records: impl Iterator<Item = (&'a TaskId, WorkerState, &'a str)>,
) -> GcReport {
    let mut by_workdir: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, state, workdir) in records {
        if state.is_terminal() {
            by_workdir.entry(workdir).or_default().push(id.as_str());
        }
    }
    let mut report = GcReport::default();
    for (workdir, ids) in &by_workdir {
        report
            .removed
            .extend(gc_workdir(Path::new(workdir), ids, KEEP_PER_WORKDIR));
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_dir(work: &Path, id: &str) -> PathBuf {
        let d = state_dir(work, id);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("rounds.jsonl"), "{}\n").unwrap();
        d
    }

    /// 把目录 mtime 拨回指定小时前（测试用）。
    /// CI 教训：`touch -d` 是 GNU 专属（macOS BSD touch 不支持）——
    /// 改用 nix::utimensat 直接设 mtime，跨 Unix 平台无子进程依赖
    fn age_hours(p: &Path, hours: u32) {
        use nix::sys::stat::{utimensat, UtimensatFlags};
        use nix::sys::time::TimeSpec;
        let past = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .saturating_sub(std::time::Duration::from_secs(hours as u64 * 3600));
        let ts = TimeSpec::from_duration(past);
        utimensat(None, p, &ts, &ts, UtimensatFlags::FollowSymlink)
            .expect("utimensat 设置 mtime");
    }

    #[test]
    fn dir_name_matches_rounder_sanitization() {
        assert_eq!(dir_name("t-1"), "t-1");
        assert_eq!(dir_name("t 1/x"), "t_1_x");
    }

    #[test]
    fn keeps_recent_removes_old_terminal_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path();
        let old1 = mk_dir(work, "t-old1");
        let old2 = mk_dir(work, "t-old2");
        let new = mk_dir(work, "t-new");
        age_hours(&old1, 3);
        age_hours(&old2, 2);
        let removed = gc_workdir(work, &["t-old1", "t-old2", "t-new"], 1);
        assert_eq!(removed.len(), 2, "keep=1 → 删掉两个旧目录: {removed:?}");
        assert!(removed.contains(&old1) && removed.contains(&old2));
        assert!(!old1.exists());
        assert!(!old2.exists());
        assert!(new.exists(), "最新目录保留");
    }

    #[test]
    fn within_keep_nothing_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path();
        for i in 0..3 {
            mk_dir(work, &format!("t-{i}"));
        }
        let removed = gc_workdir(work, &["t-0", "t-1", "t-2"], 10);
        assert!(removed.is_empty(), "未超保留额度不删: {removed:?}");
    }

    #[test]
    fn unknown_and_missing_dirs_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path();
        // 未知目录（非任务 id）+ 存在的终态目录 + 名单里不存在的 id
        let foreign = work.join(".maestro").join("user-stuff");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("notes.txt"), "x").unwrap();
        mk_dir(work, "t-1");
        let removed = gc_workdir(work, &["t-1", "t-404", "t-2"], 0);
        // keep=0 → t-1 删；t-404/t-2 不存在 → 无操作；foreign 不在名单 → 不碰
        assert_eq!(removed, vec![state_dir(work, "t-1")]);
        assert!(foreign.exists(), "未知目录不得被 GC 触碰");
    }

    #[test]
    fn gc_from_records_filters_non_terminal() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().display().to_string();
        // 终态 ×(KEEP+1) + 非终态（blocked：resume 凭据）—— 同 mtime 下
        // 删除哪个不确定，但「删几个」与「非终态不删」是确定的
        for i in 0..=KEEP_PER_WORKDIR {
            mk_dir(&Path::new(&work), &format!("t-done-{i}"));
        }
        mk_dir(&Path::new(&work), "t-blocked");
        let records: Vec<(TaskId, WorkerState, String)> = (0..=KEEP_PER_WORKDIR)
            .map(|i| (TaskId::new(format!("t-done-{i}")), WorkerState::Done, work.clone()))
            .chain(std::iter::once((
                TaskId::new("t-blocked"),
                WorkerState::Blocked,
                work.clone(),
            )))
            .collect();
        let report = gc_from_records(records.iter().map(|(a, b, c)| (a, *b, c.as_str())));
        assert_eq!(report.removed.len(), 1, "KEEP+1 个终态应恰好删 1: {report:?}");
        assert!(
            state_dir(&Path::new(&work), "t-blocked").exists(),
            "非终态目录（resume 凭据）不得删"
        );
    }

    #[test]
    fn empty_workdir_noop() {
        let report = gc_from_records(
            [(TaskId::new("t-1"), WorkerState::Done, "/nonexistent/dir".to_string())]
                .iter()
                .map(|(a, b, c)| (a, *b, c.as_str())),
        );
        assert!(report.removed.is_empty());
    }
}
