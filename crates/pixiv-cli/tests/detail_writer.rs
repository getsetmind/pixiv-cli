use pixiv_cli_rs::{DetailOutput, artwork_detail, finish_command};
use pixiv_sdk::{
    Client, Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{self, Write};

#[derive(Deserialize)]
struct Case {
    mode: String,
    cause: String,
    limit: usize,
    output: String,
    diagnostics: String,
    exit: i32,
}
struct Fixture;
impl Transport for Fixture {
    async fn send(&self, _: Request) -> Result<Response> {
        Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({"illust": {
                "id": 42, "type": "illust", "title": "writer fixture",
                "user": {"id": 7, "name": "author"}, "create_date": "0001-01-01T00:00:00Z"
            }}),
        })
    }
}
struct LimitedWriter {
    output: Vec<u8>,
    remaining: usize,
    cause: String,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = bytes.len().min(self.remaining);
        self.output.extend_from_slice(&bytes[..count]);
        self.remaining -= count;
        if count < bytes.len() {
            let (kind, message) = match self.cause.as_str() {
                "pipe" => (io::ErrorKind::BrokenPipe, "broken pipe"),
                "denied" => (io::ErrorKind::PermissionDenied, "fixture output denied"),
                _ => (io::ErrorKind::Other, "fixture output stopped"),
            };
            Err(io::Error::new(kind, message))
        } else {
            Ok(count)
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn detail_writer_failures_preserve_partial_output_diagnostics_and_ndjson_exit() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/detail-writer.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 27);
    for case in cases {
        let mode = match case.mode.as_str() {
            "json" => DetailOutput::Json,
            "ndjson" => DetailOutput::Ndjson,
            _ => DetailOutput::Human,
        };
        let mut writer = LimitedWriter {
            output: vec![],
            remaining: case.limit,
            cause: case.cause.clone(),
        };
        let result = artwork_detail(
            &Client::with_transport("fixture-access", Fixture),
            42,
            mode,
            &mut writer,
        )
        .await;
        let mut diagnostics = vec![];
        let exit = finish_command(
            result,
            case.mode == "ndjson",
            case.mode != "human",
            &mut diagnostics,
        );
        let output = String::from_utf8(writer.output).unwrap();
        let diagnostics = String::from_utf8(diagnostics).unwrap();
        let label = format!("{} {} {}", case.mode, case.cause, case.limit);
        assert_eq!(exit, case.exit, "{label}");
        if case.limit == 100000 && case.mode != "human" {
            assert_eq!(
                serde_json::from_str::<Value>(&output).unwrap(),
                serde_json::from_str::<Value>(&case.output).unwrap(),
                "{label}"
            );
        } else {
            assert_eq!(output, case.output, "{label}");
        }
        if case.mode != "human" && !case.diagnostics.is_empty() {
            assert_eq!(
                serde_json::from_str::<Value>(&diagnostics).unwrap(),
                serde_json::from_str::<Value>(&case.diagnostics).unwrap(),
                "{label}"
            );
        } else {
            assert_eq!(diagnostics, case.diagnostics, "{label}");
        }
    }
}
