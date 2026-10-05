//! sidecar 与外部工具定位。
//!
//! 查找顺序（覆盖 dev / 安装包两种布局）：
//! 1. 主程序同目录 `<name>-<TRIPLE>.exe`（Tauri externalBin 安装后的形态，保留 triple 命名）
//! 2. 主程序同目录 `<name>.exe`（dev 下 cargo 直出）
//! 3. `src-tauri/bin/<name>-<TRIPLE>.exe`（dev 下手工拷贝的 sidecar）

use std::path::{Path, PathBuf};

pub fn resolve_tool(name: &str) -> Option<PathBuf> {
    let triple = env!("MAESTRO_TARGET");
    let suffixed = format!("{name}-{triple}.exe");
    let plain = format!("{name}.exe");

    if let Ok(exe) = std::env::current_exe() {
        let dir = exe.parent()?.to_path_buf();
        for f in [&suffixed, &plain] {
            let p = dir.join(f);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("bin").join(&suffixed);
    if dev.is_file() {
        return Some(dev);
    }
    None
}

/// PATH 里找 claude CLI（设置页探测用）
pub fn detect_claude() -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(';').filter(|s| !s.is_empty()) {
        for cand in ["claude.exe", "claude.cmd", "claude.bat"] {
            if Path::new(dir).join(cand).is_file() {
                return true;
            }
        }
    }
    false
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
