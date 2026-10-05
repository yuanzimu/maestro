//! Checkpoint 时光机：git ref 原语（设计 §4）。
//!
//! 决策（调研坑 2 采纳）：不用 libgit2，直接调 git plumbing；
//! **串行执行**（worktree 并发写共享 .git 会随机锁冲突）。

use maestro_protocol::types::*;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

/// git 锁冲突重试次数（指数退避：50/100/200/400/800ms，累计约 1.55s）。
/// 背景：急停已 SIGSTOP 本 worktree 的 worker，但共享同一 .git 的**外部**
/// git 进程（用户手动操作 / 并行 worktree 的 gc / 杀毒扫描持锁）仍可能短
/// 暂持有 index.lock / ref 锁，导致急停快照偶发失败。锁等待是瞬时的，
/// 短退避重试即可消除绝大多数偶发失败；真正的硬错误仍立即上抛。
const LOCK_RETRIES: u32 = 5;

/// 判断 stderr 是否为 git 锁冲突（index.lock / ref lock / 无法立即锁定）。
fn is_lock_conflict(stderr: &str) -> bool {
    stderr.contains("Unable to create") && stderr.contains(".lock")
        || stderr.contains("File exists") && stderr.contains(".lock")
        || stderr.contains("could not lock")
        || stderr.contains("cannot lock ref")
        || stderr.contains("unable to lock")
        || stderr.contains("is locked but not by us")
}

/// checkpoint 引用命名空间：refs/maestro/cp/<task>/<seq>-<label>
pub fn cp_ref(task: &TaskId, seq: u32, reason: CpReason) -> CheckpointRef {
    CheckpointRef(format!(
        "refs/maestro/cp/{}/{}-{}",
        task,
        seq,
        reason_label(reason)
    ))
}

fn reason_label(r: CpReason) -> &'static str {
    match r {
        CpReason::Baseline => "baseline",
        CpReason::RoundStart => "round_start",
        CpReason::Emergency => "emergency",
        CpReason::Manual => "manual",
        CpReason::PreRollback => "pre_rollback",
        CpReason::AcceptancePassed => "acceptance_passed",
        CpReason::PreMerge => "pre_merge",
    }
}

fn label_to_reason(label: &str) -> Option<CpReason> {
    Some(match label {
        "baseline" => CpReason::Baseline,
        "round_start" => CpReason::RoundStart,
        "emergency" => CpReason::Emergency,
        "manual" => CpReason::Manual,
        "pre_rollback" => CpReason::PreRollback,
        "acceptance_passed" => CpReason::AcceptancePassed,
        "pre_merge" => CpReason::PreMerge,
        _ => return None,
    })
}

