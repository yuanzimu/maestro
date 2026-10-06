/// 解析端口号字符串为 u16；非法输入返回带上下文的 Err。
pub fn parse_port(s: &str) -> Result<u16, String> {
    let n: u32 = s
        .parse()
        .map_err(|_| format!("无效端口号: {s:?}（应为 0~65535 的整数）"))?;
    u16::try_from(n).map_err(|_| format!("端口号超出范围: {n}（应为 0~65535）"))
}
