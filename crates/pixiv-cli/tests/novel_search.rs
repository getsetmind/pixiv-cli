use clap::{CommandFactory, FromArgMatches, Parser};
use pixiv_cli_rs::{
    DetailOutput, finish_command,
    novel_search::{novel_search, saved_novel_search},
    search::{SearchInput, SearchOptions},
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
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/novel");
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
#[tokio::test]
async fn novel_search_preserves_go_output_windows_errors_and_saved_account_execution() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-novel-search.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 273);
    compare_output(cases, false).await;
}

#[tokio::test]
async fn novel_search_compatibility_preserves_go_outputs_through_saved_accounts() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-novel-search-compat.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 99);
    let accepted: Vec<_> = cases
        .into_iter()
        .filter(|case| {
            !case.args.iter().any(|arg| {
                matches!(
                    arg.as_str(),
                    "--type=novel" | "--rating=" | "--trending-tags"
                )
            })
        })
        .collect();
    assert_eq!(accepted.len(), 90);
    compare_output(accepted, true).await;
}

async fn compare_output(cases: Vec<Case>, compatibility: bool) {
    #[derive(Parser)]
    #[command(args_override_self = true)]
    struct CompatibilityArguments {
        #[command(flatten)]
        options: pixiv_cli_rs::novel_search::NovelSearchOptions,
    }
    for case in cases {
        for saved in [false, true] {
            let args = if compatibility {
                let args = CompatibilityArguments::try_parse_from(
                    std::iter::once("pixiv".to_owned()).chain(case.args.clone()),
                )
                .unwrap();
                let (input, options) = args.options.into_search();
                Arguments { input, options }
            } else {
                let matches = Arguments::command()
                    .try_get_matches_from(
                        std::iter::once("pixiv".to_owned()).chain(case.args.clone()),
                    )
                    .unwrap();
                let mut args = Arguments::from_arg_matches(&matches).unwrap();
                args.input.record_flag_presence(&matches);
                args
            };
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
            let result = if saved {
                let account = saved_account::saved_execution(transport);
                saved_novel_search(
                    &account.execution,
                    &pixiv_app::lifecycle::Context::new(),
                    (&input, &options, &word),
                    None,
                    mode,
                    Writer(output.clone()),
                )
                .await
            } else {
                novel_search(
                    &Client::with_transport("fixture-access", transport),
                    &input,
                    &options,
                    &word,
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
fn novel_search_process_preserves_go_validation_authentication_and_startup_order() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-novel-search.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 273);
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
