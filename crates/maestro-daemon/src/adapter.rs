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
    /// 写 cache 的 token（R28：Anthropic 单独计价项）
    pub cache_creation: Option<u64>,
    pub model: Option<String>,
    /// 本轮内发生的工具调用名（去重；U3 过程叙事素材）
    pub tools_used: Vec<String>,
    /// result.subtype（如 error_max_turns / error_during_execution）
    pub subtype: Option<String>,
    /// result.is_error
    pub is_error: bool,
    /// result.errors（结构化错误串，官方推荐的错误源 —— 优于解析 stderr）
    pub errors: Vec<String>,
    /// result.api_error_status（429/500/529 等HTTP 状态；可重试判定用）
    pub api_error_status: Option<u16>,
    /// result.total_cost_usd（CLI 自报本轮费用；daemon 对账用 —— R34）
    pub total_cost_usd: Option<f64>,
    /// 观察到的事件类型（去重保序）—— CLI schema 签名素材（R35）
    pub event_types: Vec<String>,
}

/// 解析 stream-json stdout（逐行 JSON 事件流）。
/// 未识别行忽略（前向兼容：CLI 新增事件类型不破坏解析）。
pub fn parse_stream_json(stdout: &[u8]) -> RoundOutcome {
    let mut out = RoundOutcome::default();
    for line in stdout.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(t) = v["type"].as_str() {
            if !out.event_types.iter().any(|e| e == t) {
                out.event_types.push(t.to_string());
            }
        }
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
                out.cache_creation = v["usage"]["cache_creation_input_tokens"].as_u64();
                if let Some(m) = v["model"].as_str() {
                    out.model = Some(m.to_string());
                }
                out.subtype = v["subtype"].as_str().map(String::from);
                out.is_error = v["is_error"].as_bool().unwrap_or(false);
                if let Some(errs) = v["errors"].as_array() {
                    out.errors = errs
                        .iter()
                        .filter_map(|e| e.as_str().map(String::from))
                        .collect();
                }
                out.api_error_status = v["api_error_status"]
                    .as_u64()
                    .and_then(|s| u16::try_from(s).ok());
                out.total_cost_usd = v["total_cost_usd"].as_f64();
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

/// schema 漂移检测（R35，R28 调研待办⑤）：CLI 输出对**关键事件/字段**的
/// feature-detect。Maestro 依赖三项不可前向兼容的要素——缺任一即核心
/// 能力（续接/计量/回答捕获）静默失效，必须在轮边界显式报错而非吞掉：
/// 1. system.session_id —— `--resume` 续接凭据
/// 2. result.result —— 本轮回答（DONE 信号检测源）
/// 3. result.usage.input_tokens —— 计量闭环（T4 账本）
/// 返回人话描述列表（空 = 无漂移）。未知**新**事件类型不算漂移（前向兼容）。
pub fn schema_drift(out: &RoundOutcome) -> Vec<String> {
    let mut drifts = vec![];
    if !out.event_types.iter().any(|t| t == "system") {
        drifts.push("缺 system 事件（session_id 续接凭据丢失）".into());
    } else if out.session_id.is_none() {
        drifts.push("system 事件缺 session_id 字段（CLI schema 变化？）".into());
    }
    if !out.event_types.iter().any(|t| t == "result") {
        drifts.push("缺 result 事件（answer/usage 丢失）".into());
    } else {
        if out.answer.is_empty() {
            drifts.push("result 事件缺 result 字段（回答捕获失效）".into());
        }
        if out.usage_in == 0 && out.usage_out == 0 {
            drifts.push("result 事件缺 usage（计量闭环破坏）".into());
        }
    }
    drifts
}

// ---------------------------------------------------------------------------
// 3. 方言（一轮调用的参数构造）
// ---------------------------------------------------------------------------

/// CLI 方言：如何为「一轮」构造参数（多轮驱动 Worker 消费）。
/// R2 调研四抽象点：①参数构造（round_args）②输出格式（parse_round，
/// 默认 claude stream-json）③续接语义（flag vs 子命令，round_args 表达）
/// ④错误分类（classify_exit 共用，退出码差异在方言成熟时下沉）
pub trait Dialect: Send + Sync {
    fn name(&self) -> &'static str;
    /// 一轮的参数：prompt 必达；resume 存在则续接会话
    fn round_args(&self, prompt: &str, resume: Option<&str>) -> Vec<String>;
    /// 解析一轮输出（默认 = claude stream-json 格式；事件格式不兼容的
    /// 方言如 Gemini/Codex 覆写此方法）
    fn parse_round(&self, stdout: &[u8]) -> RoundOutcome {
        parse_stream_json(stdout)
    }
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

/// Amp（Sourcegraph）方言（R41，B7 首个第二方言；R2 调研证实）：
/// headless 用 `-x` 位置参数；**事件格式 Claude Code 兼容**（--stream-json）
/// —— parse_round 直接复用默认实现。续接是子命令形态：
/// `amp threads continue <tid> -x <prompt> --stream-json`。
/// ⚠️ session 字段名待实测校准（内部 thread id 形如 T-<uuid>）
pub struct AmpDialect;

impl Dialect for AmpDialect {
    fn name(&self) -> &'static str {
        "amp"
    }
    fn round_args(&self, prompt: &str, resume: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = vec![];
        if let Some(tid) = resume {
            args.extend(["threads".into(), "continue".into(), tid.into()]);
        }
        args.extend(["-x".into(), prompt.into(), "--stream-json".into()]);
        args
    }
}

/// 按名取方言（MAESTRO_CLI_DIALECT；未知名回落 claude）
pub fn dialect_by_name(name: &str) -> Box<dyn Dialect> {
    match name {
        "claude" | "" => Box::new(ClaudeDialect),
        "amp" => Box::new(AmpDialect),
        other => {
            tracing::warn!("未知方言 {other}，回落 claude");
            Box::new(ClaudeDialect)
        }
    }
}

/// api_error_status 是否可重试（限流/过载/服务端错误 —— 等待后重跑有意义）
pub fn is_retryable_status(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
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
            r#"{"type":"result","result":"done","subtype":"success","model":"claude-sonnet-4","usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":60,"cache_creation_input_tokens":10}}"#,
        ]));
        assert_eq!(out.session_id.as_deref(), Some("sid-1"));
        assert_eq!(out.answer, "done");
        assert_eq!(out.usage_in, 100);
        assert_eq!(out.usage_out, 20);
        assert_eq!(out.cache_read, Some(60));
        assert_eq!(out.cache_creation, Some(10));
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

    /// 结构化错误（R28）：errors + api_error_status —— 优于解析 stderr
    #[test]
    fn parses_structured_errors() {
        let out = parse_stream_json(&stream(&[
            r#"{"type":"system","session_id":"s"}"#,
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["API Error: 529 overloaded_error"],"api_error_status":529,"usage":{"input_tokens":1,"output_tokens":0}}"#,
        ]));
        assert!(out.is_error);
        assert_eq!(out.errors, vec!["API Error: 529 overloaded_error"]);
        assert_eq!(out.api_error_status, Some(529));
        assert!(is_retryable_status(529));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(200));
    }

    /// total_cost_usd（R34 对账素材）：CLI 自报费用；缺省 None
    #[test]
    fn parses_total_cost_usd() {
        let out = parse_stream_json(&stream(&[
            r#"{"type":"result","result":"ok","total_cost_usd":0.0123,"usage":{"input_tokens":10,"output_tokens":5}}"#,
        ]));
        assert!((out.total_cost_usd.unwrap() - 0.0123).abs() < 1e-9);
        let out = parse_stream_json(&stream(&[
            r#"{"type":"result","result":"ok","usage":{"input_tokens":10,"output_tokens":5}}"#,
        ]));
        assert_eq!(out.total_cost_usd, None);
    }

    /// 未识别事件类型忽略（前向兼容：内容不解析，但类型记录进签名）
    #[test]
    fn unknown_events_ignored() {
        let out = parse_stream_json(&stream(&[
            r#"{"type":"future_event","payload":123}"#,
            "not json at all",
            "",
        ]));
        assert_eq!(out.event_types, vec!["future_event"], "未知类型记录签名");
        // 其余字段全默认
        assert_eq!(out.session_id, None);
        assert_eq!(out.answer, "");
        assert_eq!(out.usage_in, 0);
        assert_eq!(out.tools_used, Vec::<String>::new());
    }

    /// schema 漂移检测（R35）：三要素（续接/回答/计量）缺失逐项报
    #[test]
    fn detects_schema_drift() {
        // 完整流 → 无漂移
        let ok = parse_stream_json(&stream(&[
            r#"{"type":"system","session_id":"s"}"#,
            r#"{"type":"result","result":"done","usage":{"input_tokens":10,"output_tokens":2}}"#,
        ]));
        assert!(schema_drift(&ok).is_empty(), "{:?}", schema_drift(&ok));

        // system 缺 session_id
        let d = parse_stream_json(&stream(&[
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"result","result":"x","usage":{"input_tokens":1,"output_tokens":1}}"#,
        ]));
        assert_eq!(
            schema_drift(&d),
            vec!["system 事件缺 session_id 字段（CLI schema 变化？）"]
        );

        // result 缺 usage（计量闭环破坏）
        let d = parse_stream_json(&stream(&[
            r#"{"type":"system","session_id":"s"}"#,
            r#"{"type":"result","result":"x"}"#,
        ]));
        assert_eq!(
            schema_drift(&d),
            vec!["result 事件缺 usage（计量闭环破坏）"]
        );

        // 缺 result 事件 + 缺 system 事件（双缺失全报）
        let d = parse_stream_json(&stream(&[r#"{"type":"assistant","message":{}}"#]));
        let drifts = schema_drift(&d);
        assert_eq!(drifts.len(), 2, "{drifts:?}");

        // 未知新事件类型不算漂移（前向兼容）
        let d = parse_stream_json(&stream(&[
            r#"{"type":"future_event","payload":1}"#,
            r#"{"type":"system","session_id":"s"}"#,
            r#"{"type":"result","result":"x","usage":{"input_tokens":1,"output_tokens":0}}"#,
        ]));
        assert!(schema_drift(&d).is_empty());
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

    /// Amp 方言（R41，B7）：首轮 -x 位置参数；续接子命令形态；
    /// 事件格式 Claude 兼容 → parse_round 与 claude 同解析
    #[test]
    fn amp_dialect_args() {
        let d = AmpDialect;
        let args = d.round_args("做点事", None);
        assert_eq!(
            args,
            vec![
                "-x".to_string(),
                "做点事".to_string(),
                "--stream-json".to_string()
            ],
            "首轮：amp -x <prompt> --stream-json"
        );
        let args = d.round_args("继续", Some("T-abc-123"));
        assert_eq!(
            args,
            vec![
                "threads".to_string(),
                "continue".to_string(),
                "T-abc-123".to_string(),
                "-x".to_string(),
                "继续".to_string(),
                "--stream-json".to_string(),
            ],
            "续接：amp threads continue <tid> -x <prompt> --stream-json"
        );
        // 事件格式 Claude 兼容（R2 调研）→ 默认 parse_round 输出一致
        let stdout = stream(&[
            r#"{"type":"system","session_id":"sid-1"}"#,
            r#"{"type":"result","result":"ok","usage":{"input_tokens":1,"output_tokens":1}}"#,
        ]);
        assert_eq!(d.parse_round(&stdout), ClaudeDialect.parse_round(&stdout));
    }

    /// 方言注册表：amp 注册、未知回落 claude
    #[test]
    fn dialect_fallback() {
        assert_eq!(dialect_by_name("claude").name(), "claude");
        assert_eq!(dialect_by_name("").name(), "claude");
        assert_eq!(dialect_by_name("amp").name(), "amp");
        assert_eq!(dialect_by_name("codex-future").name(), "claude");
    }
}
