//! daemon 内置 LLM client（DEV_PLAN 0.14 骨架）：OpenAI 兼容 chat API +
//! 计价表 + 账本条目。
//!
//! 用途：多轮驱动 Worker（0.15）的模型调用、T6 三栏比价的成本估算。
//! 阻塞式（ureq），由 Core 的 worker 线程调用 —— 不进 Core 主循环
//! （网络 IO 永远不在权威线程上做）。
//!
//! 计价精度：内部用「微美分」（1e-6 美元）累计，账本条目向上取整到整
//! 美分（UsageEntry.actual_cost_cents 为 u64）。批量小调用的亚美分成本
//! 在 T6 周期汇总时才进对账（P1）。

use maestro_protocol::events::UsageEntry;
use thiserror::Error;

/// LLM 调用错误
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("network: {0}")]
    Network(String),
    #[error("http {0}: {1}")]
    Http(u16, String),
    #[error("parse: {0}")]
    Parse(String),
}

/// 一次补全请求
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub system: Option<String>,
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
}

/// 一次补全结果
#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// LLM client 抽象（测试可注入 fake）
pub trait LlmClient: Send + Sync {
    fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, LlmError>;
}

/// OpenAI 兼容 client（base_url 形如 https://api.openai.com 或本地网关）
pub struct OpenAiClient {
    base_url: String,
    api_key: String,
    agent: ureq::Agent,
}

impl OpenAiClient {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(300))
                .build(),
        }
    }

    /// 从环境构造：MAESTRO_LLM_BASE_URL（默认 OpenAI）+ OPENAI_API_KEY
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("OPENAI_API_KEY").ok()?;
        let base = std::env::var("MAESTRO_LLM_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com".into());
        Some(Self::new(&base, &key))
    }
}

