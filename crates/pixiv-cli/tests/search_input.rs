use clap::Parser;
use pixiv_cli_rs::{
    finish_command,
    search::{SearchInput, SearchOptions, artwork_search},
};
use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

#[derive(Parser)]
#[command(args_override_self = true)]
struct Arguments {
    #[command(flatten)]
    input: SearchInput,
}

#[derive(Deserialize)]
struct Case {
    args: Vec<String>,
    configured_json: bool,
    piped: bool,
    word: String,
    stdout: String,
    stderr: String,
    exit: i32,
    startup_stderr: String,
    startup_exit: i32,
    database: bool,
}

#[test]
fn search_process_accepts_go_words_boolean_overrides_and_usage_diagnostics() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-input.json"
    ))
    .unwrap();
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".pixiv-cli");
        std::fs::create_dir(&directory).unwrap();
        let config = format!("output_json = {}\n", case.configured_json);
        let path = directory.join("config.toml");
        std::fs::write(&path, &config).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pixiv"))
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
                serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap_or_else(
                    |error| panic!(
                        "{:?}: {error}: {}",
                        case.args,
                        String::from_utf8_lossy(&output.stderr)
                    )
                ),
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
        assert_eq!(
            directory.join("pixiv-cli.db").exists(),
            case.database,
            "{:?}",
            case.args
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), config);
    }
}

struct Fixture(Arc<Mutex<String>>, serde_json::Value);
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/illust");
        *self.0.lock().unwrap() = request
            .parameters
            .iter()
            .find(|(key, _)| key == "word")
            .unwrap()
            .1
            .clone();
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.1.clone(),
        })
    }
}

#[tokio::test]
async fn search_words_and_output_overrides_match_go_for_terminal_and_pipe() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-input.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 64);
    let sources: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-pages.json"
    ))
    .unwrap();
    for case in cases {
        let args =
            Arguments::try_parse_from(std::iter::once("pixiv".to_owned()).chain(case.args.clone()))
                .unwrap();
        let input = args.input;
        let word = Arc::new(Mutex::new(String::new()));
        let client = Client::with_transport(
            "fixture-access",
            Fixture(word.clone(), sources[0]["bodies"][0].clone()),
        );
        let mut output = vec![];
        let result = match input.output_mode(case.configured_json, !case.piped) {
            Ok(mode) => {
                let options = SearchOptions::default();
                let request = options
                    .request(&input.word(), chrono::Utc::now().fixed_offset())
                    .unwrap();
                artwork_search(&client, request, &options, mode, &mut output).await
            }
            Err(error) => Err(error),
        };
        let mut diagnostics = vec![];
        assert_eq!(
            finish_command(
                result,
                input.ndjson,
                input.machine_output(),
                &mut diagnostics
            ),
            case.exit,
            "{:?}",
            case.args
        );
        assert_eq!(*word.lock().unwrap(), case.word, "{:?}", case.args);
        let actual = String::from_utf8(output).unwrap();
        if input.output_mode(case.configured_json, !case.piped).ok()
            == Some(pixiv_cli_rs::DetailOutput::Json)
        {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&actual).unwrap(),
                serde_json::from_str::<serde_json::Value>(&case.stdout).unwrap()
            );
        } else if input.output_mode(case.configured_json, !case.piped).ok()
            == Some(pixiv_cli_rs::DetailOutput::Ndjson)
        {
            let parse = |value: &str| {
                value
                    .lines()
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                    .collect::<Vec<_>>()
            };
            assert_eq!(parse(&actual), parse(&case.stdout));
        } else {
            assert_eq!(actual, case.stdout, "{:?}", case.args);
        }
        if case.exit != 2 && input.machine_output() && !diagnostics.is_empty() {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&diagnostics).unwrap(),
                serde_json::from_str::<serde_json::Value>(&case.stderr).unwrap()
            );
        } else {
            assert_eq!(diagnostics, case.stderr.as_bytes());
        }
    }
}
