/// 按字符数安全截断字符串。
pub fn truncate_chars(s: &str, n: usize) -> String {
    s[..n].to_string()
}
