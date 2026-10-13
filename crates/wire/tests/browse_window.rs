use strum::IntoEnumIterator;
use wire::gateway::memory::BrowseWindow;

#[test]
fn window_serde_names_match_the_spec() {
    let expected = [
        (BrowseWindow::Today, "\"today\""),
        (BrowseWindow::Yesterday, "\"yesterday\""),
        (BrowseWindow::ThisMorning, "\"this_morning\""),
        (BrowseWindow::ThisWeek, "\"this_week\""),
        (BrowseWindow::Last24h, "\"last_24h\""),
        (BrowseWindow::Last7d, "\"last_7d\""),
        (BrowseWindow::Last14d, "\"last_14d\""),
    ];
    for (window, json) in expected {
        assert_eq!(serde_json::to_string(&window).unwrap(), json);
        assert_eq!(serde_json::from_str::<BrowseWindow>(json).unwrap(), window);
    }
}

#[test]
fn every_window_variant_is_covered_by_the_spec_table() {
    assert_eq!(BrowseWindow::iter().count(), 7);
}

#[test]
fn window_serde_rejects_unknown_names() {
    assert!(serde_json::from_str::<BrowseWindow>("\"tomorrow\"").is_err());
}

#[test]
fn window_parse_normalizes_case_and_whitespace() {
    assert_eq!(BrowseWindow::parse("TODAY").unwrap(), BrowseWindow::Today);
    assert_eq!(
        BrowseWindow::parse("  last_7d\t").unwrap(),
        BrowseWindow::Last7d
    );
}

#[test]
fn window_parse_reports_the_spec_errors() {
    let empty = BrowseWindow::parse("   ").unwrap_err().to_string();
    assert_eq!(
        empty,
        "Invalid window=\"\" — expected one of: today, yesterday, this_morning, this_week, last_24h, last_7d, last_14d"
    );
    let unknown = BrowseWindow::parse("Fortnight").unwrap_err().to_string();
    assert_eq!(
        unknown,
        "Unknown window=\"Fortnight\" (normalized to \"fortnight\"). Expected one of: today, yesterday, this_morning, this_week, last_24h, last_7d, last_14d"
    );
}
