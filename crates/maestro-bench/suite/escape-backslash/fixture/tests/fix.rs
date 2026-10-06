use escape_backslash::escape;

#[test]
fn escapes_quotes() {
    assert_eq!(escape("a\"b"), "a\\\"b");
}

#[test]
fn escapes_backslashes() {
    assert_eq!(escape("a\\b"), "a\\\\b");
    assert_eq!(escape("C:\\path"), "C:\\\\path");
}

#[test]
fn mixed() {
    assert_eq!(escape("x\"\\y"), "x\\\"\\\\y");
}

#[test]
fn plain_passthrough() {
    assert_eq!(escape("hello"), "hello");
}
