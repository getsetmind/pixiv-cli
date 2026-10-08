use chrono::DateTime;
use pixiv_cli_rs::search::SearchOptions;
use pixiv_sdk::{
    Client, Result,
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
    name: String,
    flags: BTreeMap<String, String>,
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
async fn search_selectors_match_go_queries_normalization_validation_order_and_process_errors() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-options.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 210);
    let mut processes = 0;
    let mut successes = 0;
    for case in cases {
        let mut options = SearchOptions::default();
        for (key, value) in &case.flags {
            match key.as_str() {
                "search-by" => options.search_by = value.clone(),
                "sort" => options.sort = value.clone(),
                "content-type" => options.content_type = value.clone(),
                "ai-mode" => options.ai_mode = value.clone(),
                "aspect-ratio" => options.aspect_ratio = value.clone(),
                "resolution" => options.resolution = value.clone(),
                "draw-tool" => options.draw_tool = value.clone(),
                "period" => options.dates.period = value.clone(),
                "start-date" => options.dates.start_date = value.clone(),
                "end-date" => options.dates.end_date = value.clone(),
                _ => panic!("unknown fixture flag: {key}"),
            }
        }
        let seen = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport("fixture-access", Fixture(seen.clone()));
        let prepared = options.request(
            "cat",
            DateTime::parse_from_rfc3339("2024-02-29T12:00:00+09:00").unwrap(),
        );
        assert_eq!(usize::from(prepared.is_ok()), case.calls, "{}", case.name);
        let error = match prepared {
            Ok(request) => client
                .search_artworks(request)
                .await
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default(),
            Err(error) => error.to_string(),
        };
        assert_eq!(error, case.error, "{}", case.name);
        assert_eq!(*seen.lock().unwrap(), case.queries, "{}", case.name);
        if !case.error.is_empty() {
            processes += 1;
            let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
            command
                .args(&case.args)
                .env("PIXIV_ACCESS_TOKEN", "")
                .env_remove("https_proxy")
                .env_remove("HTTPS_PROXY");
            if case.calls == 0 {
                command.env("https_proxy", "invalid proxy fixture");
            }
            let output = command.output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(case.exit),
                "{} {:?}",
                case.name,
                case.args
            );
            assert_eq!(String::from_utf8(output.stdout).unwrap(), case.stdout);
            if case.mode == "human" {
                assert_eq!(
                    String::from_utf8(output.stderr).unwrap(),
                    case.stderr,
                    "{}",
                    case.name
                );
            } else {
                assert_eq!(
                    serde_json::from_slice::<Value>(&output.stderr).unwrap(),
                    serde_json::from_str::<Value>(&case.stderr).unwrap(),
                    "{}",
                    case.name
                );
                assert_eq!(
                    output.stderr.iter().filter(|byte| **byte == b'\n').count(),
                    1
                );
            }
        } else {
            successes += 1;
            assert_eq!(case.exit, 0);
        }
    }
    assert_eq!(processes, 99);
    assert_eq!(successes, 111);
}