impl LlmClient for OpenAiClient {
    fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let mut messages = vec![];
        if let Some(sys) = &req.system {
            messages.push(serde_json::json!({ "role": "system", "content": sys }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": req.prompt }));
        let mut body = serde_json::json!({
            "model": req.model,
            "messages": messages,
        });
        if let Some(m) = req.max_tokens {
            body["max_tokens"] = serde_json::json!(m);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = serde_json::json!(t);
        }

        let resp = self
            .agent
            .post(&format!("{}/v1/chat/completions", self.base_url))
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .send_json(body)
            .map_err(|e| match e {
                ureq::Error::Status(code, r) => LlmError::Http(
                    code,
                    r.into_string()
                        .unwrap_or_default()
                        .chars()
                        .take(300)
                        .collect(),
                ),
                other => LlmError::Network(other.to_string()),
            })?;
        let v: serde_json::Value = resp
            .into_json()
            .map_err(|e| LlmError::Parse(e.to_string()))?;
        let text = v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| LlmError::Parse("missing choices[0].message.content".into()))?
            .to_string();
        let input_tokens = v["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        let output_tokens = v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
        Ok(CompletionResponse {
            text,
            input_tokens,
            output_tokens,
        })
    }
}

// ---------------------------------------------------------------------------
// 计价表（v0：常见模型牌价，美分/百万 token；T6 比价与账本共用）
// ---------------------------------------------------------------------------

/// 模型牌价（美分 / 百万 token）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input_per_m: u64,
    pub output_per_m: u64,
    /// cache 命中读价（美分/百万）——Anthropic 0.1x、OpenAI 0.25~0.5x 输入价
    pub cache_read_per_m: u64,
    /// 写 cache 价（美分/百万）：写入调用按此计费 —— OpenAI = 输入价
    /// （不额外加价），Anthropic = 1.25x（5min TTL）/2x（1h）输入价
    pub cache_write_per_m: u64,
}

/// 已知牌价表（2024-2025 公开牌价快照；T6 上线前接 provider API 校准）。
/// ⚠️ 写 cache 价语义：写入调用本身按该价计费 —— OpenAI = 输入价
/// （不额外加价），Anthropic = 1.25x（5min TTL）/2x（1h）输入价。
pub fn price_of(model: &str) -> Option<ModelPrice> {
    let p = match model {
        "gpt-4o" => ModelPrice {
            input_per_m: 250,
            output_per_m: 1000,
            cache_read_per_m: 125,
            // 写入不额外加价（按输入价）
            cache_write_per_m: 250,
        },
        "gpt-4o-mini" => ModelPrice {
            input_per_m: 15,
            output_per_m: 60,
            cache_read_per_m: 8,
            cache_write_per_m: 15,
        },
        "o3-mini" => ModelPrice {
            input_per_m: 110,
            output_per_m: 440,
            cache_read_per_m: 55,
            cache_write_per_m: 110,
        },
        "claude-sonnet-4" => ModelPrice {
            input_per_m: 300,
            output_per_m: 1500,
            cache_read_per_m: 30,
            // 1.25x 输入价（5min TTL 档）
            cache_write_per_m: 375,
        },
        "claude-haiku-3-5" => ModelPrice {
            input_per_m: 80,
            output_per_m: 400,
            cache_read_per_m: 8,
            cache_write_per_m: 100,
        },
        _ => return None,
    };
    Some(p)
}

/// 成本（微美分，1e-6 美分）：精确整数运算。
/// 牌价单位是「美分/百万 token」，直接乘 token 数恰得微美分。
pub fn cost_micro_cents(model: &str, input_tokens: u64, output_tokens: u64) -> Option<u64> {
    let p = price_of(model)?;
    let in_mc = p.input_per_m.checked_mul(input_tokens)?;
    let out_mc = p.output_per_m.checked_mul(output_tokens)?;
    Some(in_mc + out_mc)
}

/// 成本（整美分，向上取整 —— 账本条目精度）
pub fn cost_cents_ceil(model: &str, input_tokens: u64, output_tokens: u64) -> Option<u64> {
    let mc = cost_micro_cents(model, input_tokens, output_tokens)?;
    Some(mc.div_ceil(1_000_000))
}

/// 生成账本条目（T6 比价/预算引擎共用的口径）
pub fn usage_entry(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    path: &str,
    discount: f64,
) -> UsageEntry {
    let list_mc = cost_micro_cents(model, input_tokens, output_tokens);
    let actual_mc = list_mc.map(|mc| (mc as f64 * discount) as u64);
    let to_cents = |mc: Option<u64>| mc.map(|x| x.div_ceil(1_000_000));
    UsageEntry {
        input_tokens,
        output_tokens,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        path: Some(path.to_string()),
        discount: Some(discount),
        counterfactual_cost_cents: to_cents(list_mc),
        actual_cost_cents: to_cents(actual_mc),
    }
}

/// 多轮驱动轮账的 cache 感知计价（R24 计价闭环，R28 补写 cache 档）：
/// - **actual** = 未缓存输入按输入价 + cache 命中按读价（0.1x~0.5x）+
///   写 cache 按写价（Anthropic 1.25x/2x；OpenAI = 输入价不加价）+ 输出价
/// - **counterfactual** = 同内容冷跑（session/cache 全失效）全部按输入价 ——
///   U8 省了多少的口径基线。⚠️ 读主导的轮 actual ≤ counterfactual；纯写入轮
///   可能反超（cache 前置投入，回报在后续读）—— 属真实计费语义，不是 bug
///
/// token 三桶（input/cache_read/cache_write）按 API 语义互斥；异常上报钳到 input。
/// 返回 None = 模型不在牌价表（条目仍入账但不计价，cents 留空）。
pub fn priced_usage_entry(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    path: &str,
) -> Option<UsageEntry> {
    let p = price_of(model)?;
    let cache_r = cache_read.unwrap_or(0).min(input_tokens);
    let cache_w = cache_write
        .unwrap_or(0)
        .min(input_tokens.saturating_sub(cache_r));
    let cold_in = input_tokens - cache_r - cache_w;
    let actual_mc = p
        .input_per_m
        .checked_mul(cold_in)?
        .checked_add(p.cache_read_per_m.checked_mul(cache_r)?)?
        .checked_add(p.cache_write_per_m.checked_mul(cache_w)?)?
        .checked_add(p.output_per_m.checked_mul(output_tokens)?)?;
    let cf_mc = p
        .input_per_m
        .checked_mul(input_tokens)?
        .checked_add(p.output_per_m.checked_mul(output_tokens)?)?;
    Some(UsageEntry {
        input_tokens,
        output_tokens,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_write,
        path: Some(path.to_string()),
        discount: None,
        counterfactual_cost_cents: Some(cf_mc.div_ceil(1_000_000)),
        actual_cost_cents: Some(actual_mc.div_ceil(1_000_000)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// 极简 OpenAI 假端点：单连接，吃 POST，回固定 JSON
    fn fake_openai(resp_body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let mut content_length = 0usize;
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    let l = line.trim().to_lowercase();
                    if let Some(v) = l.strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                    if line.trim().is_empty() {
                        break;
                    }
                    line.clear();
                }
                let mut body = vec![0u8; content_length];
                if content_length > 0 {
                    reader.read_exact(&mut body).unwrap();
                }
                let req: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let model = req["model"].as_str().unwrap().to_string();
                let mut w = stream;
                let payload = resp_body.replace("__MODEL__", &model);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = w.write_all(resp.as_bytes());
            }
        });
        addr
    }

