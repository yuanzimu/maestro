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
///
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
/// ④错误分类（classify_exit 共用，退出码差异经 accepts_exit 表达）
pub trait Dialect: Send + Sync {
    fn name(&self) -> &'static str;
    /// 一轮的参数：prompt 必达；resume 存在则续接会话
    fn round_args(&self, prompt: &str, resume: Option<&str>) -> Vec<String>;
    /// 解析一轮输出（默认 = claude stream-json 格式；事件格式不兼容的
    /// 方言如 Gemini/Codex 覆写此方法）
    fn parse_round(&self, stdout: &[u8]) -> RoundOutcome {
        parse_stream_json(stdout)
    }
    /// 本方言视为「轮正常结束」的 CLI 退出码（默认仅 0）。
    /// 例：Gemini 53 = 轮次上限（对应 claude 的 error_max_turns ——
    /// 轮循环继续，不算任务失败）
    fn accepts_exit(&self, _code: Option<i32>) -> bool {
        false
    }
    /// schema 漂移检测（方言各自的要素清单；默认 = claude 三要素）
    fn schema_drift(&self, out: &RoundOutcome) -> Vec<String> {
        schema_drift(out)
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

/// Codex CLI（OpenAI）方言（R51，R2/R50 调研落地）：
/// `codex exec --json <prompt>`（JSONL）；续接 `codex exec resume <tid>`。
/// 事件（developers.openai.com/codex/noninteractive）：
/// - thread.started.thread_id → session（续接凭据）
/// - item.completed(item.type=agent_message).text → 回答（聚合；turn.completed 无文本）
/// - item.*(command_execution/file_change/mcp_tool_call/web_search/todo_list) → 工具
/// - turn.completed.usage → 计量；turn.failed / 顶层 error → 错误
///
/// ⚠️ usage 口径差异：**input_tokens 已含 cached**（cached 是子集非加数）——
/// 解析侧拆桶（in = input - cached），保证轮转检测的三桶合计不双计。
/// ⚠️ 参数顺序与 cache_write 字段待实测校准（版本演进字段）
pub struct CodexDialect;

/// Codex item.* 里代表工具调用的事件名 → 工具名（U3 叙事口径）
const CODEX_TOOL_ITEMS: &[&str] = &[
    "command_execution",
    "file_change",
    "mcp_tool_call",
    "web_search",
    "todo_list",
];

impl Dialect for CodexDialect {
    fn name(&self) -> &'static str {
        "codex"
    }
    fn round_args(&self, prompt: &str, resume: Option<&str>) -> Vec<String> {
        match resume {
            Some(tid) => vec![
                "exec".into(),
                "resume".into(),
                tid.into(),
                "--json".into(),
                prompt.into(),
            ],
            None => vec!["exec".into(), "--json".into(), prompt.into()],
        }
    }
    fn parse_round(&self, stdout: &[u8]) -> RoundOutcome {
        let mut out = RoundOutcome::default();
        for line in stdout.lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(t) = v["type"].as_str() else { continue };
            if !out.event_types.iter().any(|e| e == t) {
                out.event_types.push(t.to_string());
            }
            match t {
                "thread.started" => {
                    if let Some(tid) = v["thread_id"].as_str() {
                        out.session_id = Some(tid.to_string());
                    }
                }
                "item.started" | "item.updated" | "item.completed" => {
                    let item = &v["item"];
                    if let Some(kind) = item["type"].as_str() {
                        if CODEX_TOOL_ITEMS.contains(&kind)
                            && !out.tools_used.iter().any(|x| x == kind)
                        {
                            out.tools_used.push(kind.to_string());
                        }
                        // 回答 = agent_message 文本聚合（completed 才有完整 text）
                        if t == "item.completed" && kind == "agent_message" {
                            if let Some(text) = item["text"].as_str() {
                                out.answer.push_str(text);
                            }
                        }
                    }
                }
                "turn.completed" => {
                    let u = &v["usage"];
                    let input = u["input_tokens"].as_u64().unwrap_or(0);
                    // 拆桶（口径差）：codex 的 input_tokens 是总量（已含 cached
                    // 命中与 cache 写入 —— cached 是子集）—— 拆回互斥三桶，
                    // 保证轮转检测的三桶合计 = 真实 context 占用（不双计）
                    let cached = u["cached_input_tokens"].as_u64();
                    let write = u["cache_write_input_tokens"].as_u64();
                    out.cache_read = cached;
                    out.cache_creation = write;
                    out.usage_in = input
                        .saturating_sub(cached.unwrap_or(0))
                        .saturating_sub(write.unwrap_or(0));
                    out.usage_out = u["output_tokens"].as_u64().unwrap_or(0);
                }
                "turn.failed" => {
                    out.is_error = true;
                    if let Some(m) = v["error"]["message"].as_str() {
                        out.errors.push(m.to_string());
                    }
                }
                "error" => {
                    out.is_error = true;
                    if let Some(m) = v["message"].as_str() {
                        out.errors.push(m.to_string());
                    }
                }
                _ => {}
            }
        }
        out
    }
    fn schema_drift(&self, out: &RoundOutcome) -> Vec<String> {
        let mut drifts = vec![];
        if !out.event_types.iter().any(|t| t == "thread.started") {
            drifts.push("缺 thread.started 事件（thread_id 续接凭据丢失）".into());
        } else if out.session_id.is_none() {
            drifts.push("thread.started 缺 thread_id 字段（CLI schema 变化？）".into());
        }
        if !out.event_types.iter().any(|t| t == "turn.completed") {
            drifts.push("缺 turn.completed 事件（usage 计量丢失）".into());
        } else if out.usage_in == 0 && out.usage_out == 0 && out.cache_read.unwrap_or(0) == 0 {
            drifts.push("turn.completed 缺 usage（计量闭环破坏）".into());
        }
        if out.answer.is_empty() {
            drifts.push("无 agent_message 文本（回答捕获失效 —— DONE 信号检测源）".into());
        }
        drifts
    }
}

