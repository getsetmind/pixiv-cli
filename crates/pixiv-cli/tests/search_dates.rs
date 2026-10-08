use chrono::DateTime;
use pixiv_cli_rs::search::SearchDateOptions;
use pixiv_sdk::{
    Client, Result,
    pixiv::SearchArtworksRequest,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    process::Command,
    sync::{Arc, Mutex},
};

type Queries = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;

#[derive(Deserialize)]
struct Case {
    period: String,
    start_date: String,
    end_date: String,
    mode: String,
    args: Vec<String>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    calls: usize,
    error: String,
    stdout: String,
    stderr: String,
    exit: i32,
}

struct Fixture(Queries);
impl Transport for Fixture {
    async fn send(&self, request: Request) -> Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/illust");
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        self.0.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({"illusts": []}),
        })
    }
}

#[tokio::test]
async fn search_date_flags_match_go_queries_and_process_errors_before_client_configuration() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-date-options.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 213);
    let mut process_cases = 0;
    for case in cases {
        let mut request = SearchArtworksRequest {
            word: "cat".into(),
            ..Default::default()
        };
        let options = SearchDateOptions {
            period: case.period,
            start_date: case.start_date,
            end_date: case.end_date,
        };
        let result = options.apply(
            &mut request,
            DateTime::parse_from_rfc3339("2024-02-29T12:00:00+09:00").unwrap(),
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default(),
            case.error,
            "{:?}",
            case.args
        );
        if result.is_ok() {
            assert_eq!(case.calls, 1);
            let seen = Arc::new(Mutex::new(vec![]));
            let client = Client::with_transport("fixture-access", Fixture(seen.clone()));
            assert!(
                client
                    .search_artworks(request)
                    .await
                    .unwrap()
                    .items
                    .is_empty()
            );
            assert_eq!(*seen.lock().unwrap(), case.queries, "{:?}", case.args);
        } else {
            assert_eq!(case.calls, 0);
            assert!(case.queries.is_empty());
            if case.args.iter().any(|argument| argument.contains('\0')) {
                continue;
            }
            process_cases += 1;
            let output = Command::new(env!("CARGO_BIN_EXE_pixiv"))
                .args(&case.args)
                .env("PIXIV_ACCESS_TOKEN", "")
                .env("https_proxy", "invalid proxy fixture")
                .env_remove("HTTPS_PROXY")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(case.exit), "{:?}", case.args);
            assert_eq!(String::from_utf8(output.stdout).unwrap(), case.stdout);
            if case.mode == "human" {
                assert_eq!(String::from_utf8(output.stderr).unwrap(), case.stderr);
            } else {
                assert_eq!(
                    serde_json::from_slice::<Value>(&output.stderr).unwrap(),
                    serde_json::from_str::<Value>(&case.stderr).unwrap()
                );
                assert_eq!(
                    output.stderr.iter().filter(|byte| **byte == b'\n').count(),
                    1
                );
            }
        }
    }
    assert_eq!(process_cases, 174);
}

#[test]
fn long_period_flags_use_the_shared_go_date_contract() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-dates-cli.json"
    ))
    .unwrap();
    let mut checked = 0;
    for case in cases.iter().filter(|case| case["ok"] == true) {
        let options = SearchDateOptions {
            period: if case["duration"] == "within_half_year" {
                "half-year"
            } else {
                "year"
            }
            .into(),
            ..Default::default()
        };
        let mut request = SearchArtworksRequest::default();
        options
            .apply(
                &mut request,
                DateTime::parse_from_rfc3339(case["now"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
        assert!(request.duration.is_empty());
        assert_eq!(request.start_date, case["start"]);
        assert_eq!(request.end_date, case["end"]);
        checked += 1;
    }
    assert_eq!(checked, 42);
}
