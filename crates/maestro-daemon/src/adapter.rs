//! 0.6 headless CLI 适配器框架（DEV_PLAN 0.6）。
//!
//! 三件套：
//! 1. **stream-json 解析**（`parse_stream_json`）：CLI 输出 → `RoundOutcome`
//!    （session/answer/usage/model/工具调用/错误子类）—— 状态机素材源
//! 2. **退出分类**（`classify_exit`）：stderr 模式 → `ExitClass`
//!    （断连/限流 → Disconnect = suspended(NetworkLost) 自动恢复 ≠ failed，设计 §2.4；
//!    认证/配额 → Fatal = 用户必须介入）
//! 3. **方言**（`Dialect`）：一轮调用的参数构造，claude/Codex/Gemini 可插拔（B7）
//!
// 真实 claude CLI 的端到端验证待 API key；mock CLI（testkit r3）已对齐子集。

use std::io::BufRead;

// ---------------------------------------------------------------------------
// 1. stream-json 解析
// ---------------------------------------------------------------------------

/// 一轮的解析结果
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoundOutcome {
    /// 会话引用（system 行）—— 续接凭据
    pub session_id: Option<String>,
    /// 本轮最终回答（result 行）
    pub answer: String,
    pub usage_in: u64,
    pub usage_out: u64,
    pub cache_read: Option<u64>,
    pub model: Option<String>,
    /// 本轮内发生的工具调用名（去重；U3 过程叙事素材）
    pub tools_used: Vec<String>,
    /// result.subtype（如 error_max_turns / error_during_execution）
    pub subtype: Option<String>,
    /// result.is_error
    pub is_error: bool,
}

/// 解析 stream-json stdout（逐行 JSON 事件流）。
/// 未识别行忽略（前向兼容：CLI 新增事件类型不破坏解析）。
pub fn parse_stream_json(stdout: &[u8]) -> RoundOutcome {
    let mut out = RoundOutcome::default();
    for line in stdout.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("system") => {
                if let Some(s) = v["session_id"].as_str() {
                    out.session_id = Some(s.to_string());
                }
            }
            Some("assistant") => {
                // content 块数组里的 tool_use → 记名（U3 叙事）
                if let Some(blocks) = v["message"]["content"].as_array() {
                    for b in blocks {
                        if b["type"] == "tool_use" {
                            if let Some(name) = b["name"].as_str() {
                                if !out.tools_used.iter().any(|t| t == name) {
                                    out.tools_used.push(name.to_string());
                                }
                            }
                        }
                    }
                }
            }
            Some("result") => {
                if let Some(s) = v["result"].as_str() {
                    out.answer = s.to_string();
                }
                out.usage_in = v["usage"]["input_tokens"].as_u64().unwrap_or(0);
                out.usage_out = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
                out.cache_read = v["usage"]["cache_read_input_tokens"].as_u64();
                if let Some(m) = v["model"].as_str() {
                    out.model = Some(m.to_string());
                }
                out.subtype = v["subtype"].as_str().map(String::from);
                out.is_error = v["is_error"].as_bool().unwrap_or(false);
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 2. 退出分类（stderr 模式 → 恢复语义）
// ---------------------------------------------------------------------------

/// Worker/CLI 非零退出的分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitClass {
    /// 断连/限流/过载 → suspended(NetworkLost)，自动恢复（≠ failed，设计 §2.4）
    Disconnect,
    /// 认证/配额/未知错误 → failed（用户必须介入）
    Fatal,
}

/// 认证/配额类致命模式（重试无意义，只会烧钱）
const FATAL_PATTERNS: &[&str] = &[
    "invalid api key",
    "authentication error",
    "authentication_error",
    "permission denied",
    "credit balance",
    "billing",
    "quota exceeded",
];

/// 断连类模式（含 Anthropic 429/529 过载 —— 等一会就好）
const DISCONNECT_PATTERNS: &[&str] = &[
    "connection reset",
    "connection refused",
    "connection error",
    "timeout",
    "timed out",
    "econnrefused",
    "fetch failed",
    "network error",
    "epipe",
    "rate limit",
    "rate_limit_error",
    "overloaded_error",
    "api error: 429",
    "api error: 5",
    "service unavailable",
];

/// 分类非零退出。规则：致命模式优先（防「auth 错误信息里恰好含 timeout」误判为可恢复），
/// 其后断连模式，默认 Fatal（未知错误按最坏处理 —— 自动恢复未知错误可能反复烧钱）。
pub fn classify_exit(_code: Option<i32>, stderr: &str) -> ExitClass {
    let lower = stderr.to_lowercase();
    if FATAL_PATTERNS.iter().any(|p| lower.contains(p)) {
        return ExitClass::Fatal;
    }
    if DISCONNECT_PATTERNS.iter().any(|p| lower.contains(p)) {
        return ExitClass::Disconnect;
    }
    ExitClass::Fatal
}

// ---------------------------------------------------------------------------
// 3. 方言（一轮调用的参数构造）
// ---------------------------------------------------------------------------

/// CLI 方言：如何为「一轮」构造参数（多轮驱动 Worker 消费）
pub trait Dialect: Send + Sync {
    fn name(&self) -> &'static str;
    /// 一轮的参数：prompt 必达；resume 存在则续接会话
    fn round_args(&self, prompt: &str, resume: Option<&str>) -> Vec<String>;
}

/// claude CLI 方言（默认；testkit mock CLI 对齐此子集）
pub struct ClaudeDialect;

impl Dialect for ClaudeDialect {
    fn name(&self) -> &'static str {
        "claude"
    }
    fn round_args(&self, prompt: &str, resume: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "-p".into(),
            prompt.into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            // 单轮闸：轮边界由 maestro-rounder 持有（多轮驱动 = 我们的循环）
            "--max-turns".into(),
            "1".into(),
        ];
        if let Some(sid) = resume {
            args.push("--resume".into());
            args.push(sid.into());
        }
        args
    }
}

