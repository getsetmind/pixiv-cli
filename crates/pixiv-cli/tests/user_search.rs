use clap::{CommandFactory, FromArgMatches, Parser};
use pixiv_cli_rs::{
    DetailOutput, finish_command,
    search::{SearchInput, SearchOptions},
    user_search::{saved_user_search, user_search},
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
    input: SearchInput,
    #[command(flatten)]
    options: SearchOptions,
}
#[derive(Deserialize)]
struct Case {
    writer: String,
    args: Vec<String>,
    bodies: Vec<Value>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    error: String,
    stdout: String,
    stderr: String,
    exit: i32,
    startup_stderr: String,
    startup_input: String,
    startup_args: Vec<String>,
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
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/user");
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let index = usize::from(self.bodies.len() > 1 && query.contains_key("offset"));
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
fn user_search_cases() -> Vec<Case> {
    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cli-user-search.json"
    ))
    .expect("capture the fixed Go CLI contract with TestMigrationUserSearchPreservesLogicalPagesOutputsAndStartup -migration-update-cli-user-search before running user-search comparisons");
    serde_json::from_str(&fixture).expect("fixed Go CLI user-search contract must be valid JSON")
}

#[tokio::test]
async fn user_search_preserves_go_output_windows_errors_and_saved_account_execution() {
    let cases = user_search_cases();
    assert_eq!(cases.len(), 279);
    compare_output(cases).await;
}

async fn compare_output(cases: Vec<Case>) {
    for case in cases {
        for saved in [false, true] {
            let matches = Arguments::command()
                .try_get_matches_from(std::iter::once("pixiv".to_owned()).chain(case.args.clone()))
                .unwrap();
            let mut args = Arguments::from_arg_matches(&matches).unwrap();
            args.input.record_flag_presence(&matches);
            let input = args.input;
            let options = args.options;
            let word = input.word();
            let queries = Arc::new(Mutex::new(vec![]));
            let transport = Fixture {
                bodies: case.bodies.clone(),
                queries: queries.clone(),
            };
            let mode = if input.ndjson {
                DetailOutput::Ndjson
            } else if input.json == Some(true) {
                DetailOutput::Json
            } else {
                DetailOutput::Human
            };
            let output = Arc::new(Mutex::new(vec![]));
            struct Writer(Arc<Mutex<Vec<u8>>>, String);
            impl std::io::Write for Writer {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    match self.1.as_str() {
                        "other" => Err(std::io::Error::other("fixture write failed")),
                        "broken" => Err(std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "broken pipe",
                        )),
                        "short" => {
                            let mut output = self.0.lock().unwrap();
                            let count = bytes.len().min(10_usize.saturating_sub(output.len()));
                            output.extend_from_slice(&bytes[..count]);
                            Err(std::io::Error::other("short write"))
                        }
                        _ => {
                            self.0.lock().unwrap().extend_from_slice(bytes);
                            Ok(bytes.len())
                        }
                    }
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let result = if saved {
                let account = saved_account::saved_execution(transport);
                saved_user_search(
                    &account.execution,
                    &pixiv_app::lifecycle::Context::new(),
                    (&input, &options, &word),
                    None,
                    mode,
                    Writer(output.clone(), case.writer.clone()),
                )
                .await
            } else {
                user_search(
                    &Client::with_transport("fixture-access", transport),
                    &input,
                    &options,
                    &word,
                    mode,
                    &mut Writer(output.clone(), case.writer.clone()),
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
            if case.writer == "short" {
                assert_eq!(*output.lock().unwrap(), case.stdout.as_bytes());
            } else if mode == DetailOutput::Ndjson && !case.stdout.is_empty() {
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
fn user_search_process_preserves_go_validation_authentication_and_startup_order() {
    let cases = user_search_cases();
    assert_eq!(cases.len(), 279);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command
            .arg("search")
            .args(&case.startup_args)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("HTTPS_PROXY", "")
            .env("REQUEST_INTERVAL", "0")
            .env("PIXIV_ACCESS_TOKEN", "")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            case.startup_input.as_bytes(),
        )
        .unwrap();
        let output = child.wait_with_output().unwrap();
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
