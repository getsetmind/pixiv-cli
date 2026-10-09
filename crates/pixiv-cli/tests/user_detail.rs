#[path = "support/saved_account.rs"]
mod saved_account;
use pixiv_cli_rs::{DetailOutput, saved_user_detail, user_detail};
use pixiv_sdk::{
    Client, Result,
    transport::{JsonResponse, Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::{Arc, Mutex};
#[derive(Deserialize)]
struct Case {
    name: String,
    id: i64,
    body: Value,
    wire_body: Option<String>,
    mode: String,
    output: String,
    error: String,
    requests: usize,
}
#[derive(Clone)]
struct Fixture {
    user_id: i64,
    body: Vec<u8>,
    requests: Arc<Mutex<usize>>,
}
impl Transport for Fixture {
    async fn send(&self, _: Request) -> Result<Response> {
        panic!("user detail must use the lossless JSON transport")
    }
    async fn send_json(&self, request: Request) -> Result<JsonResponse> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/user/detail");
        assert_eq!(
            request.parameters,
            [("user_id".into(), self.user_id.to_string())]
        );
        *self.requests.lock().unwrap() += 1;
        Ok(JsonResponse {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}
#[tokio::test]
async fn user_detail_modes_match_go_dtos_records_profiles_and_failures() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/user-output.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 99);
    for case in cases {
        let body = case.wire_body.as_ref().map_or_else(
            || serde_json::to_vec(&case.body).unwrap(),
            |body| body.as_bytes().to_vec(),
        );
        for saved in [false, true] {
            let requests = Arc::new(Mutex::new(0));
            let client = Client::with_transport(
                "fixture-access",
                Fixture {
                    user_id: case.id,
                    body: body.clone(),
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
                    user_id: case.id,
                    body: body.clone(),
                    requests: requests.clone(),
                });
                saved_user_detail(
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
                user_detail(&client, case.id, mode, &mut out).await
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

#[tokio::test]
async fn dedicated_user_profile_preserves_owner_safe_lines_and_json() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/user-compat-output.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 8);
    for case in cases {
        let requests = Arc::new(Mutex::new(0));
        let application = saved_account::saved_execution(Fixture {
            user_id: 42,
            body: serde_json::to_vec(&case["body"]).unwrap(),
            requests: requests.clone(),
        });
        let mut output = Vec::new();
        let mode = if case["json"].as_bool().unwrap() {
            DetailOutput::Json
        } else {
            DetailOutput::Human
        };
        let error = pixiv_cli_rs::user_detail::saved_user_profile(
            &application.execution,
            &pixiv_app::lifecycle::Context::new(),
            42,
            Some(""),
            mode,
            &mut output,
        )
        .await
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
        assert_eq!(error, case["error"].as_str().unwrap(), "{}", case["name"]);
        if mode == DetailOutput::Json && !output.is_empty() {
            assert_eq!(
                serde_json::from_slice::<Value>(&output).unwrap(),
                serde_json::from_str::<Value>(case["output"].as_str().unwrap()).unwrap()
            );
        } else {
            assert_eq!(
                String::from_utf8(output).unwrap(),
                case["output"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
        assert_eq!(*requests.lock().unwrap(), 1);
    }
}