/// 按名取方言（MAESTRO_CLI_DIALECT；未知名回落 claude）
pub fn dialect_by_name(name: &str) -> Box<dyn Dialect> {
    match name {
        "claude" | "" => Box::new(ClaudeDialect),
        other => {
            tracing::warn!("未知方言 {other}，回落 claude");
            Box::new(ClaudeDialect)
        }
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(lines: &[&str]) -> Vec<u8> {
        lines.join("\n").into_bytes()
    }

    /// 完整事件流：system + assistant(tool_use) + result
    #[test]
    fn parses_full_stream() {
        let out = parse_stream_json(&stream(&[
            r#"{"type":"system","session_id":"sid-1"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read"},{"type":"tool_use","name":"Bash"},{"type":"tool_use","name":"Read"}]}}"#,
            r#"{"type":"result","result":"done","subtype":"success","model":"claude-sonnet-4","usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":60}}"#,
        ]));
        assert_eq!(out.session_id.as_deref(), Some("sid-1"));
        assert_eq!(out.answer, "done");
        assert_eq!(out.usage_in, 100);
        assert_eq!(out.usage_out, 20);
        assert_eq!(out.cache_read, Some(60));
        assert_eq!(out.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(out.tools_used, vec!["Read", "Bash"], "工具名去重");
        assert_eq!(out.subtype.as_deref(), Some("success"));
        assert!(!out.is_error);
    }

    /// error_max_turns：--max-turns 1 的正常出口（is_error 但轮循环继续）
    #[test]
    fn parses_error_max_turns() {
        let out = parse_stream_json(&stream(&[
            r#"{"type":"system","session_id":"s"}"#,
            r#"{"type":"result","result":"partial","subtype":"error_max_turns","is_error":true,"usage":{"input_tokens":1,"output_tokens":2}}"#,
        ]));
        assert!(out.is_error);
        assert_eq!(out.subtype.as_deref(), Some("error_max_turns"));
        assert_eq!(out.answer, "partial");
    }

    /// 未识别事件类型忽略（前向兼容）；空流全默认
    #[test]
    fn unknown_events_ignored() {
        let out = parse_stream_json(&stream(&[
            r#"{"type":"future_event","payload":123}"#,
            "not json at all",
            "",
        ]));
        assert_eq!(out, RoundOutcome::default());
    }

    /// 断连分类矩阵
    #[test]
    fn classifies_disconnects() {
        for s in [
            "Error: connection reset by peer",
            "fetch failed: network error",
            "Request timed out",
            "API Error: 429 rate limit exceeded",
            "API Error: 529 overloaded_error",
            "epipe",
        ] {
            assert_eq!(classify_exit(Some(1), s), ExitClass::Disconnect, "{s}");
        }
    }

    /// 致命分类：认证/配额（且优先于断连模式 —— 防误恢复烧钱）
    #[test]
    fn classifies_fatal_auth() {
        for s in [
            "invalid api key provided",
            "authentication_error: 401",
            "credit balance too low",
            "quota exceeded for this org",
        ] {
            assert_eq!(classify_exit(Some(1), s), ExitClass::Fatal, "{s}");
        }
        // 致命优先：auth 错误信息里带 timeout 字样也判 Fatal
        assert_eq!(
            classify_exit(Some(1), "authentication error (request timeout)"),
            ExitClass::Fatal
        );
        // 未知错误默认 Fatal
        assert_eq!(classify_exit(Some(1), "boom"), ExitClass::Fatal);
        assert_eq!(classify_exit(None, ""), ExitClass::Fatal);
    }

    /// claude 方言参数构造
    #[test]
    fn claude_dialect_args() {
        let d = ClaudeDialect;
        let args = d.round_args("做点事", None);
        assert_eq!(args[..2], ["-p".to_string(), "做点事".to_string()]);
        assert!(!args.iter().any(|a| a == "--resume"));
        let args = d.round_args("继续", Some("sid-9"));
        let i = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[i + 1], "sid-9");
    }

    /// 方言注册表：未知回落 claude
    #[test]
    fn dialect_fallback() {
        assert_eq!(dialect_by_name("claude").name(), "claude");
        assert_eq!(dialect_by_name("").name(), "claude");
        assert_eq!(dialect_by_name("codex-future").name(), "claude");
    }
}
