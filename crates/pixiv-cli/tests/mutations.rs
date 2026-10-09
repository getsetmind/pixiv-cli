use clap::{Parser, Subcommand};
use pixiv_cli_rs::{
    CommandError, finish_command,
    mutation::{BookmarkCommand, FollowCommand, Mutation, mutation, saved_mutation},
};
use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[path = "support/saved_account.rs"]
mod saved_account;
#[derive(Parser)]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Bookmark {
        #[command(subcommand)]
        command: BookmarkCommand,
    },
    Follow {
        #[command(subcommand)]
        command: FollowCommand,
    },
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
}
#[derive(Subcommand)]
enum UserCommand {
    Follow {
        #[command(subcommand)]
        command: FollowCommand,
    },
}
#[derive(Deserialize)]
struct Case {
    #[serde(default)]
    startup_only: bool,
    #[serde(default)]
    startup_unverified: String,
    args: Vec<String>,
    input: String,
    status: u16,
    paths: Vec<String>,
    forms: Vec<BTreeMap<String, Vec<String>>>,
    error: String,
    stdout: String,
    stderr: String,
    exit: i32,
    startup_stdout: String,
    startup_stderr: String,
    startup_exit: i32,
    config: bool,
    database: bool,
}
type Captured = Arc<Mutex<Vec<(String, BTreeMap<String, Vec<String>>)>>>;
#[derive(Clone)]
struct Fixture {
    status: u16,
    captured: Captured,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "POST");
        let mut form = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            form.entry(key).or_default().push(value);
        }
        self.captured.lock().unwrap().push((
            request
                .url
                .strip_prefix("https://app-api.pixiv.net")
                .unwrap()
                .into(),
            form,
        ));
        Ok(Response {
            status: self.status,
            retry_after: None,
            body: serde_json::json!({}),
        })
    }
}
fn parse(args: &[String]) -> Result<Mutation, CommandError> {
    match Arguments::try_parse_from(std::iter::once("pixiv".to_owned()).chain(args.to_owned())) {
        Ok(Arguments { command }) => Ok(match command {
            Command::Bookmark { command } => Mutation::Bookmark(command),
            Command::Follow { command }
            | Command::User {
                command: UserCommand::Follow { command },
            } => Mutation::Follow(command),
        }),
        Err(error) => {
            if error.kind() == clap::error::ErrorKind::UnknownArgument {
                let invalid = error
                    .get(clap::error::ContextKind::InvalidArg)
                    .map(ToString::to_string)
                    .unwrap_or_default();
                if invalid.starts_with('-') && !invalid.starts_with("--") {
                    return Err(CommandError::MessageText(format!(
                        "unknown shorthand flag: '{}' in {invalid}",
                        invalid.chars().nth(1).unwrap()
                    )));
                }
            }
            if let Some(error) = pixiv_cli_rs::mutation::argument_error(&error) {
                return Err(error);
            }
            panic!("unexpected mutation parse error: {error}");
        }
    }
}
fn compare_diagnostics(actual: &[u8], expected: &str) {
    if expected.starts_with('{') {
        let actual: Vec<serde_json::Value> = std::str::from_utf8(actual)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let expected: Vec<serde_json::Value> = expected
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(actual, expected);
    } else {
        assert_eq!(actual, expected.as_bytes());
    }
}
#[tokio::test]
async fn bookmark_follow_preserve_frozen_go_cli_and_saved_account_contracts() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/cli-bookmark-follow.json")).unwrap();
    assert_eq!(cases.len(), 278);
    for case in cases {
        if case.startup_only {
            continue;
        }
        for saved in [false, true] {
            let captured = Arc::new(Mutex::new(vec![]));
            let fixture = Fixture {
                status: case.status,
                captured: captured.clone(),
            };
            let mut diagnostics = vec![];
            let mut input = std::io::Cursor::new(case.input.as_bytes());
            let result = match parse(&case.args) {
                Err(error) => Err(error),
                Ok(action) => {
                    if saved {
                        let account = saved_account::saved_execution(fixture);
                        saved_mutation(
                            &account.execution,
                            &pixiv_app::lifecycle::Context::new(),
                            action,
                            None,
                            &mut input,
                            false,
                            &mut diagnostics,
                        )
                        .await
                    } else {
                        mutation(
                            &Client::with_transport("fixture-access", fixture),
                            action,
                            &mut input,
                            false,
                            &mut diagnostics,
                        )
                        .await
                    }
                }
            };
            assert_eq!(
                result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                case.error,
                "{:?}, saved={saved}",
                case.args
            );
            let exit = finish_command(result, false, false, &mut diagnostics);
            assert_eq!(exit, case.exit, "{:?}, saved={saved}", case.args);
            compare_diagnostics(&diagnostics, &case.stderr);
            assert!(case.stdout.is_empty());
            let captured = captured.lock().unwrap();
            assert_eq!(
                captured
                    .iter()
                    .map(|(path, _)| path.clone())
                    .collect::<Vec<_>>(),
                case.paths,
                "{:?}, saved={saved}",
                case.args
            );
            assert_eq!(
                captured
                    .iter()
                    .map(|(_, form)| form.clone())
                    .collect::<Vec<_>>(),
                case.forms,
                "{:?}, saved={saved}",
                case.args
            );
        }
    }
}
#[tokio::test]
async fn read_error_discards_partial_record_before_mutation() {
    struct Partial {
        first: bool,
    }
    impl std::io::Read for Partial {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            unreachable!()
        }
    }
    impl std::io::BufRead for Partial {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            if self.first {
                Ok(br#"{"id":"42","type":"user","url":"anything"}"#)
            } else {
                Err(std::io::Error::other("fixture read failed"))
            }
        }
        fn consume(&mut self, _: usize) {
            self.first = false;
        }
    }
    let captured = Arc::new(Mutex::new(vec![]));
    let client = Client::with_transport(
        "fixture-access",
        Fixture {
            status: 200,
            captured: captured.clone(),
        },
    );
    let action = parse(&["follow".into(), "add".into()]).unwrap();
    let mut diagnostics = vec![];
    let error = mutation(
        &client,
        action,
        &mut Partial { first: true },
        false,
        &mut diagnostics,
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "read NDJSON input: fixture read failed");
    assert!(captured.lock().unwrap().is_empty());
    assert!(diagnostics.is_empty());
}

