use pixiv_app::search_filter::normalize_filter;
use serde::Deserialize;

#[derive(Deserialize)]
struct Point {
    x_restrict: i64,
    kind: String,
}
#[derive(Deserialize)]
struct Row {
    rating: String,
    content_type: String,
    normalized_rating: String,
    normalized_content_type: String,
    context: String,
    error: String,
    matches: Vec<bool>,
}
#[derive(Deserialize)]
struct Fixture {
    points: Vec<Point>,
    rows: Vec<Row>,
}

#[test]
fn local_filters_match_go_normalization_cursor_binding_and_unknown_rating_classes() {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-local-filter.json"
    ))
    .unwrap();
    assert_eq!(fixture.points.len(), 42);
    assert_eq!(fixture.rows.len(), 168);
    let mut checked = 0;
    for row in fixture.rows {
        match normalize_filter(&row.rating, &row.content_type) {
            Ok(filter) => {
                assert!(row.error.is_empty());
                assert_eq!(filter.rating, row.normalized_rating);
                assert_eq!(filter.content_type, row.normalized_content_type);
                assert_eq!(filter.cursor_context, row.context);
                assert_eq!(row.matches.len(), fixture.points.len());
                for (point, want) in fixture.points.iter().zip(row.matches) {
                    assert_eq!(
                        filter.matches(point.x_restrict, &point.kind),
                        want,
                        "{} {} {} {}",
                        row.rating,
                        row.content_type,
                        point.x_restrict,
                        point.kind
                    );
                    checked += 1;
                }
            }
            Err(error) => {
                assert_eq!(error.to_string(), row.error);
                assert!(row.matches.is_empty());
                assert!(row.context.is_empty());
            }
        }
    }
    assert_eq!(checked, 3360);
}