fn git(worktree: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(["-C"])
        .arg(worktree)
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// 带锁冲突退避重试的 git 调用。仅在识别为锁冲突时重试；
/// spawn 失败与其他硬错误立即返回。用于 capture/restore 等写路径。
fn git_retry(worktree: &Path, args: &[&str]) -> Result<String, String> {
    let mut attempt = 0;
    loop {
        match git(worktree, args) {
            Ok(out) => return Ok(out),
            Err(e) => {
                if attempt < LOCK_RETRIES && is_lock_conflict(&e) {
                    // 指数退避：50ms 起，每轮翻倍
                    thread::sleep(Duration::from_millis(50 * 2u64.pow(attempt)));
                    attempt += 1;
                } else {
                    return Err(e);
                }
            }
        }
    }
}

/// 单个 checkpoint 的展开信息
#[derive(Debug, Clone)]
pub struct CheckpointInfo {
    pub task: TaskId,
    pub seq: u32,
    pub reason: Option<CpReason>,
    /// refs/maestro/cp/<task>/<seq>-<label> 的完整引用
    pub full_ref: String,
    /// commit hash
    pub commit: String,
    /// message JSON（含 round/ts/parent）
    pub meta: Option<CheckpointMeta>,
}

/// capture 原语（设计 §4.3）：
/// `git add -A → write-tree → commit-tree -p <parent> → update-ref → reset`
/// 不动 HEAD、不污染分支。返回创建的 ref。
pub fn capture(
    worktree: &Path,
    task: &TaskId,
    round: u32,
    reason: CpReason,
    clock: &dyn maestro_protocol::Clock,
) -> Result<CheckpointRef, String> {
    // 1. 暂存全部（含 untracked；.gitignore 的构建产物天然排除）
    git_retry(worktree, &["add", "-A"])?;
    // add 之后的主体包进闭包：任一步失败都在传播错误前尽力 reset，
    // 否则改动会以 staged 状态留在 index，违背「工作区零扰动」契约
    // （外部 git 并发持锁/磁盘满时 write-tree、update-ref 等可能失败）。
    let result = (|| -> Result<CheckpointRef, String> {
        // 2. 树对象
        let tree = git_retry(worktree, &["write-tree"])?;
        // 3. 父 commit：最近 checkpoint（无则用 HEAD，允许空仓 --allow-empty）
        let seq = next_seq(worktree, task)?;
        let parent = latest_commit(worktree, task).or_else(|| head_commit(worktree));
        // 4. commit-tree（不动 HEAD）
        let msg = serde_json::json!({
            "task": task.as_str(),
            "round": round,
            "reason": serde_json::to_value(reason).unwrap_or_default(),
            "parent_cp": serde_json::Value::Null,
            "ts": clock.now_ms(),
        })
        .to_string();
        let mut args = vec!["commit-tree", tree.as_str(), "-m", msg.as_str()];
        if let Some(p) = &parent {
            args.push("-p");
            args.push(p.as_str());
        }
        let commit = git_retry(worktree, &args)?;
        // 5. update-ref
        let r = cp_ref(task, seq, reason);
        git_retry(worktree, &["update-ref", r.as_str(), commit.as_str()])?;
        Ok(r)
    })();
    // 无论成败都还原 index（reset 本身失败不覆盖原始错误）
    let reset_err = git_retry(worktree, &["reset"]).err();
    match result {
        Ok(r) if reset_err.is_none() => Ok(r),
        Ok(_r) => Err(reset_err.unwrap_or_default()),
        Err(e) => Err(e),
    }
}

/// restore 原语（设计 §4.4）：
/// pre-rollback 安全垫 → `reset --hard <cp>` → `clean -fd`
pub fn restore(
    worktree: &Path,
    task: &TaskId,
    to: &CheckpointRef,
    clock: &dyn maestro_protocol::Clock,
) -> Result<CheckpointRef, String> {
    // I4：回滚本身可撤销 —— 先快照当前状态
    let pre = capture(worktree, task, 0, CpReason::PreRollback, clock)?;
    let _ = git_retry(worktree, &["reset", "--hard", to.as_str()])?;
    git_retry(worktree, &["clean", "-fd"])?;
    Ok(pre)
}

/// 列出任务全部 checkpoint（时间序）
pub fn list(worktree: &Path, task: &TaskId) -> Vec<CheckpointInfo> {
    let prefix = format!("refs/maestro/cp/{}/", task);
    let Ok(out) = git(
        worktree,
        &["for-each-ref", "--format=%(refname) %(objectname)", &prefix],
    ) else {
        return vec![];
    };
    let mut infos: Vec<CheckpointInfo> = out
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let full_ref = it.next()?.to_string();
            let commit = it.next()?.to_string();
            let rest = full_ref.strip_prefix(&prefix)?;
            let (seq_str, label) = rest.split_once('-')?;
            let seq = seq_str.parse().ok()?;
            Some(CheckpointInfo {
                task: task.clone(),
                seq,
                reason: label_to_reason(label),
                full_ref: full_ref.clone(),
                commit: commit.clone(),
                meta: None,
            })
        })
        .collect();
    infos.sort_by_key(|i| i.seq);
    // 补充 meta（message JSON）
    for i in &mut infos {
        if let Ok(msg) = git(worktree, &["log", "-1", "--format=%B", &i.full_ref]) {
            if let Ok(meta) = serde_json::from_str::<CheckpointMeta>(&msg) {
                i.meta = Some(meta);
            }
        }
    }
    infos
}

