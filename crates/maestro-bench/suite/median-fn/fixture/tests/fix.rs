use median_fn::median;

#[test]
fn odd_length() {
    assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
    assert_eq!(median(&[5.0]), 5.0);
}

#[test]
fn even_length_averages_middle() {
    assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
    assert_eq!(median(&[10.0, 20.0]), 15.0);
}

#[test]
fn unsorted_input() {
    assert_eq!(median(&[9.0, 1.0, 5.0, 3.0, 7.0]), 5.0);
}

#[test]
#[should_panic(expected = "空")]
fn empty_panics_with_message() {
    median(&[]);
}