    #[test]
    fn completes_against_fake_endpoint() {
        let addr = fake_openai(
            r#"{"choices":[{"message":{"role":"assistant","content":"hello!"}}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        );
        let client = OpenAiClient::new(&addr, "test-key");
        let resp = client
            .complete(&CompletionRequest {
                model: "gpt-4o-mini".into(),
                system: Some("be terse".into()),
                prompt: "say hi".into(),
                max_tokens: Some(8),
                temperature: None,
            })
            .unwrap();
        assert_eq!(resp.text, "hello!");
        assert_eq!(resp.input_tokens, 10);
        assert_eq!(resp.output_tokens, 5);
    }

    #[test]
    fn http_error_surfaced() {
        // 端点返回 401（先读完请求再响应，防 ureq 写请求体时 EPIPE）
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let mut content_length = 0usize;
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    let l = line.trim().to_lowercase();
                    if let Some(v) = l.strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                    if line.trim().is_empty() {
                        break;
                    }
                    line.clear();
                }
                if content_length > 0 {
                    let mut body = vec![0u8; content_length];
                    let _ = reader.read_exact(&mut body);
                }
                let mut stream = stream;
                let _ = stream.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\nno",
                );
                let _ = stream.flush();
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        let client = OpenAiClient::new(&addr, "bad-key");
        let err = client
            .complete(&CompletionRequest {
                model: "gpt-4o-mini".into(),
                system: None,
                prompt: "x".into(),
                max_tokens: None,
                temperature: None,
            })
            .unwrap_err();
        assert!(matches!(err, LlmError::Http(401, _)), "{err}");
    }

    #[test]
    fn pricing_math() {
        // gpt-4o：$2.50/1M in（=250 cents/1M）× 1M tokens = 250 cents
        assert_eq!(cost_micro_cents("gpt-4o", 1_000_000, 0), Some(250_000_000));
        assert_eq!(cost_cents_ceil("gpt-4o", 1_000_000, 0), Some(250));
        // 小调用：1000 token in = 0.25 cents → 微美分 250000 → 取整 1 cent
        assert_eq!(cost_micro_cents("gpt-4o", 1_000, 0), Some(250_000));
        assert_eq!(cost_cents_ceil("gpt-4o", 1_000, 0), Some(1));
        // 输出侧：$10/1M out（=1000 cents/1M）
        assert_eq!(cost_cents_ceil("gpt-4o", 0, 500_000), Some(500));
        // 未知模型
        assert_eq!(cost_micro_cents("mystery", 1, 1), None);
    }

    #[test]
    fn usage_entry_discount_math() {
        // batch 5 折：牌价 2 cents、实际 1 cent
        let e = usage_entry("gpt-4o", 800_000, 0, "batch", 0.5);
        assert_eq!(e.counterfactual_cost_cents, Some(200));
        assert_eq!(e.actual_cost_cents, Some(100));
        assert_eq!(e.path.as_deref(), Some("batch"));
    }

    #[test]
    fn priced_usage_entry_cache_math() {
        // claude-sonnet-4：1M in（800k 命中读 + 100k 写 cache）+ 100k out
        // actual = 300×0.1M + 30×0.8M + 375×0.1M + 1500×0.1M
        //        = 30M+24M+37.5M+150M = 241.5M 微美分 = 242 cents（向上取整）
        // counterfactual（冷跑）= 300×1M + 1500×0.1M = 450M 微美分 = 450 cents
        let e = priced_usage_entry(
            "claude-sonnet-4",
            1_000_000,
            100_000,
            Some(800_000),
            Some(100_000),
            "round",
        )
        .unwrap();
        assert_eq!(e.actual_cost_cents, Some(242));
        assert_eq!(e.counterfactual_cost_cents, Some(450));
        assert_eq!(e.cache_read_tokens, Some(800_000));
        assert_eq!(e.cache_creation_tokens, Some(100_000));
        // 实际必 ≤ 反事实（cache 只会省钱）
        assert!(e.actual_cost_cents.unwrap() <= e.counterfactual_cost_cents.unwrap());
    }

    #[test]
    fn priced_usage_entry_edges() {
        // cache_read > input（异常上报）：钳到 input，等价全命中读
        let e = priced_usage_entry("gpt-4o", 1_000_000, 0, Some(2_000_000), None, "round").unwrap();
        assert_eq!(e.actual_cost_cents, Some(125)); // 全按 cache 读价
                                                    // 无 cache：actual == counterfactual
        let e = priced_usage_entry("gpt-4o", 1_000_000, 0, None, None, "round").unwrap();
        assert_eq!(e.actual_cost_cents, e.counterfactual_cost_cents);
        // OpenAI 写 cache 不额外加价（按输入价）：cc 不改变 actual
        let a = priced_usage_entry("gpt-4o", 1_000_000, 0, None, Some(500_000), "round").unwrap();
        let b = priced_usage_entry("gpt-4o", 1_000_000, 0, None, None, "round").unwrap();
        assert_eq!(
            a.actual_cost_cents, b.actual_cost_cents,
            "gpt 写 cache 不应额外加价"
        );
        // 未知模型：None（调用方入账不计价）
        assert!(priced_usage_entry("mystery", 1, 1, None, None, "round").is_none());
    }
}
