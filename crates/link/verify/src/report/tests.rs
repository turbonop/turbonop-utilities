use super::*;

#[test]
fn counts_examples_and_verdicts() {
    let mut r = Report::new();
    assert!(r.passed());
    r.checked("members", 3);
    for i in 0..8 {
        r.fail("missing symbols", || format!("s{i}"));
    }
    r.expect("modules changed", || "f".into());
    assert_eq!(r.failures(), 8);
    assert!(r.failed("missing symbols"));
    let f = r.finding(Severity::Failure, "missing symbols").unwrap();
    assert_eq!(f.count, 8);
    assert_eq!(f.examples.len(), EXAMPLES);
    let text = r.text();
    assert!(text.contains("FAILED: missing symbols: 8"), "{text}");
    assert!(text.contains("expected: modules changed: 1"), "{text}");
    assert!(
        text.contains("NOT equivalent: 8 failures in 1 categories"),
        "{text}"
    );
}

#[test]
fn merging_keeps_order_and_limits() {
    let mut a = Report::new();
    a.fail("x", || "a1".into());
    a.checked("things", 1);
    let mut b = Report::new();
    for i in 0..10 {
        b.fail("x", || format!("b{i}"));
    }
    b.checked("things", 2);
    a.merge(b);
    let f = a.finding(Severity::Failure, "x").unwrap();
    assert_eq!(f.count, 11);
    assert_eq!(f.examples[0], "a1");
    assert_eq!(f.examples.len(), EXAMPLES);
    assert_eq!(a.checked_count("things"), 3);
}

#[test]
fn shows_bytes() {
    assert_eq!(show(b"?f@@YAXXZ"), "?f@@YAXXZ");
    assert_eq!(show(b"a\x00b"), "a\\x00b");
}
