//! 上游模型网关对接（DEV_PLAN v2.5 / Sprint A，决策 23）。
//!
//! Maestro 不自研模型路由：把 worker（Claude Code 等）与 daemon 自身的
//! LLM 调用统一指向本地 [CCR](https://github.com/musistudio/claude-code-router)
//! （Claude Code Router，MIT），由 CCR 负责模型分级路由、key 池轮换、
//! provider fallback 与成本核算。
//!
//! CCR 是本地 HTTP 网关，默认 `http://127.0.0.1:3456`，同端口提供
//! Anthropic `/v1/messages` 与 OpenAI `/v1/chat/completions` 端点。
//!
//! 链路：daemon 经环境变量注入 → rounder（不清环境）→ 内层 agent CLI 继承。

use crate::llm::OpenAiClient;

/// CCR 默认地址
pub const DEFAULT_CCR_URL: &str = "http://127.0.0.1:3456";
/// CCR 默认本地鉴权令牌占位（真 key 在 CCR Providers 内）
pub const DEFAULT_CCR_TOKEN: &str = "dummy";

/// 网关配置
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayConfig {
    /// CCR 地址。空串 = 不启用（直连 provider，不注入网关环境）
    pub url: String,
    /// 与本地网关通话的鉴权令牌（ANTHROPIC_AUTH_TOKEN / 出站 Bearer）
    pub token: String,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_CCR_URL.into(),
            token: DEFAULT_CCR_TOKEN.into(),
        }
    }
}

impl GatewayConfig {
    /// 是否启用网关
    pub fn enabled(&self) -> bool {
        !self.url.trim().is_empty()
    }

    /// 从环境构造：
    /// - `MAESTRO_CCR_URL`：显式设为空串即关闭网关；缺省 = 默认 CCR 地址
    /// - `MAESTRO_CCR_TOKEN`：缺省 = dummy 占位
    pub fn from_env() -> Self {
        let url = match std::env::var("MAESTRO_CCR_URL") {
            Ok(u) => u.trim_end_matches('/').to_string(),
            Err(_) => DEFAULT_CCR_URL.into(),
        };
        let token = std::env::var("MAESTRO_CCR_TOKEN").unwrap_or_else(|_| DEFAULT_CCR_TOKEN.into());
        Self { url, token }
    }

    /// 注入给 worker 的环境变量（Claude Code 走 Anthropic 端点）。
    /// 网关关闭时返回空 Vec（worker 维持各自 provider 直连配置）。
    pub fn worker_env(&self) -> Vec<(String, String)> {
        if !self.enabled() {
            return Vec::new();
        }
        vec![
            ("ANTHROPIC_BASE_URL".into(), self.url.clone()),
            ("ANTHROPIC_AUTH_TOKEN".into(), self.token.clone()),
            // 兼容读取 OPENAI 风格 base 的 agent：CCR 同端口提供 OpenAI 端点
            ("OPENAI_BASE_URL".into(), self.url.clone()),
            ("OPENAI_API_KEY".into(), self.token.clone()),
        ]
    }

    /// 构造指向网关的 daemon 自有 LLM client（U3 叙事 / U8 预估 / T1 分类共用）。
    /// 网关关闭时回退到 `OpenAiClient::from_env`（直连，读 OPENAI_API_KEY）。
    pub fn build_llm_client(&self) -> Option<OpenAiClient> {
        if self.enabled() {
            Some(OpenAiClient::new(&self.url, &self.token))
        } else {
            OpenAiClient::from_env()
        }
    }
}

/// 网关探活：GET `{url}/` 期望任意 HTTP 应答（含 4xx/5xx —— 证明端口在听）。
/// 仅传输层失败（拒绝连接 / 超时）才判定离线。阻塞式，短超时，由非权威线程调用。
pub fn is_alive(url: &str) -> bool {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(2))
        .build();
    match agent.get(url).call() {
        Ok(_resp) => true,
        // 收到 HTTP 错误状态码：端口在监听，仍视为存活
        Err(ureq::Error::Status(_, _)) => true,
        // 传输层错误（拒绝连接/超时/DNS）：离线
        Err(ureq::Error::Transport(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled() {
        let g = GatewayConfig::default();
        assert!(g.enabled());
        assert_eq!(g.url, DEFAULT_CCR_URL);
    }

    #[test]
    fn empty_url_disables() {
        let g = GatewayConfig {
            url: String::new(),
            token: "x".into(),
        };
        assert!(!g.enabled());
        assert!(g.worker_env().is_empty(), "关闭时不注入环境");
    }

    #[test]
    fn worker_env_points_to_gateway() {
        let g = GatewayConfig::default();
        let env = g.worker_env();
        let get = |k: &str| {
            env.iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("ANTHROPIC_BASE_URL"), Some(DEFAULT_CCR_URL));
        assert_eq!(get("ANTHROPIC_AUTH_TOKEN"), Some(DEFAULT_CCR_TOKEN));
        assert_eq!(get("OPENAI_BASE_URL"), Some(DEFAULT_CCR_URL));
    }

    #[test]
    fn builds_client_against_gateway() {
        // 启用：必然产出 client（不发起调用）
        assert!(GatewayConfig::default().build_llm_client().is_some());
        // 关闭且无 OPENAI_API_KEY：None
        let g = GatewayConfig {
            url: String::new(),
            token: "x".into(),
        };
        // 该断言依赖测试环境无 OPENAI_API_KEY；为避免环境敏感，仅校验不 panic
        let _ = g.build_llm_client();
    }

    #[test]
    fn alive_detects_dead_port() {
        // 绑定一个空闲端口再立即释放 → 确定无监听；拒绝连接立即返回不 flaky
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(!is_alive(&format!("http://127.0.0.1:{port}")));
    }
}
