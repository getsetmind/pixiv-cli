use chrono::DateTime;
use pixiv_cli_rs::search::{SearchOptions, collect_search, write_search_json};
use pixiv_sdk::{
    Client, Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    process::Command,
    sync::{Arc, Mutex},
};

type Queries = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
#[derive(Deserialize)]
struct Case {
    flags: BTreeMap<String, String>,
    bodies: Vec<Value>,
    args: Vec<String>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    calls: usize,
    error: String,
    stdout: String,
    stderr: String,
    exit: i32,
}
struct Fixture {
    bodies: Vec<Value>,
    seen: Queries,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/illust");
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let index = match query
            .get("offset")
            .and_then(|value| value.first())
            .map(String::as_str)
        {
            Some("30") => 1,
            Some("60") => 2,
            _ => 0,
        };
        self.seen.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}

#[tokio::test]
async fn search_rating_and_logical_pages_match_go_without_dropping_duplicates_or_partial_failure_guards()
 {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-pages.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 90);
    for case in cases {
        let mut options = SearchOptions::default();
        for (key, value) in &case.flags {
            match key.as_str() {
                "rating" => options.rating = value.clone(),
                "content-type" => options.content_type = value.clone(),
                "search-by" => options.search_by = value.clone(),
                "period" => options.dates.period = value.clone(),
                "limit" => options.limit = Some(value.parse().unwrap()),
                "page" => options.page = Some(value.parse().unwrap()),
                _ => panic!("unknown flag {key}"),
            }
        }
        let seen = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                bodies: case.bodies,
                seen: seen.clone(),
            },
        );
        let prepared = options.request(
            "cat",
            DateTime::parse_from_rfc3339("2024-02-29T12:00:00+09:00").unwrap(),
        );
        assert_eq!(
            usize::from(prepared.is_ok()),
            case.calls,
            "{:?}",
            case.flags
        );
        let result = match prepared {
            Ok(query) => collect_search(&client, query, &options).await,
            Err(error) => Err(error),
        };
        let error = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert_eq!(error, case.error, "{:?}", case.flags);
        assert_eq!(*seen.lock().unwrap(), case.queries, "{:?}", case.flags);
        if let Ok(items) = result {
            let mut out = vec![];
            write_search_json(&items, &mut out).unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&out).unwrap(),
                serde_json::from_str::<Value>(&case.stdout).unwrap(),
                "{:?}",
                case.flags
            );
            assert_eq!(case.exit, 0);
        } else {
            assert!(case.stdout.is_empty());
            if case.calls == 0 {
                let home = tempfile::tempdir().unwrap();
                let output = Command::new(env!("CARGO_BIN_EXE_pixiv"))
                    .args(case.args)
                    .env("HOME", home.path())
                    .env("USERPROFILE", home.path())
                    .env("PIXIV_ACCESS_TOKEN", "")
                    .env("https_proxy", "invalid proxy fixture")
                    .env_remove("HTTPS_PROXY")
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(case.exit));
                assert!(output.stdout.is_empty());
                assert_eq!(
                    serde_json::from_slice::<Value>(&output.stderr).unwrap(),
                    serde_json::from_str::<Value>(&case.stderr).unwrap()
                );
            }
        }
    }
}
