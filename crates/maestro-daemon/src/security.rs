//! 安全基线 v0（DEV_PLAN 0.11）：输入校验 + 敏感路径防护 + ref 命名空间约束。
//!
//! 威胁模型：本机多用户/误操作。daemon 以属主权限运行，socket 已 0600；
//! 这里的目标是把 API 可达的「任意路径访问 / 任意 ref 重写 / 事件库膨胀」
//! 挡在 Core 之前。
//!
//! Server 版（AI nginx）会把 deny-list 升级为 allowlist（E2/E4），P0 v0
//! 先挡敏感目录 + 穿越模式。

/// 敏感目录（禁止作为 workdir）
const DENIED_WORKDIRS: &[&str] = &[
    "/", "/etc", "/proc", "/sys", "/dev", "/boot", "/run", "/var", "/usr", "/lib", "/lib64",
    "/bin", "/sbin", "/root",
];

/// workdir 路径组件黑名单（凭据/密钥材料）
const DENIED_COMPONENTS: &[&str] = &[".ssh", ".gnupg", ".aws", ".kube", ".docker"];

/// 输入上限（防事件库/队列膨胀 DoS）
pub const MAX_TITLE_LEN: usize = 200;
pub const MAX_PROMPT_LEN: usize = 100_000;
pub const MAX_STEER_LEN: usize = 10_000;

/// workdir 校验：存在、是目录、非敏感路径、无凭据子路径、无穿越
pub fn validate_workdir(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    // 穿越模式先拦（canonicalize 前的原始串）
    if path.contains("..") {
        return Err("workdir 不得包含 ..".into());
    }
    // 相对路径一律拒绝（解析取决于 daemon cwd，歧义）
    if !p.is_absolute() {
        return Err("workdir 必须是绝对路径".into());
    }
    // 敏感目录按原串先拦（CI 教训：macOS 的 /etc 是符号链接，
    // canonicalize 后变 /private/etc 会绕过等值比较；且目录不存在时
    // 应报「敏感目录」而非「不存在」）
    for d in DENIED_WORKDIRS {
        if path == *d {
            return Err(format!("敏感目录禁止作为 workdir: {d}"));
        }
    }
    let canon = p
        .canonicalize()
        .map_err(|_| format!("workdir 不存在: {path}"))?;
    if !canon.is_dir() {
        return Err(format!("workdir 不是目录: {path}"));
    }
    let s = canon.display().to_string();
    // canonicalize 后再拦一道（防 /private/etc 之类真实路径直接传入）
    for d in DENIED_WORKDIRS {
        if s == *d {
            return Err(format!("敏感目录禁止作为 workdir: {d}"));
        }
    }
    // 组件级匹配（tmp/proj/.ssh 与 tmp/proj/.ssh/x 都拦）
    let hit = canon
        .components()
        .any(|c| DENIED_COMPONENTS.contains(&c.as_os_str().to_string_lossy().as_ref()));
    if hit {
        return Err("workdir 不得包含凭据目录（.ssh/.gnupg/.aws 等）".into());
    }
    Ok(())
}

/// rollback 目标 ref 校验：只允许 maestro checkpoint 命名空间。
/// 防止经 cp-rollback 重写任意 ref（如 refs/heads/main → 毁分支）。
pub fn validate_rollback_ref(r: &str) -> Result<(), String> {
    if !r.starts_with("refs/maestro/cp/") {
        return Err("rollback 目标必须在 refs/maestro/cp/ 命名空间内".into());
    }
    if r.len() > 256 {
        return Err("ref 过长".into());
    }
    if !r
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '.' | '-'))
    {
        return Err("ref 含非法字符".into());
    }
    if r.contains("..") {
        return Err("ref 不得包含 ..".into());
    }
    Ok(())
}

/// 任务输入长度校验
pub fn validate_task_input(title: &str, prompt: &str) -> Result<(), String> {
    if title.is_empty() {
        return Err("title 不能为空".into());
    }
    if title.chars().count() > MAX_TITLE_LEN {
        return Err(format!("title 超长（>{MAX_TITLE_LEN} 字符）"));
    }
    if prompt.chars().count() > MAX_PROMPT_LEN {
        return Err(format!("prompt 超长（>{MAX_PROMPT_LEN} 字符）"));
    }
    Ok(())
}

/// steering 消息长度校验
pub fn validate_steer_message(msg: &str) -> Result<(), String> {
    if msg.is_empty() {
        return Err("消息不能为空".into());
    }
    if msg.chars().count() > MAX_STEER_LEN {
        return Err(format!("消息超长（>{MAX_STEER_LEN} 字符）"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workdir_must_exist_absolute() {
        assert!(validate_workdir("/nonexistent/maestro-sec-test").is_err());
        assert!(validate_workdir("relative/path").is_err());
        assert!(validate_workdir("/etc/../tmp").is_err(), "穿越拦截");
    }

    #[test]
    fn sensitive_dirs_denied() {
        for d in ["/etc", "/proc", "/root", "/"] {
            assert!(validate_workdir(d).is_err(), "{d} 应被拒绝");
        }
    }

    #[test]
    fn credential_subpaths_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let ssh = tmp.path().join("proj/.ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        assert!(validate_workdir(ssh.to_str().unwrap()).is_err());
    }

    #[test]
    fn normal_workdir_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(validate_workdir(tmp.path().to_str().unwrap()).is_ok());
    }

    #[test]
    fn rollback_ref_namespace_locked() {
        assert!(validate_rollback_ref("refs/maestro/cp/t-1/3-baseline").is_ok());
        assert!(
            validate_rollback_ref("refs/heads/main").is_err(),
            "主分支 ref 必须拒绝"
        );
        assert!(validate_rollback_ref("refs/maestro/cp/../../evil").is_err());
        assert!(
            validate_rollback_ref("refs/maestro/cp/t-1/a b").is_err(),
            "空格非法"
        );
    }

    #[test]
    fn input_length_limits() {
        assert!(validate_task_input("t", "p").is_ok());
        assert!(validate_task_input("", "p").is_err());
        assert!(validate_task_input(&"x".repeat(201), "p").is_err());
        assert!(validate_task_input("t", &"x".repeat(MAX_PROMPT_LEN + 1)).is_err());
        assert!(validate_steer_message(&"x".repeat(MAX_STEER_LEN + 1)).is_err());
        assert!(validate_steer_message("").is_err());
    }
}
