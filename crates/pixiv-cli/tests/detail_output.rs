#[path = "support/saved_account.rs"]
mod saved_account;
use pixiv_cli_rs::{DetailOutput, artwork_detail, saved_artwork_detail};
use pixiv_sdk::{
    Client, Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::{Arc, Mutex};
#[derive(Deserialize)]
struct Case {
    name: String,
    id: i64,
    body: Value,
    mode: String,
    output: String,
    error: String,
    requests: usize,
}
#[derive(Clone)]
struct Fixture {
    body: Value,
    requests: Arc<Mutex<usize>>,
}
impl Transport for Fixture {
    async fn send(&self, _: Request) -> Result<Response> {
        *self.requests.lock().unwrap() += 1;
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}
#[tokio::test]
async fn artwork_detail_modes_match_go_dtos_records_captions_and_failures() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/detail-output.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 72);
    for case in cases {
        for saved in [false, true] {
            let requests = Arc::new(Mutex::new(0));
            let client = Client::with_transport(
                "fixture-access",
                Fixture {
                    body: case.body.clone(),
                    requests: requests.clone(),
                },
            );
            let mode = match case.mode.as_str() {
                "json" => DetailOutput::Json,
                "ndjson" => DetailOutput::Ndjson,
                _ => DetailOutput::Human,
            };
            let mut out = vec![];
            let result = if saved {
                let application = saved_account::saved_execution(Fixture {
                    body: case.body.clone(),
                    requests: requests.clone(),
                });
                saved_artwork_detail(
                    &application.execution,
                    &pixiv_app::lifecycle::Context::new(),
                    case.id,
                    0,
                    Some(""),
                    mode,
                    &mut out,
                )
                .await
            } else {
                artwork_detail(&client, case.id, mode, &mut out).await
            };
            let error = result
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default();
            assert_eq!(error, case.error, "{} {}", case.name, case.mode);
            let output = String::from_utf8(out).unwrap();
            if case.mode == "human" || case.output.is_empty() {
                assert_eq!(output, case.output, "{}", case.name);
            } else {
                assert_eq!(
                    serde_json::from_str::<Value>(&output).unwrap(),
                    serde_json::from_str::<Value>(&case.output).unwrap(),
                    "{} {}",
                    case.name,
                    case.mode
                );
                if case.mode == "ndjson" {
                    assert_eq!(output.lines().count(), 1);
                }
            }
            assert_eq!(*requests.lock().unwrap(), case.requests);
            assert!(!output.contains("fixture-signature"));
        }
    }
}
