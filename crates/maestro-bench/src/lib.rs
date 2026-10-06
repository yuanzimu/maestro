//! C7 效果基准集（benchmark-suite）。
//!
//! 组成：
//! - `suite/`：基准任务数据（每任务 = task.toml 清单 + fixture 带缺陷仓库
//!   模板 + solution 修复后文件）。manifest 而非硬编码 —— 20 → N 扩展零代码。
//! - runner：进程内拉起 daemon Core（真实 socket + rounder + bench-worker
//!   全链），逐任务跑 → 跑验收命令 → 收账本（TaskLedger / C6 LedgerSummary）
//!   → 产出 JSON + Markdown 报告。mock 模式确定性可复跑（CI 门禁），
//!   real 模式（真实 provider 对比）留后续。
//! - 验收口径：`accept` 命令（argv 形式，无 shell 解析歧义）退出码。
//!   fixture 初态必须红（验收失败）、应用 solution 后必须绿 ——
//!   `tests/suite_integrity.rs` 离线校验（不依赖 daemon），是任务集自身的
//!   红绿不变量。

pub mod manifest;
pub mod report;
pub mod runner;