/// Gemini CLI 方言（R51，R2/R50 调研落地）：
/// `gemini -p <prompt> --output-format stream-json`；续接 `--resume <uuid>`
/// （会话按项目目录哈希隔离，续接须同 cwd —— daemon 的 workdir 语义天然满足）。
/// 事件（gemini-cli headless 文档）：
/// - init.session_id → session；init.model → 模型
/// - message(role=assistant).content → 回答（delta 增量拼接；非 delta 全文覆盖）
/// - tool_use.tool_name → 工具
/// - result.stats → 计量（cached 仅有合并值 → cache_read；无读/写细分）
/// - result.status=error / error 事件 → 错误
///
/// 退出码：0 成功；1 一般错；42 输入错误（Fatal）；**53 轮次上限**
/// （= claude error_max_turns 语义：轮循环继续，不算失败）
/// ⚠️ message delta/全文混合序列与 53 时 stdout 完整性待实测校准
pub struct GeminiDialect;

impl Dialect for GeminiDialect {
    fn name(&self) -> &'static str {
        "gemini"
    }
    fn round_args(&self, prompt: &str, resume: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "-p".into(),
            prompt.into(),
            "--output-format".into(),
            "stream-json".into(),
        ];
        if let Some(sid) = resume {
            args.push("--resume".into());
            args.push(sid.into());
        }
        args
    }
    fn accepts_exit(&self, code: Option<i32>) -> bool {
        // 53 = 轮次上限：本轮事件流照常解析，轮循环继续（对应 claude 的
        // error_max_turns 正常出口）；42 输入错误不在此列 → 透传失败
        code == Some(53)
    }
    fn parse_round(&self, stdout: &[u8]) -> RoundOutcome {
        let mut out = RoundOutcome::default();
        for line in stdout.lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(t) = v["type"].as_str() else { continue };
            if !out.event_types.iter().any(|e| e == t) {
                out.event_types.push(t.to_string());
            }
            match t {
                "init" => {
                    if let Some(s) = v["session_id"].as_str() {
                        out.session_id = Some(s.to_string());
                    }
                    if let Some(m) = v["model"].as_str() {
                        out.model = Some(m.to_string());
                    }
                }
                "message" => {
                    if v["role"] == "assistant" {
                        if let Some(c) = v["content"].as_str() {
                            if v["delta"].as_bool().unwrap_or(false) {
                                out.answer.push_str(c);
                            } else {
                                // 全文消息：覆盖（防全文+增量混合时重复）
                                out.answer = c.to_string();
                            }
                        }
                    }
                }
                "tool_use" => {
                    if let Some(name) = v["tool_name"].as_str() {
                        if !out.tools_used.iter().any(|x| x == name) {
                            out.tools_used.push(name.to_string());
                        }
                    }
                }
                "error" => {
                    out.is_error = true;
                    if let Some(m) = v["message"].as_str() {
                        out.errors.push(m.to_string());
                    }
                }
                "result" => {
                    if v["status"] == "error" {
                        out.is_error = true;
                        if let Some(m) = v["error"]["message"].as_str() {
                            out.errors.push(m.to_string());
                        }
                    }
                    let s = &v["stats"];
                    out.usage_in = s["input_tokens"].as_u64().unwrap_or(0);
                    out.usage_out = s["output_tokens"].as_u64().unwrap_or(0);
                    // cached 仅合并值（无读/写细分）→ 全记 cache_read
                    out.cache_read = s["cached"].as_u64();
                }
                _ => {}
            }
        }
        out
    }
    fn schema_drift(&self, out: &RoundOutcome) -> Vec<String> {
        let mut drifts = vec![];
        if !out.event_types.iter().any(|t| t == "init") {
            drifts.push("缺 init 事件（session_id 续接凭据丢失）".into());
        } else if out.session_id.is_none() {
            drifts.push("init 事件缺 session_id 字段（CLI schema 变化？）".into());
        }
        if !out.event_types.iter().any(|t| t == "result") {
            drifts.push("缺 result 事件（usage 计量丢失）".into());
        } else if out.usage_in == 0 && out.usage_out == 0 {
            drifts.push("result 事件缺 stats（计量闭环破坏）".into());
        }
        if out.answer.is_empty() {
            drifts.push("无 assistant message（回答捕获失效 —— DONE 信号检测源）".into());
        }
        drifts
    }
}

