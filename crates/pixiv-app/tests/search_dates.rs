use pixiv_app::dates::quick_date_range;
use serde::Deserialize;

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct Case {
    now: String,
    duration: String,
    start: String,
    end: String,
    ok: bool,
}

#[test]
fn search_ranges_match_both_go_adapters_at_tokyo_midnight_month_ends_and_leap_days() {
    let cli: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-dates-cli.json"
    ))
    .unwrap();
    let mcp: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-dates-mcp.json"
    ))
    .unwrap();
    assert_eq!(cli, mcp);
    assert_eq!(cli.len(), 168);
    for case in cli {
        let now = chrono::DateTime::parse_from_rfc3339(&case.now).unwrap();
        let range = quick_date_range(&case.duration, now).unwrap();
        assert_eq!(range.is_some(), case.ok, "{} {}", case.now, case.duration);
        if let Some(range) = range {
            assert_eq!(
                (range.start_date, range.end_date),
                (case.start, case.end),
                "{} {}",
                case.now,
                case.duration
            );
        } else {
            assert_eq!((case.start.as_str(), case.end.as_str()), ("", ""));
        }
    }
}
