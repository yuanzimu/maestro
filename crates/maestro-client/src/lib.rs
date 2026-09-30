//! Maestro Rust 客户端库：连接 API socket 的 JSON-RPC + 事件流订阅。

use maestro_protocol::api::{Method, Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("连接失败（daemon 未启动？）: {0}")]
    Connect(String),
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("RPC 错误 {code}: {message}")]
    Rpc { code: i32, message: String },
}

pub struct MaestroClient {
    api_sock: PathBuf,
    events_sock: PathBuf,
}

/// 默认 socket 位置
pub fn default_data_dir() -> PathBuf {
    std::env::var("MAESTRO_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/maestro"))
}

impl MaestroClient {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            api_sock: data_dir.join("maestro.api.sock"),
            events_sock: data_dir.join("maestro.events.sock"),
        }
    }

    pub fn connect_default() -> Self {
        Self::new(&default_data_dir())
    }

    /// daemon 是否在监听
    pub fn is_daemon_alive(&self) -> bool {
        UnixStream::connect(&self.api_sock).is_ok()
    }

    /// 发送一个请求（每次新建连接：简单可靠，CLI 场景足够）
    pub fn call(
        &self,
        id: &str,
        method: Method,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        let mut stream =
            UnixStream::connect(&self.api_sock).map_err(|e| ClientError::Connect(e.to_string()))?;
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
        reader
            .read_line(&mut resp_line)
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
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

    /// 订阅事件流（阻塞回调）
    pub fn subscribe<F: FnMut(maestro_protocol::events::Envelope) -> bool>(
        &self,
        from_seq: u64,
        mut on_event: F,
    ) -> Result<(), ClientError> {
        let mut stream = UnixStream::connect(&self.events_sock)
            .map_err(|e| ClientError::Connect(e.to_string()))?;
        let hello = serde_json::json!({ "from_seq": from_seq });
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
            match serde_json::from_str(&line) {
                Ok(env) => {
                    if !on_event(env) {
                        break;
                    }
                }
                Err(_) => continue,
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
        let c = MaestroClient::new(Path::new("/tmp/maestro-nonexistent-test"));
        assert!(!c.is_daemon_alive());
        let err = c
            .call("r1", Method::ServerStatus, serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, ClientError::Connect(_)));
    }
}
