/// 数值序列的中位数（奇数取中间，偶数取中间两数平均）。
pub fn median(vals: &[f64]) -> f64 {
    if vals.is_empty() {
        panic!("median: 空序列没有中位数");
    }
    let mut s: Vec<f64> = vals.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).expect("NaN 不可比较"));
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    }
}
