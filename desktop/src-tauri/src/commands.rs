//! tauri commands：RPC 薄透传（daemon 的响应 JSON 原样给前端，
//! TS 侧 types.ts 与 wire 形状对齐）+ 引擎/设置的本机操作。
//!
//! 约定：错误统一 `Err(String)`，格式 "RPC -409: xxx" / "连接失败: xxx"。

use crate::daemon;
use crate::settings::{self, Settings};
use crate::state::AppState;
use crate::tools;
use maestro_client::MaestroClient;
use maestro_protocol::api::Method;
use serde_json::{json, Value};
use std::io::Write;
use std::process::Command;
use tauri::{AppHandle, Manager, State};

fn client(state: &AppState) -> MaestroClient {
    MaestroClient::new(&state.data_dir)
}

fn rpc_err(e: maestro_client::ClientError) -> String {
    match e {
        maestro_client::ClientError::Rpc { code, message } => format!("RPC {code}: {message}"),
        other => other.to_string(),
    }
}

fn call(state: &AppState, method: Method, params: Value) -> Result<Value, String> {
    client(state)
        .call("desktop", method, params)
        .map_err(rpc_err)
}

// ---- 引擎 ----

/// 前端监听器已注册（store 的 listen() 全部就位）→ 放行事件桥首订阅
#[tauri::command]
pub async fn bridge_ready(state: State<'_, AppState>) -> Result<(), String> {
    state
        .bridge_start
        .store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub async fn daemon_status(state: State<'_, AppState>) -> Result<Value, String> {
    let c = client(&state);
    if !c.is_daemon_alive() {
        return Ok(json!({
            "running": false,
            "managed": state.is_managed(),
            "version": "",
            "pid": 0,
            "uptime_secs": 0,
            "event_seq": 0,
        }));
    }
    let status = c
        .call("status", Method::ServerStatus, json!({}))
        .map_err(rpc_err)?;
    Ok(json!({
        "running": true,
        "managed": state.is_managed(),
        "version": status["version"],
        "pid": status["pid"],
        "uptime_secs": status["uptime_secs"],
        "event_seq": status["event_seq"],
    }))
}

#[tauri::command]
pub async fn restart_daemon(app: AppHandle) -> Result<(), String> {
    daemon::restart(&app)
}

#[tauri::command]
pub async fn shutdown_daemon(app: AppHandle) -> Result<(), String> {
    daemon::shutdown(&app, 5_000)
}

// ---- 任务 ----

#[tauri::command]
pub async fn create_task(
    state: State<'_, AppState>,
    title: String,
    prompt: String,
    workdir: Option<String>,
) -> Result<Value, String> {
    let params = json!({ "title": title, "prompt": prompt, "workdir": workdir });
    call(&state, Method::TaskCreate, params)
}

#[tauri::command]
pub async fn list_tasks(state: State<'_, AppState>) -> Result<Value, String> {
    call(&state, Method::TaskList, json!({}))
}

#[tauri::command]
pub async fn get_task(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    call(&state, Method::TaskGet, json!({ "task": id }))
}

#[tauri::command]
pub async fn pause_task(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    call(&state, Method::TaskPause, json!({ "task": id }))
}

#[tauri::command]
pub async fn resume_task(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    call(&state, Method::TaskResume, json!({ "task": id }))
}

#[tauri::command]
pub async fn cancel_task(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    call(&state, Method::TaskCancel, json!({ "task": id }))
}

#[tauri::command]
pub async fn steer_task(
    state: State<'_, AppState>,
    id: String,
    message: String,
) -> Result<Value, String> {
    call(&state, Method::TaskSteer, json!({ "task": id, "message": message }))
}

#[tauri::command]
pub async fn get_ledger(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    call(&state, Method::TaskLedger, json!({ "task": id }))
}

/// U7 v1 反馈闭环：结果卡 👍/👎 + 理由 → daemon 落事件 + 项目记忆
#[tauri::command]
pub async fn task_feedback(
    state: State<'_, AppState>,
    id: String,
    positive: bool,
    reason: Option<String>,
) -> Result<Value, String> {
    call(
        &state,
        Method::TaskFeedback,
        json!({ "task": id, "positive": positive, "reason": reason }),
    )
}

#[tauri::command]
pub async fn list_workers(state: State<'_, AppState>) -> Result<Value, String> {
    call(&state, Method::WorkerList, json!({}))
}

#[tauri::command]
pub async fn list_inbox(state: State<'_, AppState>) -> Result<Value, String> {
    call(&state, Method::InboxList, json!({}))
}

// ---- 急停 / 恢复 ----

#[tauri::command]
pub async fn emergency_stop(state: State<'_, AppState>, reason: Option<String>) -> Result<Value, String> {
    call(&state, Method::ServerEmergencyStop, json!({ "reason": reason.unwrap_or_else(|| "desktop-ui".into()) }))
}

#[tauri::command]
pub async fn resume_all(state: State<'_, AppState>, steering: String) -> Result<Value, String> {
    let s = if steering == "hold" { "hold" } else { "flush" };
    call(&state, Method::ServerResumeAll, json!({ "steering": s }))
}

// ---- checkpoint ----

#[tauri::command]
pub async fn list_checkpoints(state: State<'_, AppState>, task: String) -> Result<Value, String> {
    call(&state, Method::CheckpointList, json!({ "task": task }))
}

#[tauri::command]
pub async fn rollback_checkpoint(
    state: State<'_, AppState>,
    task: String,
    to: String,
) -> Result<Value, String> {
    call(&state, Method::CheckpointRollback, json!({ "task": task, "to": to }))
}

// ---- 设置 / 探测 ----

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Value, String> {
    let s = state.settings.lock().unwrap().clone();
    serde_json::to_value(s).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    settings: Value,
) -> Result<(), String> {
    let s: Settings =
        serde_json::from_value(settings).map_err(|e| format!("设置格式错误: {e}"))?;
    settings::save(&s).map_err(|e| format!("保存失败: {e}"))?;
    let state = app.state::<AppState>();
    *state.settings.lock().unwrap() = s;
    Ok(())
}

#[tauri::command]
pub async fn probe_worker(state: State<'_, AppState>) -> Result<Value, String> {
    let s = state.settings.lock().unwrap().clone();
    let p = |name: &str| {
        tools::resolve_tool(name)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    Ok(json!({
        "mode": serde_json::to_value(s.worker_mode).unwrap_or(json!("demo")),
        "claude_found": tools::detect_claude(),
        "daemon_path": p("maestro-daemon"),
        "rounder_path": p("maestro-rounder"),
        "mock_path": p("mock-cli"),
        "data_dir": state.data_dir.to_string_lossy(),
    }))
}

// ---- U5 结果卡：变更明细（task_diff）----

/// diff 钳制上限：结果卡 inline 展示，超长截断（完整内容看工作区 git）
const MAX_PATCH_BYTES: usize = 96 * 1024;

/// task_diff：baseline checkpoint → 当前工作区的变更（stat + patch）。
/// 结果卡「变更明细」入口。只读操作，桌面进程直接跑 git ——
/// 仓库布局与 daemon checkpoints.rs 一致（refs/maestro/cp/<task>/<seq>-<label>），
/// baseline 为任务入队时的 seq=1 锚点（缺失则取最早一条 checkpoint）。
#[tauri::command]
pub async fn task_diff(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    let unavailable = |reason: String| Ok(json!({ "available": false, "reason": reason }));

    // workdir 从 daemon 拿（task_get 响应的权威字段），防前端传陈旧路径
    let task = call(&state, Method::TaskGet, json!({ "task": id }))?;
    let workdir = task["workdir"].as_str().unwrap_or_default().to_string();
    if workdir.is_empty() {
        return unavailable("任务无 workdir（无变更明细）".into());
    }

    // baseline：version 排序下最早一条（正常即 1-baseline；
    // for-each-ref 前缀匹配到 / 为止，t-1 不会误吞 t-10 的引用）
    let refs = match git_out(
        &workdir,
        &[
            "for-each-ref",
            &format!("refs/maestro/cp/{id}"),
            "--sort=version:refname",
            "--format=%(refname)",
        ],
    ) {
        Ok(r) => r,
        Err(e) => return unavailable(format!("git 不可用：{e}")),
    };
    let Some(baseline) = refs.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return unavailable("无 checkpoint（任务未产生快照）".into());
    };

    // stat/patch：临时 index 三步法（含 untracked、排除 .maestro/ 噪声）
    let numstat = baseline_workdir_diff(&workdir, baseline, "numstat")?;
    let (files, ins, del) = parse_numstat(&numstat);

    let patch_bytes = baseline_workdir_diff(&workdir, baseline, "patch")?
        .into_bytes();
    let truncated = patch_bytes.len() > MAX_PATCH_BYTES;
    let patch = clamp_utf8(&patch_bytes, MAX_PATCH_BYTES);

    Ok(json!({
        "available": true,
        "baseline": baseline,
        "files": files,
        "insertions": ins,
        "deletions": del,
        "patch": patch,
        "truncated": truncated,
    }))
}

fn git_out(workdir: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// numstat 解析：`ins\tdel\tpath` 每行一条；二进制文件增删为 `-`（只计文件数）
fn parse_numstat(s: &str) -> (u64, u64, u64) {
    let (mut files, mut ins, mut del) = (0, 0, 0);
    for line in s.lines() {
        if line.is_empty() {
            continue;
        }
        files += 1;
        let mut parts = line.splitn(3, '\t');
        ins += parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        del += parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    }
    (files, ins, del)
}

/// 字节钳制 + UTF-8 字符边界截断（回退越过续字节 0b10xxxxxx，防多字节字符切断）
fn clamp_utf8(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut end = max;
    while end > 0 && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
        end -= 1;
    }
    let mut s = String::from_utf8_lossy(&bytes[..end]).into_owned();
    s.push_str("\n…（diff 过长已截断，完整变更请查看工作区 git）");
    s
}

// ---- C2：hunk 级部分接受 ----
//
// 语义：工作区即「全部改动」（默认全接受）。用户逐 hunk 拒绝 →
// 被拒 hunk 从工作区 reverse-apply 撤销 + 附理由自动转修正任务。
//
// 契约：前后端对同一 patch 文本按同一规则解析 hunk ——
// `diff --git` 分文件、`@@` 分 hunk、全局序号 = 解析顺序（文件序 × hunk 序）。

/// baseline vs 工作区（**含 untracked**）的 diff —— 临时 index 三步法：
/// `read-tree baseline → add -A → write-tree → diff-tree`。
/// 直接 `git diff <ref>` 只比 tracked 文件，而 baseline capture 曾
/// `add -A` 烧入 untracked → 它们在 diff 里全部误报为「删除」（实测：
/// demo-result.md 明明存在却 +0 −365，reverse apply 撞存活文件）。
/// GIT_INDEX_FILE 指向临时文件，真 index 零扰动。文件名带纳秒时间戳
/// —— 同进程并发调用（两个 task_diff 在飞）不得共享临时 index 交叉写。
fn baseline_workdir_diff(
    workdir: &str,
    baseline: &str,
    mode: &str, // "patch" | "numstat"
) -> Result<String, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("maestro-idx-{}-{nanos}", std::process::id()));
    let tmp_s = tmp.to_string_lossy().into_owned();
    let run = |args: &[&str]| -> Result<String, String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(workdir)
            .args(args)
            .env("GIT_INDEX_FILE", &tmp_s)
            .output()
            .map_err(|e| format!("spawn git: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    let result = (|| {
        run(&["read-tree", baseline])?;
        // .maestro/ 是 daemon 运行时数据（任务产物随生命周期增删）—— 排除
        run(&["add", "-A", "--", ".", ":(exclude).maestro"])?;
        let now_tree = run(&["write-tree"])?;
        match mode {
            "numstat" => run(&[
                "-c", "core.quotepath=false", "diff-tree", "--no-commit-id",
                "-r", "--numstat", baseline, &now_tree,
            ]),
            _ => run(&[
                "diff-tree", "--no-commit-id", "-r", "-p", "--no-color",
                baseline, &now_tree,
            ]),
        }
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// patch 中单个文件段：header（diff --git … --- … +++）+ hunks（@@ 起）
struct FileDiff {
    header: Vec<String>,
    hunks: Vec<String>,
}

/// 把 unified diff 文本拆成文件段。header 含首个 @@ 前的全部行
///（diff --git / index / --- / +++ / 新文件模式行等）。
fn split_patch(patch: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileDiff> = None;
    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            if let Some(f) = cur.take() {
                files.push(f);
            }
            cur = Some(FileDiff { header: vec![line.to_string()], hunks: Vec::new() });
        } else if let Some(f) = cur.as_mut() {
            if line.starts_with("@@") {
                f.hunks.push(String::new());
            }
            if f.hunks.is_empty() {
                // @@ 之前都属 header（含二进制文件的 GIT binary patch 段）
                f.header.push(line.to_string());
            } else if let Some(last) = f.hunks.last_mut() {
                last.push_str(line);
                last.push('\n');
            }
        }
    }
    if let Some(f) = cur {
        files.push(f);
    }
    files
}

