use chrono::DateTime;
use pixiv_cli_rs::{
    DetailOutput, finish_command,
    search::{SearchOptions, artwork_search, saved_artwork_search},
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
    sync::{Arc, Mutex},
};

type Queries = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
#[path = "support/saved_account.rs"]
mod saved_account;
#[derive(Deserialize)]
struct Source {
    flags: BTreeMap<String, String>,
    bodies: Vec<Value>,
}
#[derive(Deserialize)]
struct Case {
    source: usize,
    mode: String,
    failure: String,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    stdout: String,
    stderr: String,
    error: String,
    exit: i32,
}
#[derive(Clone)]
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
    failure: String,
}
#[derive(Clone)]
struct SharedOutput(Arc<Mutex<Output>>);
impl Write for SharedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.failure.as_str() {
            "other" => Err(io::Error::other("fixture write failed")),
            "broken" => Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe")),
            "short" => {
                let count = bytes.len().min(10_usize.saturating_sub(self.bytes.len()));
                self.bytes.extend_from_slice(&bytes[..count]);
                Err(io::Error::other("short write"))
            }
            _ => {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn search_output_matches_go_records_streaming_partial_results_and_writer_failures() {
    let sources: Vec<Source> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-pages.json"
    ))
    .unwrap();
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-output.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 120);
    for case in cases {
        for saved in [false, true] {
            let source = &sources[case.source];
            let mut options = SearchOptions::default();
            for (key, value) in &source.flags {
                match key.as_str() {
                    "rating" => options.rating = value.clone(),
                    "content-type" => options.content_type = value.clone(),
                    "limit" => options.limit = Some(value.parse().unwrap()),
                    "page" => options.page = Some(value.parse().unwrap()),
                    _ => panic!("unexpected flag"),
                }
            }
            let queries = Arc::new(Mutex::new(vec![]));
            let transport = Fixture {
                bodies: source.bodies.clone(),
                queries: queries.clone(),
            };
            let request = options
                .request(
                    "cat",
                    DateTime::parse_from_rfc3339("2024-02-29T12:00:00+09:00").unwrap(),
                )
                .unwrap();
            let ndjson = case.mode == "ndjson";
            let mode = if case.mode == "json" {
                DetailOutput::Json
            } else if ndjson {
                DetailOutput::Ndjson
            } else {
                DetailOutput::Human
            };
            let mut out = SharedOutput(Arc::new(Mutex::new(Output {
                bytes: vec![],
                failure: case.failure.clone(),
            })));
            let result = if saved {
                let accounts = saved_account::saved_execution(transport);
                saved_artwork_search(
                    &accounts.execution,
                    &pixiv_app::lifecycle::Context::new(),
                    request,
                    options,
                    None,
                    mode,
                    out.clone(),
                )
                .await
            } else {
                let client = Client::with_transport("fixture-access", transport);
                artwork_search(&client, request, &options, mode, &mut out).await
            };
            assert_eq!(
                result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                case.error,
                "source {} {} {}",
                case.source,
                case.mode,
                case.failure
            );
            let mut diagnostics = vec![];
            assert_eq!(
                finish_command(result, ndjson, case.mode != "human", &mut diagnostics),
                case.exit
            );
            assert_eq!(*queries.lock().unwrap(), case.queries);
            let actual = String::from_utf8(out.0.lock().unwrap().bytes.clone()).unwrap();
            if case.mode == "json" && !actual.is_empty() && case.failure != "short" {
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
        }
    }
}
