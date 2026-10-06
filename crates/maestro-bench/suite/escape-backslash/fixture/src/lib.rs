/// 转义字符串中的特殊字符（双引号与反斜杠）。
pub fn escape(s: &str) -> String {
    s.replace('"', "\\\"")
}
