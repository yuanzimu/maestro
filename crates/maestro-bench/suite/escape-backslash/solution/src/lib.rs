/// 转义字符串中的特殊字符（先反斜杠后双引号，保证可逆）。
pub fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
