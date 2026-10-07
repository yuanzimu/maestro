//! sidecar 与外部工具定位（C3-4 跨平台化）。
//!
//! 查找顺序（覆盖 dev / 安装包两种布局）：
//! 1. 主程序同目录 `<name>-<TRIPLE><exe>`（Tauri externalBin 安装后的形态，保留 triple 命名）
//! 2. 主程序同目录 `<name><exe>`（dev 下 cargo 直出）
//! 3. `src-tauri/bin/<name>-<TRIPLE><exe>`（dev 下手工拷贝的 sidecar）
//!
//! 平台差异集中在 `tool_candidates` / `path_entries` 两个纯函数（可单测）：
//! - Windows：`.exe` 后缀、PATH 用 `;` 分隔、claude 三种封装都要探测
//! - Unix：无后缀、PATH 用 `:` 分隔、裸 `claude`（C3-4 支持任意 triple）

use std::path::{Path, PathBuf};

/// 平台可执行后缀（Windows `.exe`，Unix 空）
fn exe_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
    }
}

/// 一次查找的候选文件名（按优先级）：`<name>-<triple><sfx>` → `<name><sfx>`
fn tool_candidates(name: &str) -> Vec<String> {
    let triple = env!("MAESTRO_TARGET");
    let sfx = exe_suffix();
    vec![format!("{name}-{triple}{sfx}"), format!("{name}{sfx}")]
}

/// PATH 拆分（Windows `;` / Unix `:`；空段滤掉 —— `a;;b` 不产生 "" 段）
fn path_entries(path: &str) -> Vec<String> {
    let sep = if cfg!(windows) { ';' } else { ':' };
    path.split(sep)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn resolve_tool(name: &str) -> Option<PathBuf> {
    let candidates = tool_candidates(name);
    if let Ok(exe) = std::env::current_exe() {
        let dir = exe.parent()?.to_path_buf();
        for f in &candidates {
            let p = dir.join(f);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // dev 布局：src-tauri/bin/<name>-<TRIPLE><sfx>
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("bin");
    for f in &candidates {
        let p = dev.join(f);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// PATH 里找 claude CLI（设置页探测用）
pub fn detect_claude() -> bool {
    let variants: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd", "claude.bat"]
    } else {
        &["claude"]
    };
    let path = std::env::var("PATH").unwrap_or_default();
    path_entries(&path).iter().any(|dir| {
        variants
            .iter()
            .any(|cand| Path::new(dir).join(cand).is_file())
    })
}

/// 读日志文件尾部（启动失败诊断用）
pub fn log_tail(max_bytes: usize) -> String {
    let log = crate::state::app_root().join("logs").join("daemon.log");
    match std::fs::read(&log) {
        Ok(bytes) => {
            let mut start = bytes.len().saturating_sub(max_bytes);
            // 起点可能落在多字节 UTF-8 字符中间：直接切片会 panic。
            // 向前推进到一个字符边界（首字节满足 0b10xx_xxxx 的是续字节）。
            while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
                start += 1;
            }
            String::from_utf8_lossy(&bytes[start..]).into_owned()
        }
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 候选名优先级与后缀：triple 形态在前，裸名在后（C3-4）
    #[test]
    fn candidates_order_and_suffix() {
        let c = tool_candidates("maestro-daemon");
        assert_eq!(c.len(), 2, "triple + 裸名两个候选");
        assert!(
            c[0].starts_with("maestro-daemon-"),
            "triple 候选应在前: {c:?}"
        );
        if cfg!(windows) {
            assert!(c[0].ends_with(".exe") && c[1] == "maestro-daemon.exe");
        } else {
            assert!(c[1] == "maestro-daemon", "Unix 无后缀: {c:?}");
        }
    }

    /// PATH 拆分：按平台分隔符切、空段滤除（C3-4）
    #[test]
    fn path_split_filters_empty() {
        if cfg!(windows) {
            assert_eq!(path_entries("a;b;;c"), vec!["a", "b", "c"]);
            // Unix 形态在 Windows 是单一路径（含冒号），不误拆
            assert_eq!(path_entries("/usr/bin"), vec!["/usr/bin"]);
        } else {
            assert_eq!(path_entries("a:b::c"), vec!["a", "b", "c"]);
            // Windows 分号列表在 Unix 是单一路径，不误拆。样例须不含盘符
            // 冒号 —— `C:\...` 的冒号在 Unix 恰是分隔符必被拆（此前该断言
            // 只在 Windows 分支跑过，Linux 首跑即挂）
            assert_eq!(
                path_entries("\\\\srv\\share;\\\\srv2\\sh2"),
                vec!["\\\\srv\\share;\\\\srv2\\sh2"]
            );
        }
    }
}
