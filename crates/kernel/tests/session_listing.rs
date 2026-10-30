use kernel::transcript::ListLimit;

#[test]
fn limit_query_parses_a_lenient_integer_prefix() {
    let cases = [
        (None, 100),
        (Some(""), 100),
        (Some("0"), 100),
        (Some("-1"), 100),
        (Some("-5"), 100),
        (Some("abc"), 100),
        (Some("20abc"), 20),
        (Some("3.9"), 3),
        (Some("1e3"), 1),
        (Some("0x10"), 100),
        (Some("999"), 500),
        (Some("+7"), 7),
        (Some("  7"), 7),
        (Some("99999999999999999999999999999"), 500),
    ];
    for (raw, expected) in cases {
        assert_eq!(ListLimit::from_query(raw).get(), expected, "{raw:?}");
    }
    assert_eq!(ListLimit::per_harness_route().get(), 100);
}
