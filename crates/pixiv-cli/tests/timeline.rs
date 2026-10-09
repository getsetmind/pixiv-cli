use clap::{CommandFactory, Parser};
use pixiv_cli_rs::{
    CommandError, DetailOutput, finish_command,
    timeline::{FollowingOptions, Timeline, TimelineOptions, saved_timeline, timeline},
};
use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::{Arc, Mutex},
};
#[path = "support/json_object_order.rs"]
mod json_object_order;
#[path = "support/saved_account.rs"]
mod saved_account;
#[derive(Parser)]
#[command(args_override_self = true)]
struct FollowingArgs {
    #[command(flatten)]
    options: FollowingOptions,
    #[command(flatten)]
    proxy: Proxy,
}
#[derive(Parser)]
#[command(args_override_self = true)]
struct LatestArgs {
    #[command(flatten)]
    options: TimelineOptions,
    #[command(flatten)]
    proxy: Proxy,
}
#[derive(clap::Args)]
struct Proxy {
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long,num_args=0..=1,require_equals=true,default_missing_value="true")]
    no_proxy: Option<bool>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    operation: String,
    args: Vec<String>,
    source: String,
    mode: String,
    writer: String,
    input: String,
    read_error: bool,
    parser_only: bool,
    requests: Vec<Fetch>,
    commits: Vec<bool>,
    error: String,
    stdout: String,
    stderr: String,
    exit: i32,
    bytes: usize,
    proxy: Option<String>,
    json: Option<bool>,
}
#[derive(Debug, Deserialize, PartialEq)]
struct Fetch {
    path: String,
    query: BTreeMap<String, Vec<String>>,
}
#[derive(Clone)]
struct Fixture {
    bodies: Vec<Value>,
    requests: Arc<Mutex<Vec<Fetch>>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: serde_json::json!({"access_token":"fixture-access-42","refresh_token":"fixture-rotated-42","expires_in":3600,"user":{"id":42}}),
            });
        }
        assert_eq!(request.method.as_str(), "GET");
        assert!(
            request
                .headers
                .iter()
                .any(|(key, value)| key.eq_ignore_ascii_case("authorization")
                    && value == "Bearer fixture-access-42")
        );
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let mut requests = self.requests.lock().unwrap();
        let index = requests.len().min(self.bodies.len() - 1);
        requests.push(Fetch {
            path: request
                .url
                .strip_prefix("https://app-api.pixiv.net")
                .unwrap()
                .to_owned(),
            query,
        });
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}
struct Writer {
    out: Arc<Mutex<Vec<u8>>>,
    failure: String,
    written: Arc<Mutex<bool>>,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        *self.written.lock().unwrap() = true;
        let mut out = self.out.lock().unwrap();
        match self.failure.as_str() {
            "broken" => Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe")),
            "other" => Err(io::Error::other("fixture write failed")),
            "short" => {
                let count = bytes.len().min(10_usize.saturating_sub(out.len()));
                out.extend_from_slice(&bytes[..count]);
                Err(io::Error::other("short write"))
            }
            _ => {
                out.extend_from_slice(bytes);
                Ok(bytes.len())
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn equal(actual: &[u8], expected: &str, mode: DetailOutput, partial: bool) {
    if !partial && !expected.is_empty() && mode == DetailOutput::Json {
        assert_eq!(
            json_object_order::canonicalize(actual),
            json_object_order::canonicalize(expected.as_bytes())
        );
    } else if !partial && mode == DetailOutput::Ndjson {
        assert_eq!(
            json_object_order::canonicalize_ndjson(actual),
            json_object_order::canonicalize_ndjson(expected.as_bytes())
        );
    } else {
        assert_eq!(actual, expected.as_bytes());
    }
}
#[tokio::test]
async fn timeline_matches_go_filters_aliases_windows_outputs_validation_and_saved_execution() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-timeline.json"
    ))
    .unwrap();
    let sources: BTreeMap<String, Vec<Value>> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-timeline-bodies.json"
    ))
    .unwrap();
    let mut exact = 0;
    let mut rejection_only = 0;
    for case in cases {
        let parsed = if case.operation == "following" {
            FollowingArgs::try_parse_from(
                std::iter::once("pixiv".to_owned()).chain(case.args.clone()),
            )
            .map(|args| (Timeline::Following(args.options), args.proxy))
        } else {
            LatestArgs::try_parse_from(std::iter::once("pixiv".to_owned()).chain(case.args.clone()))
                .map(|args| (Timeline::Latest(args.options), args.proxy))
        };
        if case.parser_only {
            assert!(parsed.is_err(), "{}: leaf parser rejection only", case.name);
            assert!(case.requests.is_empty());
            assert!(case.commits.is_empty());
            assert_eq!(case.bytes, 0);
            rejection_only += 1;
            continue;
        }
        exact += 1;
        let (options, proxy) = parsed.unwrap_or_else(|error| panic!("{}: {error}", case.name));
        assert_eq!(case.bytes, 0, "{}", case.name);
        // Timeline's no-input contract leaves every byte and reader error untouched.
        assert!(case.input.is_empty() || case.name == "stdin-ignored");
        assert!(!case.read_error || case.name == "failed-reader-ignored");
        for saved in [false, true] {
            let requests = Arc::new(Mutex::new(vec![]));
            let fixture = Fixture {
                bodies: sources[&case.source].clone(),
                requests: requests.clone(),
            };
            let output = Arc::new(Mutex::new(vec![]));
            let written = Arc::new(Mutex::new(false));
            let mut writer = Writer {
                out: output.clone(),
                failure: case.writer.clone(),
                written: written.clone(),
            };
            let mode = match case.mode.as_str() {
                "json" => DetailOutput::Json,
                "ndjson" | "auto" => DetailOutput::Ndjson,
                _ => DetailOutput::Human,
            };
            let validation = options
                .validate()
                .and_then(|_| {
                    if proxy.proxy.is_some() && proxy.no_proxy.is_some() {
                        Err(CommandError::Message(
                            "use either --proxy or --no-proxy, not both",
                        ))
                    } else {
                        Ok(())
                    }
                })
                .and_then(|_| options.output_mode(false, case.mode != "auto").map(|_| ()));
            let result = if let Err(error) = validation {
                Err(error)
            } else if saved {
                let account = saved_account::saved_execution(fixture);
                saved_timeline(
                    &account.execution,
                    &pixiv_app::lifecycle::Context::new(),
                    options.clone(),
                    None,
                    mode,
                    writer,
                )
                .await
            } else {
                let credentials = pixiv_sdk::oauth::refresh(&fixture, "fixture-refresh-42")
                    .await
                    .unwrap();
                timeline(
                    &Client::from_credentials(&credentials, fixture),
                    &options,
                    mode,
                    &mut writer,
                )
                .await
            };
            assert_eq!(
                result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                case.error,
                "{} saved={saved}",
                case.name
            );
            if !case.commits.is_empty() && case.writer.is_empty() {
                assert_eq!(
                    case.commits,
                    vec![*written.lock().unwrap() && mode != DetailOutput::Json],
                    "{}",
                    case.name
                );
            }
            let mut diagnostics = vec![];
            assert_eq!(
                finish_command(
                    result,
                    case.mode == "ndjson" || case.mode == "auto",
                    case.mode != "human" && case.mode != "auto",
                    &mut diagnostics
                ),
                case.exit,
                "{}",
                case.name
            );
            equal(
                &output.lock().unwrap(),
                &case.stdout,
                mode,
                case.writer == "short",
            );
            if case.stderr.starts_with('{') {
                assert_eq!(
                    json_object_order::canonicalize(&diagnostics),
                    json_object_order::canonicalize(case.stderr.as_bytes()),
                    "{}",
                    case.name
                );
            } else {
                assert_eq!(diagnostics, case.stderr.as_bytes(), "{}", case.name);
            }
            assert_eq!(*requests.lock().unwrap(), case.requests, "{}", case.name);
            if !case.requests.is_empty() {
                assert_eq!(
                    case.proxy,
                    if proxy.no_proxy == Some(true) {
                        Some(String::new())
                    } else {
                        proxy.proxy.clone()
                    }
                );
                if !options.options().listing.ndjson {
                    assert_eq!(options.options().listing.json, case.json);
                }
            }
        }
    }
    assert_eq!((exact, rejection_only), (73, 2));
}

#[test]
fn timeline_production_parser_preserves_go_normalized_flag_values_and_rejections() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-timeline-flags.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 33);
    for case in cases {
        let arguments = case["args"].as_array().unwrap();
        assert_eq!(arguments[0], "following");
        let parsed = pixiv_cli_rs::timeline::configure_command(FollowingArgs::command())
            .try_get_matches_from(
                std::iter::once("pixiv")
                    .chain(arguments[1..].iter().map(|arg| arg.as_str().unwrap())),
            );
        if let Some(expected) = case["parsed_value"].as_str() {
            let matches = parsed.unwrap_or_else(|error| panic!("{}: {error}", case["name"]));
            let flag = case["name"].as_str().unwrap().split(':').next().unwrap();
            let actual = if matches!(flag, "limit" | "page") {
                matches.get_one::<i64>(flag).unwrap().to_string()
            } else {
                matches.get_one::<bool>(flag).unwrap().to_string()
            };
            assert_eq!(actual, expected, "{}", case["name"]);
        } else {
            assert!(parsed.is_err(), "{}", case["name"]);
        }
    }
}