/// OpenCode（sst/opencode）方言（R52，R2/R50 调研落地）：
/// `opencode run --format json <prompt>`（NDJSON）；续接 `-s <sessionID>`
/// （`-c` = 最近会话）。事件（opencode.ai/docs/cli）：
/// - **每行都带 sessionID**（驼峰；无 init 事件）→ session
/// - text.part.text → 回答（聚合）；tool_use.part.tool → 工具
/// - step_finish.part.{tokens,cost} → 计量 + **自报费用（USD）** ——
///   四方言中唯一带 cost 字段的（对账素材天然存在）
/// - error.error.data.message → 错误
///
/// ⚠️ 无官方 schema 版本承诺：解析对未知 type 跳行容错（与 claude 同策略）；
/// tokens 的 cache 细分字段未稳定 → cache 桶留空（轮转检测按 in 桶低估，
/// 待实测校准后补）
pub struct OpenCodeDialect;

impl Dialect for OpenCodeDialect {
    fn name(&self) -> &'static str {
        "opencode"
    }
    fn round_args(&self, prompt: &str, resume: Option<&str>) -> Vec<String> {
        let mut args = vec!["run".into()];
        if let Some(sid) = resume {
            args.extend(["-s".into(), sid.into()]);
        }
        args.extend(["--format".into(), "json".into(), prompt.into()]);
        args
    }
    fn parse_round(&self, stdout: &[u8]) -> RoundOutcome {
        let mut out = RoundOutcome::default();
        for line in stdout.lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(t) = v["type"].as_str() else { continue };
            if !out.event_types.iter().any(|e| e == t) {
                out.event_types.push(t.to_string());
            }
            // sessionID 每行都带（无 init 事件）；以首见为准
            if out.session_id.is_none() {
                if let Some(s) = v["sessionID"].as_str() {
                    out.session_id = Some(s.to_string());
                }
            }
            match t {
                "text" => {
                    if let Some(txt) = v["part"]["text"].as_str() {
                        out.answer.push_str(txt);
                    }
                }
                "tool_use" => {
                    if let Some(name) = v["part"]["tool"].as_str() {
                        if !out.tools_used.iter().any(|x| x == name) {
                            out.tools_used.push(name.to_string());
                        }
                    }
                }
                "step_finish" => {
                    let p = &v["part"];
                    out.usage_in = p["tokens"]["input"].as_u64().unwrap_or(0);
                    out.usage_out = p["tokens"]["output"].as_u64().unwrap_or(0);
                    // 唯一带自报费用的 CLI（USD → 对账素材）
                    out.total_cost_usd = p["cost"].as_f64();
                }
                "error" => {
                    out.is_error = true;
                    if let Some(m) = v["error"]["data"]["message"].as_str() {
                        out.errors.push(m.to_string());
                    }
                }
                _ => {}
            }
        }
        out
    }
    fn schema_drift(&self, out: &RoundOutcome) -> Vec<String> {
        let mut drifts = vec![];
        if out.session_id.is_none() {
            drifts.push("事件流无 sessionID（续接凭据丢失 —— OpenCode 每行都应带）".into());
        }
        if !out.event_types.iter().any(|t| t == "step_finish") {
            drifts.push("缺 step_finish 事件（usage 计量丢失）".into());
        } else if out.usage_in == 0 && out.usage_out == 0 {
            drifts.push("step_finish 缺 tokens（计量闭环破坏）".into());
        }
        if out.answer.is_empty() {
            drifts.push("无 text 事件（回答捕获失效 —— DONE 信号检测源）".into());
        }
        drifts
    }
}

