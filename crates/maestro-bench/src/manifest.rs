//! 基准任务清单（task.toml）解析与套件发现。

use std::path::{Path, PathBuf};

/// 一个基准任务的清单。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TaskManifest {
    /// 任务 ID（必须与目录名一致 —— bench-worker 按 cwd 父目录名定位 solution）
    pub id: String,
    /// 标题（进报告）
    pub title: String,
    /// issue | refactor | feature
    pub kind: String,
    /// 任务指令（真实模式下即发给 LLM worker 的 prompt）
    pub prompt: String,
    /// 验收命令（argv 形式，在任务 workdir 内执行；退出码 0 = 通过）
    pub accept: Vec<String>,
    /// 验收超时（秒）
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    300
}

/// 套件内一个任务（清单 + 目录）。
#[derive(Debug, Clone)]
pub struct BenchTask {
    pub manifest: TaskManifest,
    /// 任务目录（含 task.toml / fixture / solution）
    pub dir: PathBuf,
}

impl BenchTask {
    pub fn fixture_dir(&self) -> PathBuf {
        self.dir.join("fixture")
    }

    pub fn solution_dir(&self) -> PathBuf {
        self.dir.join("solution")
    }
}

/// 发现并加载套件目录下全部任务（按 id 排序，稳定输出）。
pub fn load_suite(suite_dir: &Path) -> Result<Vec<BenchTask>, String> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(suite_dir)
        .map_err(|e| format!("读套件目录失败 {}: {e}", suite_dir.display()))?;
    for ent in entries.flatten() {
        let dir = ent.path();
        if !dir.is_dir() {
            continue;
        }
        let toml_path = dir.join("task.toml");
        if !toml_path.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(&toml_path)
            .map_err(|e| format!("读 {} 失败: {e}", toml_path.display()))?;
        let manifest: TaskManifest =
            toml::from_str(&raw).map_err(|e| format!("解析 {} 失败: {e}", toml_path.display()))?;
        let dir_name = ent.file_name().to_string_lossy().to_string();
        if manifest.id != dir_name {
            return Err(format!(
                "{}: 清单 id {:?} 与目录名 {:?} 不一致",
                toml_path.display(),
                manifest.id,
                dir_name
            ));
        }
        if manifest.accept.is_empty() {
            return Err(format!("{}: accept 不能为空", toml_path.display()));
        }
        if manifest.prompt.trim().is_empty() {
            return Err(format!("{}: prompt 不能为空", toml_path.display()));
        }
        if !matches!(manifest.kind.as_str(), "issue" | "refactor" | "feature") {
            return Err(format!(
                "{}: kind 须为 issue/refactor/feature，当前 {:?}",
                toml_path.display(),
                manifest.kind
            ));
        }
        if !dir.join("fixture").is_dir() {
            return Err(format!("{}: 缺 fixture/ 目录", toml_path.display()));
        }
        if !dir.join("solution").is_dir() {
            return Err(format!("{}: 缺 solution/ 目录", toml_path.display()));
        }
        out.push(BenchTask {
            manifest,
            dir: dir.clone(),
        });
    }
    if out.is_empty() {
        return Err(format!("套件目录无任务: {}", suite_dir.display()));
    }
    out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    Ok(out)
}

/// 递归复制目录（fixture → 任务 workdir）。写后把 mtime 拨到当前 ——
/// Windows fs::copy（CopyFileW）保留源文件时间戳；solution 覆盖场景若
/// 保留旧时间戳，会早于红跑编译产物，cargo 按 mtime 判「源未变」跳过
/// 重编译，绿跑拿到旧二进制。
pub fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建 {} 失败: {e}", dst.display()))?;
    let now = std::time::SystemTime::now();
    let entries = std::fs::read_dir(src).map_err(|e| format!("读 {} 失败: {e}", src.display()))?;
    for ent in entries.flatten() {
        let from = ent.path();
        let to = dst.join(ent.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("复制 {} 失败: {e}", from.display()))?;
            if let Ok(f) = std::fs::File::options().write(true).open(&to) {
                let _ = f.set_modified(now);
            }
        }
    }
    Ok(())
}

/// 在目录内执行 git 命令（fixture 仓库初始化用）。
fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git 启动失败（PATH 无 git？）: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {args:?} 失败: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// 复制 fixture 到 workdir 并初始化为 git 仓库（daemon baseline 依赖）。
pub fn materialize_fixture(task: &BenchTask, workdir: &Path) -> Result<(), String> {
    copy_dir(&task.fixture_dir(), workdir)?;
    git(workdir, &["init", "-q"])?;
    git(workdir, &["config", "user.email", "bench@maestro"])?;
    git(workdir, &["config", "user.name", "maestro-bench"])?;
    git(workdir, &["add", "-A"])?;
    git(workdir, &["commit", "-q", "-m", "fixture: 带缺陷初始态"])?;
    Ok(())
}

/// 把 solution/ 下的文件覆盖到 workdir（红转绿；bench-worker 同款逻辑，
/// 离线校验也直接用）。copy_dir 内部已对新写文件拨 mtime。
pub fn apply_solution(task: &BenchTask, workdir: &Path) -> Result<(), String> {
    copy_dir(&task.solution_dir(), workdir)
}

/// 运行验收命令；返回 (是否通过, stdout+stderr 尾部)。
/// 显式剥掉 CARGO_TARGET_DIR —— fixture 是独立 workspace，避免被外层
/// 环境的重定向污染（hermetic）。
pub fn run_accept(manifest: &TaskManifest, workdir: &Path) -> (bool, String) {
    let mut cmd = std::process::Command::new(&manifest.accept[0]);
    cmd.args(&manifest.accept[1..])
        .current_dir(workdir)
        .env_remove("CARGO_TARGET_DIR")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    match cmd.output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(15);
            let tail: String = lines[start..].join("\n");
            (out.status.success(), tail)
        }
        Err(e) => (
            false,
            format!("验收命令启动失败 {}: {e}", manifest.accept[0]),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suite_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("suite")
    }

    #[test]
    fn suite_loads_and_ids_are_consistent() {
        let tasks = load_suite(&suite_dir()).expect("套件应可加载");
        assert!(tasks.len() >= 10, "v0 至少 10 任务: {}", tasks.len());
        let kinds: Vec<&str> = tasks.iter().map(|t| t.manifest.kind.as_str()).collect();
        assert!(kinds.contains(&"issue"), "应含 issue 型任务");
        assert!(kinds.contains(&"refactor"), "应含 refactor 型任务");
        assert!(kinds.contains(&"feature"), "应含 feature 型任务");
    }
}
