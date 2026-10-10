#[path = "support/reverse_search.rs"]
mod support;

use pixiv_app::{lifecycle::Context, reverse_search::Provider};
use pixiv_mcp::reverse_search::{ReverseExecutor, ReverseSearchInput, reverse_search_tool};
use std::sync::atomic::Ordering;

#[test]
fn reverse_search_lists_the_exact_frozen_input_and_output_schema() {
    assert_eq!(reverse_search_tool(), support::fixture()["tool"]);
}

#[tokio::test]
async fn reverse_executor_preserves_the_frozen_results_errors_and_caller_ownership() {
    let fixture = support::fixture();
    for row in fixture["cases"].as_array().unwrap() {
        let Some(expected) = row["wire_response"].get("result") else {
            continue;
        };
        if row["unconfigured"] == true {
            continue;
        }
        let searcher = support::FixtureSearcher::from_row(row);
        let executor = ReverseExecutor::new(
            searcher.clone(),
            Provider::from(row["startup_provider"].as_str().unwrap()),
            row["startup_pixiv_only"].as_bool().unwrap(),
        );
        let input = row["wire_request"]["params"]["arguments"].clone();
        let actual = executor
            .invoke(
                &Context::new(),
                ReverseSearchInput {
                    source: input["source"].as_str().unwrap().into(),
                    provider: input["provider"].as_str().unwrap_or_default().into(),
                },
            )
            .await;
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            *expected,
            "{}",
            row["name"]
        );
        assert_eq!(
            *searcher.requests.lock().unwrap(),
            *row["searcher_requests"].as_array().unwrap(),
            "{}",
            row["name"]
        );
        drop(executor);
        assert_eq!(searcher.closed.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn structured_numeric_id_uses_sdk_wire_round_trip_while_record_identity_is_exact() {
    let response = support::response(&serde_json::json!({
        "input":{"kind":"","sha256":""},
        "providers":null,
        "results":[{"pixiv":{"type":"artwork","id":9007199254740993_i64},"evidence":null}],
        "provider_errors":null,"partial":false
    }));
    let searcher = support::FixtureSearcher::new(move |_, _| {
        let response = response.clone();
        Box::pin(async move {
            pixiv_app::reverse_search::SearchOutcome {
                response,
                error: None,
            }
        })
    });
    let executor = ReverseExecutor::new(searcher, Provider::SauceNao, false);
    let actual = serde_json::to_value(
        executor
            .invoke(
                &Context::new(),
                ReverseSearchInput {
                    source: String::from("/synthetic/private-source-secret.png"),
                    provider: String::new(),
                },
            )
            .await,
    )
    .unwrap();
    assert_eq!(
        actual["structuredContent"]["results"][0]["pixiv"]["id"],
        9007199254740992_i64
    );
    assert_eq!(
        actual["structuredContent"]["records"][0],
        serde_json::json!({
            "id":"9007199254740993","type":"artwork","url":"https://www.pixiv.net/artworks/9007199254740993"
        })
    );
}