/// 按名取方言（MAESTRO_CLI_DIALECT；未知名回落 claude）
pub fn dialect_by_name(name: &str) -> Box<dyn Dialect> {
    match name {
        "claude" | "" => Box::new(ClaudeDialect),
        "amp" => Box::new(AmpDialect),
        "codex" => Box::new(CodexDialect),
        "gemini" => Box::new(GeminiDialect),
        "opencode" => Box::new(OpenCodeDialect),
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
        assert_eq!(dialect_by_name("codex").name(), "codex");
        assert_eq!(dialect_by_name("gemini").name(), "gemini");
        assert_eq!(dialect_by_name("codex-future").name(), "claude");
    }

    /// Codex 方言（R51）：thread/item/turn 事件解析 + **拆桶口径**
    /// （input_tokens 已含 cached → in = input - cached，三桶合计不双计）
    #[test]
    fn codex_dialect_parses_and_splits_cache() {
        let d = CodexDialect;
        let stdout = stream(&[
            r#"{"type":"thread.started","thread_id":"th-42"}"#,
            r#"{"type":"item.started","item":{"id":"i1","type":"command_execution","command":"ls"}}"#,
            r#"{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"ls","exit_code":0}}"#,
            r#"{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"第一段"}}"#,
            r#"{"type":"item.completed","item":{"id":"i3","type":"agent_message","text":"第二段"}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":600,"cache_write_input_tokens":50,"output_tokens":80}}"#,
        ]);
        let oc = d.parse_round(&stdout);
        assert_eq!(oc.session_id.as_deref(), Some("th-42"));
        assert_eq!(oc.answer, "第一段第二段", "agent_message 文本聚合");
        assert_eq!(oc.tools_used, vec!["command_execution"]);
        // 拆桶：in = 1000-600-50（input 是总量，cached/cache_write 是子集）；
        // 三桶互斥、合计 = 1000（真实 context 占用）
        assert_eq!(oc.usage_in, 350);
        assert_eq!(oc.cache_read, Some(600));
        assert_eq!(oc.cache_creation, Some(50));
        assert_eq!(oc.usage_in + oc.cache_read.unwrap() + oc.cache_creation.unwrap(), 1000);
        assert_eq!(oc.usage_out, 80);
        assert!(d.schema_drift(&oc).is_empty(), "要素齐全: {:?}", d.schema_drift(&oc));
        // 退出码：codex 无特殊正常码（默认仅 0）
        assert!(!d.accepts_exit(Some(53)));
    }

    /// Codex 缺 cached 字段（版本演进容错）：整体进 in 桶
    #[test]
    fn codex_dialect_missing_cached_field() {
        let d = CodexDialect;
        let stdout = stream(&[
            r#"{"type":"thread.started","thread_id":"th-1"}"#,
            r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"ok"}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":700,"output_tokens":30}}"#,
        ]);
        let oc = d.parse_round(&stdout);
        assert_eq!(oc.usage_in, 700);
        assert_eq!(oc.cache_read, None);
    }

    /// Codex turn.failed / 顶层 error → 结构化错误
    #[test]
    fn codex_dialect_errors() {
        let d = CodexDialect;
        let stdout = stream(&[
            r#"{"type":"thread.started","thread_id":"th-1"}"#,
            r#"{"type":"turn.failed","error":{"message":"upstream 500"}}"#,
        ]);
        let oc = d.parse_round(&stdout);
        assert!(oc.is_error);
        assert_eq!(oc.errors, vec!["upstream 500".to_string()]);
    }

    /// Codex 参数构造：exec 形态 + resume 子命令形态
    #[test]
    fn codex_dialect_args() {
        let d = CodexDialect;
        assert_eq!(
            d.round_args("做点事", None),
            vec!["exec".to_string(), "--json".to_string(), "做点事".to_string()]
        );
        assert_eq!(
            d.round_args("继续", Some("th-9")),
            vec![
                "exec".to_string(),
                "resume".to_string(),
                "th-9".to_string(),
                "--json".to_string(),
                "继续".to_string(),
            ]
        );
    }

    /// Gemini 方言（R51）：init/message/tool_use/result 解析 + delta 拼接
    #[test]
    fn gemini_dialect_parses_stream() {
        let d = GeminiDialect;
        let stdout = stream(&[
            r#"{"type":"init","session_id":"g-1","model":"gemini-2.5-pro"}"#,
            r#"{"type":"message","role":"user","content":"任务"}"#,
            r#"{"type":"message","role":"assistant","content":"正在","delta":true}"#,
            r#"{"type":"message","role":"assistant","content":"处理","delta":true}"#,
            r#"{"type":"tool_use","tool_name":"read_file","tool_id":"t1"}"#,
            r#"{"type":"result","status":"success","stats":{"input_tokens":200,"output_tokens":40,"cached":120}}"#,
        ]);
        let oc = d.parse_round(&stdout);
        assert_eq!(oc.session_id.as_deref(), Some("g-1"));
        assert_eq!(oc.model.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(oc.answer, "正在处理", "delta 增量拼接");
        assert_eq!(oc.tools_used, vec!["read_file"]);
        assert_eq!(oc.usage_in, 200);
        assert_eq!(oc.usage_out, 40);
        assert_eq!(oc.cache_read, Some(120), "cached 合并值 → cache_read");
        assert!(!oc.is_error);
        assert!(d.schema_drift(&oc).is_empty());
    }

    /// Gemini 全文消息覆盖语义（防全文+增量混合重复）；result.status=error
    #[test]
    fn gemini_dialect_full_message_and_error() {
        let d = GeminiDialect;
        let stdout = stream(&[
            r#"{"type":"init","session_id":"g-1"}"#,
            r#"{"type":"message","role":"assistant","content":"完整回答"}"#,
            r#"{"type":"result","status":"error","error":{"type":"api","message":"quota exceeded"},"stats":{"input_tokens":10,"output_tokens":1}}"#,
        ]);
        let oc = d.parse_round(&stdout);
        assert_eq!(oc.answer, "完整回答");
        assert!(oc.is_error);
        assert_eq!(oc.errors, vec!["quota exceeded".to_string()]);
    }

    /// Gemini 退出码：53（轮次上限）= 正常轮出口；42/1 不接受
    #[test]
    fn gemini_dialect_exit_codes() {
        let d = GeminiDialect;
        assert!(d.accepts_exit(Some(53)), "53 = 轮次上限，轮循环继续");
        assert!(!d.accepts_exit(Some(0)), "0 走 success 路径，无需特判");
        assert!(!d.accepts_exit(Some(42)), "42 = 输入错误 → 失败");
        assert!(!d.accepts_exit(Some(1)));
        assert_eq!(
            d.round_args("继续", Some("g-7")),
            vec![
                "-p".to_string(),
                "继续".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--resume".to_string(),
                "g-7".to_string(),
            ]
        );
    }

    /// 方言各自的三要素漂移（claude 格式喂 gemini 方言 → 报缺 init）
    #[test]
    fn gemini_dialect_drift_on_claude_events() {
        let d = GeminiDialect;
        let stdout = stream(&[
            r#"{"type":"system","session_id":"s1"}"#,
            r#"{"type":"result","result":"ok","usage":{"input_tokens":1,"output_tokens":1}}"#,
        ]);
        let oc = d.parse_round(&stdout);
        let drifts = d.schema_drift(&oc);
        assert!(
            drifts.iter().any(|x| x.contains("init")),
            "claude 事件流对 gemini 方言应报缺 init: {drifts:?}"
        );
    }

    /// OpenCode 方言（R52）：sessionID 每行都带、text 聚合、step_finish
    /// 计量 + **自报 cost（USD）**（四方言唯一）
    #[test]
    fn opencode_dialect_parses_ndjson() {
        let d = OpenCodeDialect;
        let stdout = stream(&[
            r#"{"type":"step_start","sessionID":"ses-1","timestamp":1}"#,
            r#"{"type":"text","sessionID":"ses-1","part":{"text":"正在执行"}}"#,
            r#"{"type":"tool_use","sessionID":"ses-1","part":{"tool":"read","callID":"c1"}}"#,
            r#"{"type":"text","sessionID":"ses-1","part":{"text":"，完成"}}"#,
            r#"{"type":"step_finish","sessionID":"ses-1","part":{"reason":"stop","cost":0.0123,"tokens":{"input":200,"output":40}}}"#,
        ]);
        let oc = d.parse_round(&stdout);
        assert_eq!(oc.session_id.as_deref(), Some("ses-1"), "每行 sessionID，首见为准");
        assert_eq!(oc.answer, "正在执行，完成", "text 事件聚合");
        assert_eq!(oc.tools_used, vec!["read"]);
        assert_eq!(oc.usage_in, 200);
        assert_eq!(oc.usage_out, 40);
        assert_eq!(oc.total_cost_usd, Some(0.0123), "唯一自报费用的方言");
        assert!(d.schema_drift(&oc).is_empty());
    }

    /// OpenCode error 事件 + 参数构造（run -s 续接）
    #[test]
    fn opencode_dialect_error_and_args() {
        let d = OpenCodeDialect;
        let stdout = stream(&[
            r#"{"type":"step_start","sessionID":"ses-1"}"#,
            r#"{"type":"error","sessionID":"ses-1","error":{"data":{"message":"provider down"}}}"#,
        ]);
        let oc = d.parse_round(&stdout);
        assert!(oc.is_error);
        assert_eq!(oc.errors, vec!["provider down".to_string()]);

        assert_eq!(
            d.round_args("做点事", None),
            vec!["run".to_string(), "--format".to_string(), "json".to_string(), "做点事".to_string()]
        );
        assert_eq!(
            d.round_args("继续", Some("ses-9")),
            vec![
                "run".to_string(),
                "-s".to_string(),
                "ses-9".to_string(),
                "--format".to_string(),
                "json".to_string(),
                "继续".to_string(),
            ]
        );
    }

    /// OpenCode 漂移：无 sessionID / 缺 step_finish 的人话诊断
    #[test]
    fn opencode_dialect_drift() {
        let d = OpenCodeDialect;
        let stdout = stream(&[
            r#"{"type":"text","sessionID":"ses-1","part":{"text":"ok"}}"#,
        ]);
        let oc = d.parse_round(&stdout);
        let drifts = d.schema_drift(&oc);
        assert!(drifts.iter().any(|x| x.contains("step_finish")), "{drifts:?}");
    }
}
