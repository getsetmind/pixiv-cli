use clap::Parser;
use pixiv_cli_rs::novel_series::NovelSeriesOptions;
use serde::Deserialize;
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};

#[derive(Deserialize)]
struct Case {
    args: Vec<String>,
    input: String,
    word: String,
    bytes: usize,
    error: String,
    stderr: String,
    exit: i32,
    config: bool,
    database: bool,
}

#[derive(Parser)]
#[command(args_override_self = true)]
struct Arguments {
    #[command(flatten)]
    input: NovelSeriesOptions,
}

struct Reader<'a> {
    input: &'a [u8],
    bytes: usize,
    allowed: bool,
}

impl Read for Reader<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        assert!(self.allowed, "explicit words must not consume stdin");
        let count = self.input.read(output)?;
        self.bytes += count;
        Ok(count)
    }
}

#[test]
fn artwork_series_stdin_resolution_matches_go_without_trimming_splitting_or_reading_explicit_words()
{
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-artwork-series-stdin.json"
    ))
    .unwrap();
    for case in cases {
        let mut args =
            Arguments::try_parse_from(std::iter::once("pixiv".to_owned()).chain(case.args))
                .unwrap();
        let mut reader = Reader {
            input: case.input.as_bytes(),
            bytes: 0,
            allowed: args.input.sources.is_empty(),
        };
        let result = args
            .input
            .resolve_source(&mut reader, false)
            .and_then(|_| args.input.validate())
            .map(|_| args.input.sources.join(" "));
        assert_eq!(reader.bytes, case.bytes);
        match result {
            Ok(word) => {
                assert!(case.error.is_empty());
                assert_eq!(word, case.word);
            }
            Err(error) => assert_eq!(error.to_string(), case.error),
        }
    }
}

#[test]
fn artwork_series_process_preserves_go_stdin_usage_and_startup_side_effects() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-artwork-series-stdin.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 99);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .arg("series")
            .args(&case.args)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PIXIV_ACCESS_TOKEN", "")
            .env_remove("https_proxy")
            .env("HTTPS_PROXY", "")
            .env("REQUEST_INTERVAL", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(case.input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(case.exit),
            "{:?} {:?}",
            case.args,
            case.input
        );
        assert!(output.stdout.is_empty());
        if case.stderr.starts_with('{') {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
                serde_json::from_str::<serde_json::Value>(&case.stderr).unwrap()
            );
        } else {
            assert_eq!(
                output.stderr,
                case.stderr.as_bytes(),
                "{:?} {:?}",
                case.args,
                case.input
            );
        }
        let directory = home.path().join(".pixiv-cli");
        assert_eq!(directory.join("config.toml").exists(), case.config);
        assert_eq!(directory.join("pixiv-cli.db").exists(), case.database);
    }
}
