//! 验收门 v0：命令退出码 + 读回校验（DEV_PLAN 0.10 / U10 设计）。
//!
//! 判定「假完成」的启发式：worker exit 0 前后 worktree 无任何实际变化
//! → 视为没产出 → AcceptanceGateFailed。三次失败 → blocked(AcceptanceFailed)。
//!
//! 局限（v0 明知接受）：
//! - SipHash 内容比对：改完又还原 → 检测不到（真·无进展）
//! - 构建产物目录（target/ 等）不算产物 —— 防「只 cargo build 不改代码」的假完成
//! - 产物写在 workdir 之外（如 /tmp）→ 检测不到（误判为假完成）
//! - workdir 与 daemon data_dir 重叠 → 读回不可观测，门自动通过（老语义）
//! - 无产物型任务（调研/问答）会误判 —— 多轮驱动 Worker 模式（0.15）接管后，
//!   验收标准升级为「结构化验收断言」

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// worktree 快照：文件集合（相对路径 + 内容哈希）。
/// 哈希而非字节数：防「同长度不同内容」的假完成碰撞（R18 负载测试抓到）。
pub type Snapshot = HashSet<(PathBuf, u64)>;

/// 读回校验的观测能力
pub enum Readback {
    /// 可比对快照
    Verifiable(Snapshot),
    /// workdir 与 daemon 数据目录重叠：内部噪音（logs/pidfile/事件库）无法
    /// 与任务产物区分，读回校验无意义 → 门退化为仅看退出码
    Unverifiable,
}

/// 不视为任务产物的目录（版本库内部 / 构建输出 / 虚拟环境）
const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".maestro",
    "target",
    "node_modules",
    "dist",
    "build",
    "__pycache__",
    ".venv",
];

/// 快照条目上限（防超大目录拖慢 Core 线程）
const MAX_ENTRIES: usize = 50_000;
/// 单文件哈希读取上限（更大的文件退化为「路径+长度」混合哈希）
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// 文件指纹：内容哈希（SipHash）。超大文件只哈希首 8MB + 长度。
fn file_fingerprint(path: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    let len = meta.len();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if len <= MAX_FILE_BYTES {
        if let Ok(bytes) = std::fs::read(path) {
            bytes.hash(&mut h);
            return h.finish();
        }
    } else if let Ok(mut f) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buf = vec![0u8; MAX_FILE_BYTES as usize];
        if f.read(&mut buf).is_ok() {
            buf.hash(&mut h);
            len.hash(&mut h); // 尾部未读，掺长度
            return h.finish();
        }
    }
    // 读不了内容：退化为长度
    len.hash(&mut h);
    h.finish()
}

/// 对 workdir 做文件级快照（符号链接跳过，防循环）。
/// `exclude`：daemon 数据目录等不得计入产物的路径（及其子树）。
pub fn snapshot(workdir: &Path, exclude: &[PathBuf]) -> Readback {
    let Ok(root) = workdir.canonicalize() else {
        return Readback::Unverifiable; // workdir 不存在：spawn 阶段就会失败
    };
    // 整棵树都在排除范围 → 不可观测
    let excluded: Vec<PathBuf> = exclude
        .iter()
        .filter_map(|p| p.canonicalize().ok())
        .collect();
    if excluded.iter().any(|e| root == *e || root.starts_with(e)) {
        return Readback::Unverifiable;
    }
    let mut snap = Snapshot::new();
    let mut budget = MAX_ENTRIES;
    walk(&root, PathBuf::new(), &mut snap, &mut budget, &excluded);
    Readback::Verifiable(snap)
}

fn walk(root: &Path, rel: PathBuf, out: &mut Snapshot, budget: &mut usize, excluded: &[PathBuf]) {
    if *budget == 0 {
        return;
    }
    let dir = root.join(&rel);
    if excluded.contains(&dir) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for e in entries.flatten() {
        if *budget == 0 {
            return;
        }
        let Ok(ft) = e.file_type() else {
            continue;
        };
        let name = e.file_name();
        let child_rel = rel.join(&name);
        if ft.is_dir() {
            if IGNORED_DIRS.contains(&name.to_string_lossy().as_ref()) {
                continue;
            }
            *budget -= 1;
            walk(root, child_rel, out, budget, excluded);
        } else if ft.is_file() {
            out.insert((child_rel, file_fingerprint(&e.path())));
        }
        // symlink：不跟进（防循环 + 产物应以真实文件为准）
    }
}

/// 读回校验：前后快照不一致 = 有实际产出
pub fn changed(before: &Snapshot, after: &Snapshot) -> bool {
    before != after
}

