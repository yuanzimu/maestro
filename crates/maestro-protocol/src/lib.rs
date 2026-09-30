//! Maestro 协议层：数据模型、事件类型、API schema。
//! 纯数据 crate，零 IO —— 一切开发的契约先行（DEV_PLAN P0.2）。

pub mod api;
pub mod clock;
pub mod events;
pub mod types;

pub use clock::{Clock, SystemClock};
pub use types::*;