/// GC（设计 §4.2 保留策略）：pinned 永久；round_start/emergency 滚动保留最近 N 个。
pub fn gc(worktree: &Path, task: &TaskId, keep_recent: usize) -> Result<usize, String> {
    let infos = list(worktree, task);
    let rolling: Vec<&CheckpointInfo> = infos
        .iter()
        .filter(|i| !i.reason.map(|r| r.is_pinned()).unwrap_or(false))
        .collect();
    if rolling.len() <= keep_recent {
        return Ok(0);
    }
    let to_delete = &rolling[..rolling.len() - keep_recent];
    for info in to_delete {
        let _ = git(worktree, &["update-ref", "-d", &info.full_ref]);
    }
    Ok(to_delete.len())
}

// ---------------------------------------------------------------------------
// 内部
// ---------------------------------------------------------------------------

fn next_seq(worktree: &Path, task: &TaskId) -> Result<u32, String> {
    // 基于**现存最大序号** +1，而非数量 +1：gc 会物理删除早期 ref 而不
    // 压缩序号，len+1 会在 gc 之后产生与现存 ref 碰撞的序号，
    // update-ref 静默覆盖旧 checkpoint（历史丢失、谱系错乱）。
    Ok(list(worktree, task).iter().map(|i| i.seq).max().unwrap_or(0) + 1)
}

fn latest_commit(worktree: &Path, task: &TaskId) -> Option<String> {
    let infos = list(worktree, task);
    infos.last().map(|i| i.commit.clone())
}

