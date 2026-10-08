use chrono::DateTime;
use pixiv_cli_rs::{
    DetailOutput, finish_command,
    search::{SearchOptions, artwork_search},
};
use pixiv_sdk::{
    Client, Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{self, Write},
    process::Command,
    sync::{Arc, Mutex},
};

type Queries = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
#[derive(Deserialize)]
struct Source {
    bodies: Vec<Value>,
}
#[derive(Deserialize)]
struct Case {
    args: Vec<String>,
    flags: BTreeMap<String, String>,
    source: usize,
    negative: bool,
    mode: String,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    stdout: String,
    stderr: String,
    error: String,
    exit: i32,
}
struct Fixture {
    bodies: Vec<Value>,
    queries: Queries,
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
            .and_then(|v| v.first())
            .map(String::as_str)
        {
            Some("30") => 1,
            Some("60") => 2,
            _ => 0,
        };
        self.queries.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}
struct Output {
    bytes: Vec<u8>,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn bookmark_search_matches_go_bounds_strategies_completeness_and_atomic_output() {
    let sources: Vec<Source> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-pages.json"
    ))
    .unwrap();
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-bookmark.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 393);
    for case in cases {
        let source = &sources[case.source];
        let mut options = SearchOptions::default();
        for (key, value) in &case.flags {
            match key.as_str() {
                "rating" => options.rating = value.clone(),
                "content-type" => options.content_type = value.clone(),
                "limit" => options.limit = Some(value.parse().unwrap()),
                "page" => options.page = Some(value.parse().unwrap()),
                "bookmark-min" => options.bookmark_min = Some(value.parse().unwrap()),
                "bookmark-max" => options.bookmark_max = Some(value.parse().unwrap()),
                "bookmark-strategy" => options.bookmark_strategy = Some(value.clone()),
                "period" => options.dates.period = value.clone(),
                _ => panic!("unexpected flag"),
            }
        }
        let queries = Arc::new(Mutex::new(vec![]));
        let mut bodies = source.bodies.clone();
        for (page_index, body) in bodies.iter_mut().enumerate() {
            for (item_index, item) in body["illusts"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                if item.is_object() {
                    item["total_bookmarks"] =
                        (if case.negative && page_index == 0 && item_index == 0 {
                            -1
                        } else {
                            item["id"].as_i64().unwrap() * 10
                        })
                        .into();
                }
            }
        }
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                bodies,
                queries: queries.clone(),
            },
        );
        let request = options.request(
            "cat",
            DateTime::parse_from_rfc3339("2024-02-29T12:00:00+09:00").unwrap(),
        );
        let ndjson = case.mode == "ndjson";
        let mode = if case.mode == "json" {
            DetailOutput::Json
        } else if ndjson {
            DetailOutput::Ndjson
        } else {
            DetailOutput::Human
        };
        let mut out = Output { bytes: vec![] };
        let early_error = request.is_err();
        let result = match request {
            Ok(request) => artwork_search(&client, request, &options, mode, &mut out).await,
            Err(error) => Err(error),
        };
        assert_eq!(
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default(),
            case.error,
            "source {} {} {:?}",
            case.source,
            case.mode,
            case.flags
        );
        let mut diagnostics = vec![];
        assert_eq!(
            finish_command(result, ndjson, case.mode != "human", &mut diagnostics),
            case.exit
        );
        assert_eq!(*queries.lock().unwrap(), case.queries);
        let actual = String::from_utf8(out.bytes).unwrap();
        if case.mode == "json" && !actual.is_empty() {
            assert_eq!(
                serde_json::from_str::<Value>(&actual).unwrap(),
                serde_json::from_str::<Value>(&case.stdout).unwrap()
            );
        } else if ndjson {
            let parse = |value: &str| {
                value
                    .lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap())
                    .collect::<Vec<_>>()
            };
            assert_eq!(parse(&actual), parse(&case.stdout));
        } else {
            assert_eq!(actual, case.stdout);
        }
        if case.mode != "human" && !diagnostics.is_empty() {
            assert_eq!(
                serde_json::from_slice::<Value>(&diagnostics).unwrap(),
                serde_json::from_str::<Value>(&case.stderr).unwrap()
            );
        } else {
            assert_eq!(String::from_utf8(diagnostics).unwrap(), case.stderr);
        }
        if early_error {
            let output = Command::new(env!("CARGO_BIN_EXE_pixiv"))
                .args(&case.args)
                .env("PIXIV_ACCESS_TOKEN", "")
                .env("https_proxy", "invalid proxy fixture")
                .env_remove("HTTPS_PROXY")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(case.exit));
            assert!(output.stdout.is_empty());
            if case.mode != "human" {
                assert_eq!(
                    serde_json::from_slice::<Value>(&output.stderr).unwrap(),
                    serde_json::from_str::<Value>(&case.stderr).unwrap()
                );
            } else {
                assert_eq!(String::from_utf8(output.stderr).unwrap(), case.stderr);
            }
        }
    }
}
