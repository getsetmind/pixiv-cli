use pixiv_app::{
    download::{DownloadSaveClient, SaveFuture},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_cli_rs::{
    CommandError,
    download::{DownloadCommand, DownloadRuntime, DownloadSinks},
};
use pixiv_sdk::resource::ResourceRef;
use pixiv_sdk::transport::{Request, Response, Transport};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[path = "../src/record_input/json.rs"]
mod record_json;

#[derive(Clone)]
struct UnusedTransport;

struct UnusedSaveClient;

struct CancelingRecordReader {
    input: std::io::Cursor<Vec<u8>>,
    context: Context,
    armed: bool,
    canceled: bool,
}

impl std::io::Read for CancelingRecordReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.armed && !self.canceled {
            self.context.cancel();
            self.canceled = true;
        }
        std::io::Read::read(&mut self.input, buffer)
    }
}

impl DownloadSaveClient for UnusedSaveClient {
    fn save_ref(&self, _: Context, _: ResourceRef, _: std::path::PathBuf) -> SaveFuture<'_> {
        panic!("rejected input must not save a resource")
    }

    fn save_url(&self, _: Context, _: String, _: std::path::PathBuf) -> SaveFuture<'_> {
        panic!("rejected input must not save a URL")
    }
}

impl Transport for UnusedTransport {
    async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
        panic!("rejected input must not make an SDK request")
    }
}

fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn record_parser_exposed_fields_match_each_frozen_go_physical_line() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_record_codec.json")).unwrap();
    let mut compared = 0;
    for row in fixture["cases"].as_array().unwrap() {
        let input = bytes(row["input_hex"].as_str().unwrap());
        let lines: Vec<_> = input.split_inclusive(|byte| *byte == b'\n').collect();
        let expected = row["actual"]["lines"].as_array().unwrap();
        assert_eq!(lines.len(), expected.len(), "{}", row["name"]);
        for (line, expected) in lines.into_iter().zip(expected) {
            compared += 1;
            let observed = record_json::parse_go_line(line);
            let label = format!("{} physical line {}", row["name"], expected["line"]);
            if expected["parse_error"] == "" {
                let (id, typ, url) = observed.unwrap_or_else(|error| panic!("{label}: {error}"));
                assert_eq!(id, expected["value"]["id"], "{label}");
                assert_eq!(typ, expected["value"]["type"], "{label}");
                assert_eq!(url, expected["value"]["url"], "{label}");
            } else {
                assert_eq!(
                    observed.unwrap_err(),
                    expected["parse_error"].as_str().unwrap(),
                    "{label}"
                );
            }
        }
    }
    assert_eq!(compared, 217);
}

#[test]
fn record_parser_diagnostic_identity_matches_frozen_go_raw_first_values() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_record_codec.json")).unwrap();
    let mut compared = 0;
    for row in fixture["cases"].as_array().unwrap() {
        let input = bytes(row["input_hex"].as_str().unwrap());
        for (line, expected) in input
            .split_inclusive(|byte| *byte == b'\n')
            .zip(row["actual"]["lines"].as_array().unwrap())
        {
            compared += 1;
            let (id, typ) = record_json::diagnostic_identity(line);
            let label = format!("{} physical line {}", row["name"], expected["line"]);
            assert_eq!(id, expected["identity_id"], "{label}");
            assert_eq!(typ, expected["identity_type"], "{label}");
        }
    }
    assert_eq!(compared, 217);
}

