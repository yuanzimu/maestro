/// 服务端配置解析（旧实现，与新重复）。
pub fn load_server_config(text: &str) -> Option<(String, u16)> {
    let mut host = None;
    let mut port = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("host=") {
            host = Some(v.trim().to_string());
        }
        if let Some(v) = line.strip_prefix("port=") {
            port = v.trim().parse::<u16>().ok();
        }
    }
    match (host, port) {
        (Some(h), Some(p)) => Some((h, p)),
        _ => None,
    }
}

/// 客户端配置解析（旧实现，与服务端完全重复）。
pub fn load_client_config(text: &str) -> Option<(String, u16)> {
    let mut host = None;
    let mut port = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("host=") {
            host = Some(v.trim().to_string());
        }
        if let Some(v) = line.strip_prefix("port=") {
            port = v.trim().parse::<u16>().ok();
        }
    }
    match (host, port) {
        (Some(h), Some(p)) => Some((h, p)),
        _ => None,
    }
}
