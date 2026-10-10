#[path = "support/updater_source_selection/mod.rs"]
mod support;

use pixiv_app::update::source::{
    GITHUB_USER_AGENT, ReleaseSourceSelector, default_release_sources, first_release_source,
    parse_release_sources,
};
use serde_json::json;
use std::sync::Arc;
use support::*;

#[test]
fn frozen_parser_and_transform_rows() {
    let fixture = fixture();
    let mut covered = 0;
    for row in rows(&fixture).filter(|row| !go_only(row)) {
        let input = &row["input"];
        let output = &row["output"];
        match text(row, "operation") {
            "parse" => {
                let result = parse_release_sources(text(input, "body").as_bytes());
                assert_message(&result, &output["error"], row);
                if let Ok(sources) = result {
                    assert_eq!(
                        ids(&sources),
                        source_ids(&output["sources"]),
                        "{}",
                        row["name"]
                    );
                    for (source, expected) in
                        sources.iter().zip(output["sources"].as_array().unwrap())
                    {
                        let canonical = "https://github.com/FlanChanXwO/pixiv-cli/releases";
                        let escaped =
                            "https%3A%2F%2Fgithub.com%2FFlanChanXwO%2Fpixiv-cli%2Freleases";
                        let expected_url = |field| {
                            text(expected, field)
                                .replace("{url}", canonical)
                                .replace("{url_query}", escaped)
                        };
                        if text(expected, "api").is_empty() {
                            assert_eq!(
                                source.api_url(canonical).unwrap_err().to_string(),
                                format!(
                                    "release source \"{}\" does not support GitHub Releases API",
                                    source.id()
                                )
                            );
                        } else {
                            assert_eq!(source.api_url(canonical).unwrap(), expected_url("api"));
                        }
                        assert_eq!(source.asset_url(canonical).unwrap(), expected_url("asset"));
                    }
                }
                covered += 1;
            }
            "transform" | "embedded_transform" => {
                let source = if text(row, "operation") == "embedded_transform" {
                    default_release_sources()
                        .into_iter()
                        .find(|source| source.id() == text(input, "id"))
                        .unwrap()
                } else {
                    sources_from_descriptions(&json!([input["source"]])).remove(0)
                };
                let canonical = input["canonical"]
                    .as_str()
                    .unwrap_or_else(|| text(input, "api_canonical"));
                let api = source.api_url(canonical);
                assert_message(&api, &output["api_error"], row);
                assert_eq!(
                    api.unwrap_or_default(),
                    text(output, "api_url"),
                    "{} API",
                    row["name"]
                );
                let canonical = input["asset_canonical"].as_str().unwrap_or(canonical);
                let asset = source.asset_url(canonical);
                assert_message(&asset, &output["asset_error"], row);
                assert_eq!(
                    asset.unwrap_or_default(),
                    text(output, "asset_url"),
                    "{} asset",
                    row["name"]
                );
                covered += 1;
            }
            "default_sources" => {
                let mut first = default_release_sources();
                first.clear();
                let second = default_release_sources();
                assert_eq!(ids(&second), source_ids(&output["sources"]));
                assert_eq!(GITHUB_USER_AGENT, text(output, "user_agent"));
                covered += 1;
            }
            "first_source_behavior" => {
                let sources =
                    parse_release_sources(b"a|{url}|{url}\nasset|-|{url}\nc|{url}|{url}").unwrap();
                assert_eq!(
                    first_release_source(&sources).unwrap().id(),
                    text(output, "first_id")
                );
                assert!(first_release_source(&[]).is_none());
                covered += 1;
            }
            _ => {}
        }
    }
    assert_eq!(covered, 100);
}

#[tokio::test]
async fn frozen_single_probe_rows_preserve_body_and_request_boundaries() {
    let fixture = fixture();
    for row in rows(&fixture).filter(|row| text(row, "operation") == "ordered_single") {
        let input = &row["input"];
        let output = &row["output"];
        let (context, caller) = context(text(input, "context"));
        let transport = Arc::new(SingleTransport::from_input(
            input,
            context.clone(),
            caller.clone(),
        ));
        let selector = ReleaseSourceSelector::new(
            sources_from_descriptions(&input["sources"]),
            Arc::new(pixiv_app::update::http::ClientTransport::new(
                transport.clone(),
            )),
        );
        let result = selector
            .ordered(context, kind(text(input, "kind")), text(input, "canonical"))
            .await;
        assert_probe_message(&result, &output["error"], row);
        assert_eq!(
            result.map(|sources| ids(&sources)).unwrap_or_default(),
            strings(&output["ids"]),
            "{}",
            row["name"]
        );
        assert_eq!(
            json!(transport.trace()),
            output["body_trace"],
            "{} body",
            row["name"]
        );
        assert_eq!(
            json!(transport.requests()),
            output["requests"],
            "{} requests",
            row["name"]
        );
        assert_eq!(
            caller.error().is_some(),
            output["caller_canceled"].as_bool().unwrap()
        );
    }
}

