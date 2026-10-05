//! settings.json 读写（原子写：tmp + rename）

use crate::state::app_root;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerMode {
    /// 内置 mock worker（无需 AI key，演示全流程）
    Demo,
    /// 本机 claude CLI（多轮驱动）
    Claude,
    /// 自定义 headless CLI（claude stream-json 兼容或配 MAESTRO_CLI_DIALECT）
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub worker_mode: WorkerMode,
    #[serde(default)]
    pub custom_program: String,
    #[serde(default)]
    pub custom_args: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            worker_mode: WorkerMode::Demo,
            custom_program: String::new(),
            custom_args: String::new(),
        }
    }
}

fn path() -> PathBuf {
    app_root().join("settings.json")
}

pub fn load() -> Settings {
    let p = path();
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(s: &Settings) -> std::io::Result<()> {
    let p = path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(s).unwrap())?;
    std::fs::rename(&tmp, &p)
}
