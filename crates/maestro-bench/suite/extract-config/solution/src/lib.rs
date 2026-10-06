/// 统一配置解析：重构抽取（旧函数委托至此，行为不变）。
pub fn load_config(text: &str) -> Option<(String, u16)> {
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

/// 服务端配置解析（委托统一实现）。
pub fn load_server_config(text: &str) -> Option<(String, u16)> {
    load_config(text)
}

/// 客户端配置解析（委托统一实现）。
pub fn load_client_config(text: &str) -> Option<(String, u16)> {
    load_config(text)
}