fn head_commit(worktree: &Path) -> Option<String> {
    git(worktree, &["rev-parse", "HEAD"]).ok()
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_protocol::SystemClock;

    fn init_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        git(p, &["init", "-q"]).unwrap();
        git(p, &["config", "user.email", "t@t"]).unwrap();
        git(p, &["config", "user.name", "t"]).unwrap();
        git(p, &["commit", "--allow-empty", "-q", "-m", "init"]).unwrap();
        tmp
    }

    fn write(p: &Path, name: &str, content: &str) {
        std::fs::write(p.join(name), content).unwrap();
    }

    #[test]
    fn capture_does_not_move_head() {
        let tmp = init_repo();
        let p = tmp.path();
        let head_before = head_commit(p).unwrap();
        let t = TaskId::new("t1");
        capture(p, &t, 1, CpReason::RoundStart, &SystemClock).unwrap();
        let head_after = head_commit(p).unwrap();
        assert_eq!(head_before, head_after, "capture 不得动 HEAD");
        assert_eq!(list(p, &t).len(), 1);
    }

    #[test]
    fn capture_includes_untracked() {
        let tmp = init_repo();
        let p = tmp.path();
        let t = TaskId::new("t1");
        write(p, "new-file.txt", "hello");
        let cp = capture(p, &t, 1, CpReason::RoundStart, &SystemClock).unwrap();
        // 恢复到该 cp 应带回 new-file.txt
        write(p, "new-file.txt", "changed");
        std::fs::remove_file(p.join("extra.txt")).ok();
        restore(p, &t, &cp, &SystemClock).unwrap();
        let content = std::fs::read_to_string(p.join("new-file.txt")).unwrap();
        assert_eq!(content, "hello", "untracked 文件应进快照");
    }

    /// I4 主径：restore 前自动 pre-rollback 安全垫，回滚可撤销
    #[test]
    fn restore_creates_prerollback_and_is_undoable() {
        let tmp = init_repo();
        let p = tmp.path();
        let t = TaskId::new("t1");
        write(p, "a.txt", "v1");
        let cp1 = capture(p, &t, 1, CpReason::RoundStart, &SystemClock).unwrap();
        write(p, "a.txt", "v2");
        let cp2 = capture(p, &t, 2, CpReason::RoundStart, &SystemClock).unwrap();

        // 回滚到 cp1：应产生 pre_rollback 安全垫
        let pre = restore(p, &t, &cp1, &SystemClock).unwrap();
        assert!(pre.as_str().contains("pre_rollback"), "安全垫 ref: {}", pre);
        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "v1");

        // 撤销回滚：restore 到安全垫 → 回到 v2
        restore(p, &t, &pre, &SystemClock).unwrap();
        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "v2");
        let _ = cp2;
    }

    /// GC：pinned 永久保留，滚动清理
    #[test]
    fn gc_keeps_pinned_and_recent() {
        let tmp = init_repo();
        let p = tmp.path();
        let t = TaskId::new("t1");
        capture(p, &t, 0, CpReason::Baseline, &SystemClock).unwrap();
        for r in 1..=8 {
            write(p, &format!("f{r}.txt"), "x");
            capture(p, &t, r, CpReason::RoundStart, &SystemClock).unwrap();
        }
        let deleted = gc(p, &t, 3).unwrap();
        assert!(deleted >= 4, "应清理早期 rolling cp: {deleted}");
        let remaining = list(p, &t);
        // baseline 必在
        assert!(remaining
            .iter()
            .any(|i| i.reason == Some(CpReason::Baseline)));
        // rolling 只剩最近 3 个
        let rolling: Vec<_> = remaining
            .iter()
            .filter(|i| i.reason == Some(CpReason::RoundStart))
            .collect();
        assert_eq!(rolling.len(), 3);
    }

    /// 工作区零扰动：capture 前后 status 完全一致（无 staged 残留）
    #[test]
    fn capture_leaves_clean_index() {
        let tmp = init_repo();
        let p = tmp.path();
        let t = TaskId::new("t1");
        write(p, "dirty.txt", "d");
        let before = git(p, &["status", "--porcelain"]).unwrap();
        let _ = capture(p, &t, 1, CpReason::RoundStart, &SystemClock).unwrap();
        let after = git(p, &["status", "--porcelain"]).unwrap();
        assert_eq!(
            before, after,
            "capture 不得扰动 status（前: {before:?} 后: {after:?}）"
        );
        // 未跟踪文件仍是未跟踪（不被意外 staged）
        assert!(
            after.contains("?? dirty.txt"),
            "untracked 应保持 untracked: {after}"
        );
    }

    /// 锁冲突分类器：锁类错误识别为可重试，其他错误不重试。
    #[test]
    fn lock_conflict_classifier() {
        assert!(is_lock_conflict(
            "fatal: Unable to create '.git/index.lock': File exists."
        ));
        assert!(is_lock_conflict(
            "error: cannot lock ref 'refs/maestro/cp/t1/1-emergency'"
        ));
        assert!(is_lock_conflict("fatal: could not lock config file"));
        assert!(!is_lock_conflict("fatal: not a git repository"));
        assert!(!is_lock_conflict("error: pathspec did not match"));
    }

    /// 急停快照遇瞬时持锁：锁在退避窗口内释放后应成功（偶发失败主场景）。
    /// 先同步建锁保证首次必撞锁，后台线程 150ms 后释放；重试预算远大于此。
    #[test]
    fn capture_survives_transient_lock() {
        let tmp = init_repo();
        let p = tmp.path();
        write(p, "dirty.txt", "d");
        let lock = p.join(".git").join("index.lock");
        std::fs::write(&lock, b"").unwrap();
        let lock_for_thread = lock.clone();
        let h = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            let _ = std::fs::remove_file(&lock_for_thread);
        });

        let t = TaskId::new("t1");
        let cp = capture(p, &t, 1, CpReason::Emergency, &SystemClock);
        h.join().unwrap();
        assert!(cp.is_ok(), "瞬时锁释放后应急快照应成功: {cp:?}");
        assert!(
            git(p, &["status", "--porcelain"]).unwrap().contains("?? dirty.txt"),
            "成功后工作区应保持原状"
        );
        assert!(!lock.exists(), "成功后不应残留 index.lock");
    }
}
