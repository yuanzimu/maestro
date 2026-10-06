use utf8_truncate::truncate_chars;

#[test]
fn ascii_truncates_normally() {
    assert_eq!(truncate_chars("hello", 3), "hel");
}

#[test]
fn multibyte_boundary_does_not_panic() {
    // 「你好世界」每字 3 字节：字节切片会落在字符中间 → panic
    assert_eq!(truncate_chars("你好世界", 2), "你好");
}

#[test]
fn n_beyond_len_returns_all() {
    assert_eq!(truncate_chars("ab", 10), "ab");
}

#[test]
fn empty_input() {
    assert_eq!(truncate_chars("", 3), "");
}