#[tokio::test]
async fn saved_pipeline_cancellation_stops_before_read_and_after_first_failed_invoke() {
    use pixiv_app::lifecycle::Context;
    struct CountRead {
        input: std::io::Cursor<Vec<u8>>,
        reads: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl std::io::Read for CountRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            unreachable!()
        }
    }
    impl std::io::BufRead for CountRead {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::io::BufRead::fill_buf(&mut self.input)
        }
        fn consume(&mut self, n: usize) {
            std::io::BufRead::consume(&mut self.input, n);
        }
    }
    #[derive(Clone)]
    struct Cancel {
        context: Context,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Transport for Cancel {
        async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.context.cancel();
            Err(pixiv_sdk::Error::new(
                pixiv_sdk::Reason::UpstreamUnavailable,
                "FollowUser",
            ))
        }
    }
    for before in [true, false] {
        let context = Context::new();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let account = saved_account::saved_execution(Cancel {
            context: context.clone(),
            calls: calls.clone(),
        });
        let mut input=CountRead {input:std::io::Cursor::new(b"{\"id\":\"42\",\"type\":\"user\",\"url\":\"anything\"}\n{\"id\":\"43\",\"type\":\"user\",\"url\":\"anything\"}\n".to_vec()),reads:reads.clone()};
        let action = parse(&["follow".into(), "add".into()]).unwrap();
        if before {
            context.cancel();
        }
        let mut diagnostics = vec![];
        let error = saved_mutation(
            &account.execution,
            &context,
            action,
            None,
            &mut input,
            false,
            &mut diagnostics,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "context canceled");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(!before)
        );
        assert_eq!(
            reads.load(std::sync::atomic::Ordering::SeqCst),
            if before { 0 } else { 2 }
        );
        assert!(diagnostics.is_empty());
    }
}

#[test]
fn real_mutation_cli_preserves_go_isolated_startup() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/cli-bookmark-follow.json")).unwrap();
    assert_eq!(
        cases
            .iter()
            .filter(|case| !case.startup_unverified.is_empty())
            .count(),
        3
    );
    for case in cases {
        if !case.startup_unverified.is_empty() {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .args(&case.args)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("REQUEST_INTERVAL", "0")
            .env_remove("HTTPS_PROXY")
            .env_remove("https_proxy")
            .env_remove("PIXIV_ACCESS_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(case.input.as_bytes()).unwrap();
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(case.startup_exit),
            "{:?} input={:?}",
            case.args,
            case.input
        );
        assert_eq!(
            output.stdout,
            case.startup_stdout.as_bytes(),
            "{:?}",
            case.args
        );
        compare_diagnostics(&output.stderr, &case.startup_stderr);
        assert_eq!(
            home.path().join(".pixiv-cli/config.toml").exists(),
            case.config,
            "{:?}",
            case.args
        );
        assert_eq!(
            home.path().join(".pixiv-cli/pixiv-cli.db").exists(),
            case.database,
            "{:?}",
            case.args
        );
    }
}
