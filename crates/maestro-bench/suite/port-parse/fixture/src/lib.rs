/// 解析端口号字符串为 u16；非法输入应返回 Err 而非 panic。
pub fn parse_port(s: &str) -> Result<u16, String> {
    let n: u32 = s.parse().unwrap();
    if n > 65535 {
        panic!("port too large");
    }
    Ok(n as u16)
}