#[tokio::test]
async fn frozen_redirect_rows_follow_go_default_http_client_policy() {
    let fixture = fixture();
    for row in rows(&fixture).filter(|row| text(row, "operation") == "ordered_redirect") {
        let input = &row["input"];
        let output = &row["output"];
        let (context, caller) = context("");
        let transport = Arc::new(RedirectTransport::new(
            input.clone(),
            context.clone(),
            caller,
        ));
        let selector = ReleaseSourceSelector::new(
            sources_from_descriptions(&input["sources"]),
            Arc::new(pixiv_app::update::http::ClientTransport::new(
                transport.clone(),
            )),
        );
        let result = selector
            .ordered(context, kind(text(input, "kind")), text(input, "canonical"))
            .await;
        assert_message(&result, &output["error"], row);
        assert_eq!(
            result.map(|sources| ids(&sources)).unwrap_or_default(),
            strings(&output["ids"])
        );
        assert_eq!(
            json!(transport.requests()),
            output["requests"],
            "{} requests",
            row["name"]
        );
        assert_eq!(
            json!(transport.traces()),
            output["body_traces"],
            "{} bodies",
            row["name"]
        );
    }
}

#[tokio::test]
async fn frozen_ordered_boundaries_and_constructor_ownership() {
    let fixture = fixture();
    for row in rows(&fixture).filter(|row| {
        !go_only(row)
            && matches!(
                text(row, "operation"),
                "ordered_boundary" | "ordered_constructor_copy" | "check_context"
            )
    }) {
        let input = &row["input"];
        let output = &row["output"];
        let (context, caller) = context(input["context"].as_str().unwrap_or(""));
        let mock = json!({"body_hex":"5b5d","status":200,"body_mode":"","transport":""});
        let transport = Arc::new(SingleTransport::from_input(&mock, context.clone(), caller));
        let mut sources = if text(row, "operation") == "ordered_constructor_copy" {
            parse_release_sources(b"original|{url}|{url}").unwrap()
        } else if text(row, "operation") == "check_context" {
            parse_release_sources(b"owned|{url}|{url}").unwrap()
        } else {
            sources_from_descriptions(&input["sources"])
        };
        let selector = ReleaseSourceSelector::new(
            sources.clone(),
            Arc::new(pixiv_app::update::http::ClientTransport::new(
                transport.clone(),
            )),
        );
        if text(row, "operation") == "ordered_constructor_copy" {
            if input["mutate_slice_after_constructor"].as_bool().unwrap() {
                sources = parse_release_sources(b"replacement|{url}|{url}").unwrap();
            }
            assert_eq!(ids(&sources), strings(&input["input_ids_when_ordered"]));
        }
        let result = selector
            .ordered(
                context,
                kind(input["kind"].as_str().unwrap_or("GitHub Releases API")),
                input["canonical"]
                    .as_str()
                    .unwrap_or("https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases"),
            )
            .await;
        if text(row, "operation") == "check_context" {
            let mut expected = output["error"].clone();
            expected["message"] = json!(
                text(&output["error"], "message").replace("owned action", "select release source")
            );
            assert_message(&result, &expected, row);
        } else {
            assert_message(&result, &output["error"], row);
            assert_eq!(
                result.map(|sources| ids(&sources)).unwrap_or_default(),
                strings(&output["ids"])
            );
            if let Some(calls) = output["transport_calls"].as_u64() {
                assert_eq!(transport.requests().len() as u64, calls);
            }
        }
    }
}

#[tokio::test]
async fn frozen_concurrent_source_outcomes_keep_winner_order_and_cancel_losers() {
    let fixture = fixture();
    for row in rows(&fixture).filter(|row| {
        matches!(
            text(row, "operation"),
            "ordered_race" | "ordered_all_failure" | "ordered_parent_cancel"
        )
    }) {
        run_concurrent_row(row).await;
    }
}

#[test]
fn every_frozen_row_has_an_explicit_contract_mapping() {
    let fixture = fixture();
    let mut behavior = 0;
    let mut go = 0;
    for row in rows(&fixture) {
        if go_only(row) {
            go += 1;
            assert!(matches!(
                text(row, "operation"),
                "template_parse"
                    | "template_apply"
                    | "probe_private"
                    | "ordered_boundary"
                    | "check_context"
                    | "first_source"
                    | "constructors"
                    | "candidates"
                    | "place_first"
            ));
        } else {
            behavior += 1;
            assert!(matches!(
                text(row, "operation"),
                "parse"
                    | "transform"
                    | "default_sources"
                    | "embedded_transform"
                    | "ordered_single"
                    | "ordered_redirect"
                    | "ordered_boundary"
                    | "ordered_constructor_copy"
                    | "check_context"
                    | "first_source_behavior"
                    | "ordered_race"
                    | "ordered_all_failure"
                    | "ordered_parent_cancel"
            ));
        }
    }
    assert_eq!((behavior, go), (167, 33));
}

#[tokio::test]
async fn a_context_ignoring_loser_cannot_hold_a_successful_source_selection_open() {
    ignoring_loser_is_dropped_without_delaying_the_winner(false).await;
    ignoring_loser_is_dropped_without_delaying_the_winner(true).await;
}
