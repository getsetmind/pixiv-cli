use clap::{CommandFactory, FromArgMatches, Parser};
use pixiv_cli_rs::{
    finish_command,
    search::{SearchInput, SearchOptions},
    trending::{saved_trending_tags, trending_tags},
};
use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
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
    body: serde_json::Value,
    requests: usize,
    error: String,
    stdout: String,
    stderr: String,
    exit: i32,
    args: Vec<String>,
    startup_stderr: String,
    startup_exit: i32,
    config: bool,
    database: bool,
}

#[derive(Clone)]
struct Fixture {
    body: serde_json::Value,
    requests: Arc<AtomicUsize>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(
            request.url,
            "https://app-api.pixiv.net/v1/trending-tags/illust"
        );
        assert!(request.parameters.is_empty());
        self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}

#[tokio::test]
async fn trending_data_and_output_match_go_through_sdk_and_saved_accounts() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/trending-tags.json"
    ))
    .unwrap();
    for case in cases {
        for saved in [false, true] {
            let matches = Arguments::command()
                .try_get_matches_from(std::iter::once("pixiv".to_owned()).chain(case.args.clone()))
                .unwrap();
            let mut args = Arguments::from_arg_matches(&matches).unwrap();
            args.input.record_flag_presence(&matches);
            let requests = Arc::new(AtomicUsize::new(0));
            let transport = Fixture {
                body: case.body.clone(),
                requests: requests.clone(),
            };
            let mut output = vec![];
            let result = match args
                .input
                .validate_trending_arguments()
                .and_then(|()| args.input.validate_trending_flags())
            {
                Err(error) => Err(error),
                Ok(()) => {
                    if saved {
                        let accounts = saved_account::saved_execution(transport);
                        saved_trending_tags(
                            &accounts.execution,
                            &pixiv_app::lifecycle::Context::new(),
                            None,
                            args.input.json.unwrap_or(false),
                            &mut output,
                        )
                        .await
                    } else {
                        let client = Client::with_transport("fixture-access", transport);
                        trending_tags(&client, args.input.json.unwrap_or(false), &mut output).await
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
                "{:?}",
                case.args
            );
            assert_eq!(requests.load(Ordering::SeqCst), case.requests);
            let mut diagnostics = vec![];
            assert_eq!(
                finish_command(result, false, args.input.machine_output(), &mut diagnostics),
                case.exit
            );
            if args.input.json == Some(true) && !output.is_empty() {
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
                    serde_json::from_str::<serde_json::Value>(&case.stdout).unwrap()
                );
            } else {
                assert_eq!(output, case.stdout.as_bytes());
            }
            if args.input.machine_output() && !diagnostics.is_empty() {
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&diagnostics).unwrap(),
                    serde_json::from_str::<serde_json::Value>(&case.stderr).unwrap()
                );
            } else {
                assert_eq!(diagnostics, case.stderr.as_bytes());
            }
        }
    }
}

#[test]
fn trending_process_matches_go_no_word_and_explicit_flag_restrictions() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/trending-tags.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 70);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .arg("search")
            .args(&case.args)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PIXIV_ACCESS_TOKEN", "")
            .env_remove("https_proxy")
            .env("HTTPS_PROXY", "")
            .env("REQUEST_INTERVAL", "0")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(case.startup_exit),
            "{:?}",
            case.args
        );
        assert!(output.stdout.is_empty());
        if case.startup_stderr.starts_with('{') {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
                serde_json::from_str::<serde_json::Value>(&case.startup_stderr).unwrap()
            );
        } else {
            assert_eq!(
                output.stderr,
                case.startup_stderr.as_bytes(),
                "{:?}",
                case.args
            );
        }
        let directory = home.path().join(".pixiv-cli");
        assert_eq!(directory.join("config.toml").exists(), case.config);
        assert_eq!(directory.join("pixiv-cli.db").exists(), case.database);
    }
}