/// 变化摘要（事件 output 字段用）
pub fn diff_summary(before: &Snapshot, after: &Snapshot) -> String {
    let has = |p: &PathBuf, s: &u64, snap: &Snapshot| snap.contains(&(p.clone(), *s));
    let added = after.iter().filter(|(p, s)| !has(p, s, before)).count();
    let removed = before.iter().filter(|(p, s)| !has(p, s, after)).count();
    let same_path = |p: &PathBuf, snap: &Snapshot| snap.iter().any(|(bp, _)| bp == p);
    let modified = after
        .iter()
        .filter(|(p, s)| same_path(p, before) && !has(p, s, before))
        .count();
    format!("added={added} removed={removed} modified={modified}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(p: &Path) -> Snapshot {
        match snapshot(p, &[]) {
            Readback::Verifiable(s) => s,
            Readback::Unverifiable => panic!("should be verifiable"),
        }
    }

    #[test]
    fn detects_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let before = snap(tmp.path());
        assert!(before.is_empty());
        std::fs::write(tmp.path().join("out.txt"), "hi").unwrap();
        let after = snap(tmp.path());
        assert!(changed(&before, &after), "新文件应判定为有产物");
    }

    #[test]
    fn detects_size_change() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "aaa").unwrap();
        let before = snap(tmp.path());
        std::fs::write(tmp.path().join("a.txt"), "aaaaaa").unwrap();
        let after = snap(tmp.path());
        assert!(changed(&before, &after), "同路径大小变化应判定为有产物");
    }

    /// R18 负载测试抓到的碰撞：同字节数不同内容（t-10 vs t-12 覆写同文件）
    #[test]
    fn same_length_different_content_detected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("out.txt"), "t-10\n").unwrap();
        let before = snap(tmp.path());
        std::fs::write(tmp.path().join("out.txt"), "t-12\n").unwrap();
        let after = snap(tmp.path());
        assert!(
            changed(&before, &after),
            "同长度不同内容必须判定为有产物（内容哈希）"
        );
    }

    #[test]
    fn no_change_no_artifact() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "aaa").unwrap();
        let before = snap(tmp.path());
        std::thread::sleep(std::time::Duration::from_millis(5));
        let after = snap(tmp.path());
        assert!(!changed(&before, &after), "无变化 = 假完成");
    }

    #[test]
    fn build_outputs_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let before = snap(tmp.path());
        std::fs::create_dir_all(tmp.path().join("target/debug")).unwrap();
        std::fs::write(tmp.path().join("target/debug/libfoo.a"), "bin").unwrap();
        std::fs::create_dir_all(tmp.path().join(".git/objects")).unwrap();
        std::fs::write(tmp.path().join(".git/HEAD"), "ref").unwrap();
        let after = snap(tmp.path());
        assert!(
            !changed(&before, &after),
            "target/.git 内的变化不算任务产物"
        );
    }

    #[test]
    fn missing_workdir_is_unverifiable() {
        assert!(matches!(
            snapshot(Path::new("/nonexistent/maestro-test"), &[]),
            Readback::Unverifiable
        ));
    }

    /// workdir 与排除目录重叠 → 不可观测（门退化为仅退出码）
    #[test]
    fn overlapping_workdir_unverifiable() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        assert!(matches!(
            snapshot(&data, &[data.clone()]),
            Readback::Unverifiable
        ));
        // workdir 在 data 内部同样不可观测
        let nested = data.join("task1");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(matches!(
            snapshot(&nested, &[data.clone()]),
            Readback::Unverifiable
        ));
    }

    /// data_dir 嵌在 workdir 内部：排除子树，其余照常比对
    #[test]
    fn nested_data_dir_excluded_but_rest_compared() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let data = work.join(".maestro-data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("events.db"), "noise").unwrap();
        let before = snap_with(&work, &[data.clone()]);
        std::fs::write(data.join("logs.txt"), "more noise").unwrap();
        let after_noise = snap_with(&work, &[data.clone()]);
        assert!(!changed(&before, &after_noise), "data_dir 内部噪音不算产物");
        std::fs::write(work.join("src.rs"), "fn main() {}").unwrap();
        let after_real = snap_with(&work, &[data.clone()]);
        assert!(changed(&before, &after_real), "真实源码变化应判有产物");
    }

    fn snap_with(p: &Path, exclude: &[PathBuf]) -> Snapshot {
        match snapshot(p, exclude) {
            Readback::Verifiable(s) => s,
            Readback::Unverifiable => panic!("should be verifiable"),
        }
    }
}
