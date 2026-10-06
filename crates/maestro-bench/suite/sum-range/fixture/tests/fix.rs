use sum_range::sum_upto;

#[test]
fn includes_endpoint() {
    assert_eq!(sum_upto(5), 15); // 0+1+2+3+4+5
}

#[test]
fn small_cases() {
    assert_eq!(sum_upto(0), 0);
    assert_eq!(sum_upto(1), 1);
    assert_eq!(sum_upto(2), 3);
}

#[test]
fn matches_naive_loop() {
    let naive: u64 = (0..=100).sum();
    assert_eq!(sum_upto(100), naive);
}
