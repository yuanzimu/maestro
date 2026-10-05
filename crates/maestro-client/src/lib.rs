//! Maestro Rust 客户端库：连接 API socket 的 JSON-RPC + 事件流订阅 + Web UI。

pub mod transport;
pub mod ui;

pub use transport::default_data_dir;

use maestro_protocol::api::{Method, Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use transport::Addr;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("连接失败（daemon 未启动？）: {0}")]
    Connect(String),
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("RPC 错误 {code}: {message}")]
    Rpc { code: i32, message: String },
}

#[derive(Clone)]
pub struct MaestroClient {
    api: Addr,
    events: Addr,
}

impl MaestroClient {
    pub fn new(data_dir: &Path) -> Self {
        let (api, events) = Addr::endpoints(data_dir);
        Self { api, events }
    }

    /// 从 ENV_SOCKET_PATH 值构造（多轮驱动 Worker 回连用）。
    /// Unix = socket 路径；Windows = "127.0.0.1:PORT"（见 transport）
    pub fn from_api_socket(api_env: &str) -> Self {
        let api = Addr::from_env_value(api_env);
        let events = api.sibling_events();
        Self { api, events }
    }

    pub fn connect_default() -> Self {
        Self::new(&default_data_dir())
    }

    /// daemon 是否在监听
    pub fn is_daemon_alive(&self) -> bool {
        self.api.connect().is_ok()
    }

    /// 发送一个请求（每次新建连接：简单可靠，CLI 场景足够）
    pub fn call(
        &self,
        id: &str,
        method: Method,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        let mut stream = self
            .api
            .connect()
            .map_err(|e| ClientError::Connect(e.to_string()))?;
        let req = Request {
            id: id.into(),
            method,
            params,
        };
        let line = serde_json::to_string(&req).map_err(|e| ClientError::Protocol(e.to_string()))?;
        stream
            .write_all(line.as_bytes())
            .and_then(|_| stream.write_all(b"\n"))
            .and_then(|_| stream.flush())
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        let mut reader = BufReader::new(stream);
        let mut resp_line = String::new();
        let n = reader
            .read_line(&mut resp_line)
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        // 对端在写响应前关闭/崩溃：read_line 返回 0（EOF）。这是连接中断，
        // 不是「协议解析错误」——旧实现对空串做 from_str 报成误导性的
        // "EOF while parsing"，影响上层诊断/重试归类。
        if n == 0 {
            return Err(ClientError::Connect("对端在响应前关闭连接".into()));
        }
        let resp: Response =
            serde_json::from_str(&resp_line).map_err(|e| ClientError::Protocol(e.to_string()))?;
        match resp {
            Response::Ok { result, .. } => Ok(result),
            Response::Err { error, .. } => Err(ClientError::Rpc {
                code: error.code,
                message: error.message,
            }),
        }
    }

    /// 订阅事件流（阻塞回调）。
    /// - follow=true：持续跟随新事件（连接保持）
    /// - follow=false：daemon 重放完 [from_seq, 现在) 即关闭连接（看历史用）
    pub fn subscribe<F: FnMut(maestro_protocol::events::Envelope) -> bool>(
        &self,
        from_seq: u64,
        follow: bool,
        mut on_event: F,
    ) -> Result<(), ClientError> {
        let mut stream = self
            .events
            .connect()
            .map_err(|e| ClientError::Connect(e.to_string()))?;
        let hello = serde_json::json!({ "from_seq": from_seq, "live": follow });
        stream
            .write_all(serde_json::to_string(&hello).unwrap().as_bytes())
            .and_then(|_| stream.write_all(b"\n"))
            .and_then(|_| stream.flush())
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let line = line.map_err(|e| ClientError::Protocol(e.to_string()))?;
            if line.trim().is_empty() {
                continue;
            }
            // 1) 先按无类型 Value 校验 JSON 结构完整性：结构损坏（传输
            //    破坏）属真实故障，返回错误而非静默丢弃。
            if let Err(e) = serde_json::from_str::<serde_json::Value>(&line) {
                return Err(ClientError::Protocol(format!("事件 JSON 损坏: {e}")));
            }
            // 2) 再做强类型解析：失败通常是 daemon 版本更新、发来旧客户端
            //    不认识的事件类型（前向兼容）——告警计数后跳过，不静默。
            match serde_json::from_str(&line) {
                Ok(env) => {
                    if !on_event(env) {
                        break;
                    }
                }
                Err(e) => eprintln!("maestro: 跳过无法解析的事件（daemon 更新？）: {e}"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_refused_when_no_daemon() {
        let dir = std::env::temp_dir().join("maestro-nonexistent-test");
        let c = MaestroClient::new(&dir);
        assert!(!c.is_daemon_alive());
        let err = c
            .call("r1", Method::ServerStatus, serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, ClientError::Connect(_)));
    }
}