/// 按全局 hunk 索引拼「拒绝子 patch」：每个含被拒 hunk 的文件保留
/// 其 header + 被拒 hunk 正文。序号顺序与 split_patch 一致。
fn build_reject_patch(patch: &str, reject: &[usize]) -> Result<String, String> {
    if reject.is_empty() {
        return Err("未选择任何要拒绝的 hunk".into());
    }
    let wanted: std::collections::BTreeSet<usize> = reject.iter().copied().collect();
    let mut out = String::new();
    let mut idx = 0usize;
    let mut picked = 0usize;
    for f in split_patch(patch) {
        // 文件内被拒的 hunk（按出现顺序）
        let chosen: Vec<&String> = f
            .hunks
            .iter()
            .enumerate()
            .filter(|(i, _)| wanted.contains(&(idx + i)))
            .map(|(_, h)| h)
            .collect();
        idx += f.hunks.len();
        if chosen.is_empty() {
            continue;
        }
        for l in &f.header {
            out.push_str(l);
            out.push('\n');
        }
        for h in chosen {
            out.push_str(h);
            picked += 1;
        }
    }
    if picked != wanted.len() {
        return Err(format!(
            "hunk 索引越界：选中 {} 段，实际找到 {} 段（diff 可能已变化，请刷新）",
            wanted.len(),
            picked
        ));
    }
    Ok(out)
}

