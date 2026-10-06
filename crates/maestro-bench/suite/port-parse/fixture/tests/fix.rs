use port_parse::parse_port;

#[test]
fn valid_port() {
    assert_eq!(parse_port("8080"), Ok(8080));
    assert_eq!(parse_port("0"), Ok(0));
    assert_eq!(parse_port("65535"), Ok(65535));
}

#[test]
fn not_a_number_is_err() {
    let r = parse_port("abc");
    assert!(r.is_err(), "非数字应返回 Err: {r:?}");
}

#[test]
fn out_of_range_is_err() {
    let r = parse_port("99999");
    assert!(r.is_err(), "超范围应返回 Err: {r:?}");
    let r = parse_port("-1");
    assert!(r.is_err(), "负数应返回 Err: {r:?}");
}

#[test]
fn error_message_is_human_readable() {
    let msg = parse_port("abc").unwrap_err();
    assert!(!msg.is_empty(), "错误信息不应为空");
}
