use clap::Parser;
use pixiv_cli_rs::{
    DetailOutput, finish_command,
    novel_series::{NovelSeriesOptions, novel_series, saved_novel_series},
};
use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    process::Command,
    sync::{Arc, Mutex},
};
#[path = "support/saved_account.rs"]
mod saved_account;

#[derive(Parser)]
#[command(args_override_self = true)]
struct Arguments {
    #[command(flatten)]
    options: NovelSeriesOptions,
}
#[derive(Deserialize)]
struct Case {
    args: Vec<String>,
    bodies: Vec<Value>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    error: String,
    stdout: String,
    stderr: String,
    exit: i32,
    startup_stderr: String,
    startup_exit: i32,
    config: bool,
    database: bool,
}
type Queries = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
#[derive(Clone)]
struct Fixture {
    bodies: Vec<Value>,
    queries: Queries,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v2/novel/series");
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let index = usize::from(self.bodies.len() > 1 && query.contains_key("last_order"));
        self.queries.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}
fn equal_output(actual: &[u8], expected: &str) {
    if expected.starts_with('{') {
        assert_eq!(
            serde_json::from_slice::<Value>(actual).unwrap(),
            serde_json::from_str::<Value>(expected).unwrap()
        );
    } else {
        assert_eq!(actual, expected.as_bytes());
    }
}
#[tokio::test]
async fn series_preserves_go_output_windows_errors_and_saved_account_execution() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-novel-series.json"
    ))
    .unwrap();
    for case in cases {
        for saved in [false, true] {
            let options = Arguments::try_parse_from(
                std::iter::once("pixiv".to_owned()).chain(case.args.clone()),
            )
            .unwrap()
            .options;
            let queries = Arc::new(Mutex::new(vec![]));
            let transport = Fixture {
                bodies: case.bodies.clone(),
                queries: queries.clone(),
            };
            let mode = if options.ndjson {
                DetailOutput::Ndjson
            } else if options.json == Some(true) {
                DetailOutput::Json
            } else {
                DetailOutput::Human
            };
            let output = Arc::new(Mutex::new(vec![]));
            struct Writer(Arc<Mutex<Vec<u8>>>);
            impl std::io::Write for Writer {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    self.0.lock().unwrap().extend_from_slice(bytes);
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let validation = options
                .validate()
                .and_then(|_| options.output_mode(false, true).map(|_| ()));
            let result = if let Err(error) = validation {
                Err(error)
            } else if saved {
                let account = saved_account::saved_execution(transport);
                saved_novel_series(
                    &account.execution,
                    &pixiv_app::lifecycle::Context::new(),
                    options.clone(),
                    None,
                    mode,
                    Writer(output.clone()),
                )
                .await
            } else {
                novel_series(
                    &Client::with_transport("fixture-access", transport),
                    &options,
                    mode,
                    &mut Writer(output.clone()),
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
                "{:?}",
                case.args
            );
            let mut diagnostics = vec![];
            assert_eq!(
                finish_command(
                    result,
                    mode == DetailOutput::Ndjson,
                    mode != DetailOutput::Human,
                    &mut diagnostics
                ),
                case.exit
            );
            if mode == DetailOutput::Ndjson && !case.stdout.is_empty() {
                let parse = |bytes: &[u8]| {
                    String::from_utf8(bytes.to_vec())
                        .unwrap()
                        .lines()
                        .map(|line| serde_json::from_str::<Value>(line).unwrap())
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    parse(&output.lock().unwrap()),
                    parse(case.stdout.as_bytes())
                );
            } else {
                equal_output(&output.lock().unwrap(), &case.stdout);
            }
            equal_output(&diagnostics, &case.stderr);
            assert_eq!(*queries.lock().unwrap(), case.queries, "{:?}", case.args);
        }
    }
}
#[test]
fn series_process_preserves_go_validation_authentication_and_startup_order() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-novel-series.json"
    ))
    .unwrap();
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .arg("series")
            .args(&case.args)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("HTTPS_PROXY", "")
            .env("REQUEST_INTERVAL", "0")
            .env("PIXIV_ACCESS_TOKEN", "")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(case.startup_exit),
            "{:?}",
            case.args
        );
        assert!(output.stdout.is_empty());
        equal_output(&output.stderr, &case.startup_stderr);
        assert_eq!(
            home.path().join(".pixiv-cli/config.toml").exists(),
            case.config
        );
        assert_eq!(
            home.path().join(".pixiv-cli/pixiv-cli.db").exists(),
            case.database
        );
    }
}
