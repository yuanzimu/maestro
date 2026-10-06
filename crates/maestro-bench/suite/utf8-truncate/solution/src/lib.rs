/// 按字符数安全截断字符串（字符边界，不落多字节中间）。
pub fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