#[tokio::test]
async fn parsed_download_validation_preserves_frozen_cancellation_during_record_reads() {
    let fixture: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/download_record_read_cancel.json")).unwrap();
    assert_eq!(fixture.len(), 2);
    let mut differences = Vec::new();
    for row in fixture {
        let context = Context::new();
        let mut input = CancelingRecordReader {
            input: std::io::Cursor::new(bytes(row["input_hex"].as_str().unwrap())),
            context: context.clone(),
            armed: false,
            canceled: false,
        };
        let command = DownloadCommand::parse(&["download".into()], &mut input, false).unwrap();
        assert_eq!(
            input.input.position(),
            row["classifier_read_bytes"].as_u64().unwrap()
        );
        assert!(!input.canceled);
        input.armed = true;
        let stdout = Arc::new(Mutex::new(Vec::<u8>::new()));
        let stderr = Arc::new(Mutex::new(Vec::<u8>::new()));
        let opened = std::cell::Cell::new(false);
        let result = command
            .execute_with_factory(
                &context,
                || {
                    Ok(DownloadRuntime {
                        download_path: "unused-record-fixture-directory".into(),
                        ..Default::default()
                    })
                },
                &mut input,
                DownloadSinks {
                    output: stdout.clone(),
                    error: stderr.clone(),
                },
                || -> Result<Execution<UnusedTransport>, CommandError> {
                    opened.set(true);
                    Err(CommandError::Message("fixture unexpected execution"))
                },
                |_| Arc::new(UnusedSaveClient),
            )
            .await;
        let error = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        let canceled = matches!(&result, Err(CommandError::App(error)) if error.is_canceled());
        let actual_stderr = String::from_utf8(stderr.lock().unwrap().clone()).unwrap();
        if error != row["error"].as_str().unwrap()
            || canceled != row["canceled"].as_bool().unwrap()
            || actual_stderr != row["stderr"].as_str().unwrap()
            || input.canceled != row["canceled_during_read"].as_bool().unwrap()
            || !stdout.lock().unwrap().is_empty()
            || opened.get()
        {
            differences.push(format!("{}: error={error:?}, canceled={canceled}, stderr={actual_stderr:?}, execution_opened={}", row["name"], opened.get()));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[tokio::test]
async fn rejected_download_records_match_frozen_go_before_opening_an_execution() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_record_codec.json")).unwrap();
    assert_eq!(
        fixture["frozen_go"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    let mut compared = 0;
    let mut differences = Vec::new();
    for row in fixture["cases"].as_array().unwrap() {
        let input_bytes = bytes(row["input_hex"].as_str().unwrap());
        if row["boundary"] != "action"
            || row["reader"] != "plain"
            || row["writer"] != "plain"
            || row["cancellation"] != ""
            || row["action_failure"] != ""
            || row["actual"]["pipeline"] != true
            || !row["actual"]["action_ids"].as_array().unwrap().is_empty()
            || input_bytes
                .iter()
                .find(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
                != Some(&b'{')
        {
            continue;
        }
        compared += 1;
        let name = row["name"].as_str().unwrap();
        let mut input = std::io::Cursor::new(input_bytes);
        let args = vec![
            "download".to_owned(),
            format!("--on-error={}", row["on_error"].as_str().unwrap()),
        ];
        let command = DownloadCommand::parse(&args, &mut input, false).unwrap();
        let stdout = Arc::new(Mutex::new(Vec::<u8>::new()));
        let stderr = Arc::new(Mutex::new(Vec::<u8>::new()));
        let opened = std::cell::Cell::new(false);
        let result = command
            .execute_with_factory(
                &Context::new(),
                || {
                    Ok(DownloadRuntime {
                        download_path: "unused-record-fixture-directory".into(),
                        ..Default::default()
                    })
                },
                &mut input,
                DownloadSinks {
                    output: stdout.clone(),
                    error: stderr.clone(),
                },
                || -> Result<Execution<UnusedTransport>, CommandError> {
                    opened.set(true);
                    Err(CommandError::Message("fixture unexpected execution"))
                },
                |_| Arc::new(UnusedSaveClient),
            )
            .await;
        let actual_stderr = String::from_utf8(stderr.lock().unwrap().clone()).unwrap();
        let actual_error = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        let pipeline = matches!(result, Err(CommandError::Pipeline));
        if !stdout.lock().unwrap().is_empty()
            || actual_stderr != row["actual"]["stderr"].as_str().unwrap()
            || actual_error != row["actual"]["error"].as_str().unwrap()
            || pipeline != row["actual"]["pipeline"].as_bool().unwrap()
            || opened.get()
        {
            differences.push(format!(
                "{name}: execution_opened={}, pipeline={pipeline}, error={actual_error:?}, stderr={actual_stderr:?}; frozen stderr={:?}",
                opened.get(),
                row["actual"]["stderr"].as_str().unwrap(),
            ));
        }
    }
    assert_eq!(compared, 77);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