/// 被拒 hunk 的修正任务 prompt（正文钳制，防巨 diff 打爆上下文）
fn followup_prompt(task_title: &str, patch: &str, reject: &[usize], reason: &str) -> String {
    let mut out = format!(
        "修正任务（源自「{task_title}」被拒绝的改动段）：\n拒绝理由：{reason}\n请按理由重新实现以下改动：\n"
    );
    let mut idx = 0usize;
    let mut picked = 0usize;
    for f in split_patch(patch) {
        // 文件路径：+++ b/（b 侧）；删除文件 b 侧为 /dev/null → 回退 --- a/
        let path = f
            .header
            .iter()
            .find_map(|l| l.strip_prefix("+++ b/"))
            .or_else(|| {
                f.header.iter().find_map(|l| l.strip_prefix("--- a/"))
            })
            .unwrap_or("(未知文件)");
        for (i, h) in f.hunks.iter().enumerate() {
            if reject.contains(&(idx + i)) {
                let head = h.lines().next().unwrap_or("").to_string();
                out.push_str(&format!("\n--- {path} {head}\n"));
                out.push_str(&clamp_utf8(h.as_bytes(), 2_048));
                out.push('\n');
                picked += 1;
            }
        }
        idx += f.hunks.len();
    }
    if picked == 0 {
        out.push_str("\n（无具体段落 —— 请按拒绝理由整体复查）");
    }
    out
}

