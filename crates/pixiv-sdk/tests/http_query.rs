#[path = "common/http_query.rs"]
mod http_query;

use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    path: String,
    parameters: Vec<(String, String)>,
    target: String,
}

#[tokio::test]
async fn actual_http_query_bytes_match_go_escaping_and_preserve_order_and_existing_query() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/http-query.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 8);
    for case in cases {
        assert_eq!(
            http_query::capture_target(&case.path, case.parameters).await,
            case.target,
            "{}",
            case.name
        );
    }
}