/// task_diff_revert：拒绝所选 hunk —— 从工作区 reverse-apply 撤销 +
/// 附理由自动转修正任务（同 workdir）一条龙。
///
/// `patch` 为**前端展示的 diff 原文**（用户勾选即基于它）：不能在提交时
/// 重新拉取 —— 查看与提交之间工作区若被并发改动（另一任务写同 workdir），
/// 重拉的 patch 里同序号可能是别的 hunk → 静默撤销错误内容（apply 照样
/// 成功，无报错）。以所见原文为准：过期则 git apply --reverse 原子失败，
/// 大声报错让用户重开 diff。
#[tauri::command]
pub async fn task_diff_revert(
    state: State<'_, AppState>,
    id: String,
    hunks: Vec<usize>,
    reason: String,
    patch: String,
) -> Result<Value, String> {
    if reason.trim().is_empty() {
        return Err("拒绝理由不能为空".into());
    }
    // workdir 定位与 task_diff 一致（apply 目标目录）
    let task = call(&state, Method::TaskGet, json!({ "task": id }))?;
    let title = task["title"].as_str().unwrap_or("").to_string();
    let workdir = task["workdir"].as_str().unwrap_or_default().to_string();
    if workdir.is_empty() {
        return Err("任务无 workdir（无可拒绝的变更）".into());
    }
    if patch.trim().is_empty() {
        return Err("diff 内容为空（请重新展开变更明细）".into());
    }

    // 1. 拼拒绝子 patch（所见原文上的索引；越界/勾空在此报错）
    let sub = build_reject_patch(&patch, &hunks)?;

    // 2. reverse-apply 撤销（stdin 传 patch；--whitespace=nowarn 防 CJK 尾空格噪声）
    let apply = Command::new("git")
        .arg("-C")
        .arg(&workdir)
        .args(["apply", "--reverse", "--whitespace=nowarn", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn git apply: {e}"))?;
    apply
        .stdin
        .as_ref()
        .unwrap()
        .write_all(sub.as_bytes())
        .map_err(|e| format!("write patch: {e}"))?;
    let out = apply.wait_with_output().map_err(|e| format!("wait git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git apply --reverse 失败（工作区可能已变化）：\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    // 3. 转修正任务（同 workdir；prompt 附被拒 hunk + 理由）
    let prompt = followup_prompt(&title, &patch, &hunks, reason.trim());
    let followup = call(
        &state,
        Method::TaskCreate,
        json!({
            "title": format!("修正: {title}"),
            "prompt": prompt,
            "workdir": workdir,
        }),
    )?;

    let reverted_files = split_patch(&sub).len();
    Ok(json!({
        "reverted_files": reverted_files,
        "reverted_hunks": hunks.len(),
        "followup_task": followup["task"]["id"],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numstat_parses_and_skips_binary() {
        let (f, i, d) = parse_numstat("12\t3\tsrc/a.rs\n-\t-\tlogo.png\n\n5\t0\tsrc/b.rs");
        assert_eq!((f, i, d), (3, 17, 3));
    }

    #[test]
    fn clamp_cuts_on_char_boundary() {
        // "中" 3 字节 / "文" 3 字节：max=5 应回退到 3 —— 保留完整一个字符
        let s = clamp_utf8("中文".as_bytes(), 5);
        assert_eq!(s.matches('中').count(), 1);
        assert!(s.contains("截断"));
        // 未超限原样返回
        assert_eq!(clamp_utf8(b"abc", 10), "abc");
    }

    fn sample_patch() -> &'static str {
        "diff --git a/src/a.rs b/src/a.rs\nindex 111..222 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,4 @@\n ctx\n-old\n+new\n+added\n@@ -10,2 +11,2 @@\n x\n-y\n+z\ndiff --git a/README.md b/README.md\nindex 333..444 100644\n--- a/README.md\n+++ b/README.md\n@@ -1,1 +1,1 @@\n-hi\n+hello\n"
    }

    #[test]
    fn split_patch_files_and_hunks() {
        let files = split_patch(sample_patch());
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[1].hunks.len(), 1);
        // header 到首个 @@ 为止（不含 @@）
        assert!(files[0].header.iter().all(|l| !l.starts_with("@@")));
        assert!(files[0].header.iter().any(|l| l.starts_with("+++ b/")));
        // hunk 正文以 @@ 开头且带尾换行
        assert!(files[0].hunks[0].starts_with("@@ -1,3 +1,4 @@\n"));
        assert!(files[0].hunks[1].ends_with('\n'));
    }

    #[test]
    fn build_reject_picks_cross_file_hunks() {
        let p = sample_patch();
        // 全局序号：a.rs#0 a.rs#1 README#2
        let sub = build_reject_patch(p, &[1, 2]).unwrap();
        // 两个文件各取其 header
        assert_eq!(sub.matches("diff --git").count(), 2);
        // hunk 头行计数（"@@ … @@" 头行本身含两个 @@ 字面量，须按行首统计）
        let hunk_heads = sub.lines().filter(|l| l.starts_with("@@")).count();
        assert_eq!(hunk_heads, 2);
        assert!(sub.contains("@@ -10,2 +11,2 @@"));
        assert!(sub.contains("@@ -1,1 +1,1 @@"));
        // 未选中的 a.rs#0 不得混入
        assert!(!sub.contains("@@ -1,3 +1,4 @@"));
    }

    #[test]
    fn build_reject_out_of_range_and_empty() {
        let p = sample_patch();
        assert!(build_reject_patch(p, &[]).is_err());
        let err = build_reject_patch(p, &[9]).unwrap_err();
        assert!(err.contains("越界"));
        // 同 hunk 重复序号按去重计
        let sub = build_reject_patch(p, &[0, 0]).unwrap();
        let hunk_heads = sub.lines().filter(|l| l.starts_with("@@")).count();
        assert_eq!(hunk_heads, 1);
    }

    #[test]
    fn followup_prompt_lists_rejected_hunks() {
        let p = sample_patch();
        let s = followup_prompt("标题", p, &[2], "风格不对");
        assert!(s.contains("拒绝理由：风格不对"));
        assert!(s.contains("README.md"));
        assert!(s.contains("@@ -1,1 +1,1 @@"));
        assert!(!s.contains("a.rs"));
    }
}
