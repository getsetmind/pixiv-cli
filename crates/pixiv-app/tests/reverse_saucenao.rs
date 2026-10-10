use futures_util::FutureExt;
use pixiv_app::reverse_search::{
    ErrorCode,
    saucenao::{Client, Options},
};
use pixiv_sdk::context::Context;
use std::sync::Arc;

mod reverse_saucenao_support;

use pixiv_app::reverse_search::{
    CallerContext, Error, Loader, ProviderClient, SourceLoader, SourceLoaderOptions,
};
use pixiv_sdk::context::ContextError;
use reverse_saucenao_support::{StreamingTransport, Transport, bytes, caller_key};
use serde_json::{Value, json};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[tokio::test]
async fn credentials_are_checked_before_the_image_is_opened() {
    let client = Client::new(Options::default());
    let error = client
        .preflight(Some(Arc::new(Context::background())))
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::MissingCredential);
    assert_eq!(error.to_string(), "SauceNAO API key is required");
}

#[tokio::test]
async fn image_upload_is_unlimited_and_streams_through_the_provider_port() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("owned-large-image");
    let image_size = 20 * 1024 * 1024;
    let mut file = std::fs::File::create(&path).unwrap();
    let block = [b'x'; 32 * 1024];
    for _ in 0..image_size / block.len() {
        std::io::Write::write_all(&mut file, &block).unwrap();
    }
    drop(file);
    let snapshot = Loader::new(SourceLoaderOptions {
        temp_dir: root.path().to_owned(),
        ..Default::default()
    })
    .load(Arc::new(Context::background()), path.to_str().unwrap())
    .await
    .unwrap();
    let transport = Arc::new(StreamingTransport {
        image_size,
        bytes_seen: AtomicUsize::new(0),
    });
    let client: Arc<dyn ProviderClient> = Arc::new(Client::new(Options {
        api_key: "synthetic-saucenao-key-marker".to_owned(),
        transport: Some(transport.clone()),
        endpoint: String::new(),
    }));
    client
        .preflight(Arc::new(Context::background()))
        .await
        .unwrap();
    let response = client
        .search(Arc::new(Context::background()), snapshot.clone())
        .await
        .unwrap();
    assert_eq!(
        response.provider,
        pixiv_app::reverse_search::Provider::SauceNao
    );
    assert!(response.matches.is_empty());
    assert!(transport.bytes_seen.load(Ordering::SeqCst) > image_size);
    assert!(snapshot.open().is_ok());
    client.close().await.unwrap();
    snapshot.close().unwrap();
}

fn check_error(actual: Option<&Error>, expected: &Value, name: &str) {
    match actual {
        None => assert!(expected.is_null(), "{name}: expected {expected}"),
        Some(error) => {
            assert_eq!(error.code().as_str(), expected["code"], "{name}");
            assert_eq!(error.to_string(), expected["message"], "{name}");
            assert_eq!(
                error.context_error() == Some(ContextError::Canceled),
                expected["canceled"].as_bool().unwrap(),
                "{name}"
            );
            assert_eq!(
                error.context_error() == Some(ContextError::DeadlineExceeded),
                expected["deadline"].as_bool().unwrap(),
                "{name}"
            );
            for marker in [
                "synthetic-saucenao-key-marker",
                "synthetic-upstream-error-marker",
                "synthetic-private-image-marker",
            ] {
                let mut nested: Option<&dyn std::error::Error> = Some(error);
                while let Some(current) = nested {
                    assert!(!current.to_string().contains(marker), "{name}");
                    nested = current.source();
                }
            }
            assert_eq!(
                expected["chain"].as_array().unwrap().len(),
                1,
                "sealed Go concrete types remain distinct from Rust representations"
            );
        }
    }
}

#[tokio::test]
async fn sealed_public_saucenao_provider_contracts() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../pixiv-cli/tests/fixtures/reverse-saucenao.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 277);
    let mut go_nil_receivers = Vec::new();
    let mut failures = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let expected = &case["observation"];
        let is_nil_receiver = input["client"] == "nil";
        if is_nil_receiver {
            go_nil_receivers.push(name);
        }
        let outcome = std::panic::AssertUnwindSafe(async {
        let context = Arc::new(
            (if input["context"] == "deadline" {
                Context::with_deadline(Instant::now() - Duration::from_secs(1))
            } else {
                Context::new()
            })
            .with_value(
                caller_key(),
                Arc::new("synthetic-caller-context".to_owned()),
            ),
        );
        if input["context"] == "canceled" {
            context.cancel();
        }
        let caller: Option<CallerContext> =
            (input["context"] != "nil").then(|| context.clone() as CallerContext);
        let transport = Arc::new(Transport {
            input: input.clone(),
            expected_requests: expected["requests"].as_array().unwrap().clone(),
            requests: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
            idle: AtomicUsize::new(0),
            context: context.clone(),
        });
        let client = Client::new(Options {
            api_key: if is_nil_receiver {
                String::new()
            } else {
                input["key"].as_str().unwrap().to_owned()
            },
            endpoint: input["endpoint"].as_str().unwrap().to_owned(),
            transport: Some(transport.clone()),
        });
        let root = tempfile::tempdir().unwrap();
        let payload = bytes(&input["payload"]);
        let snapshot = if input["snapshot"] == "nil" {
            None
        } else {
            let path = root.path().join("owned-image");
            std::fs::write(&path, &payload).unwrap();
            let loader = Loader::new(SourceLoaderOptions {
                temp_dir: root.path().to_owned(),
                ..Default::default()
            });
            let snapshot = loader
                .load(Arc::new(Context::background()), path.to_str().unwrap())
                .await
                .unwrap();
            assert_eq!(
                snapshot.kind(),
                pixiv_app::reverse_search::SourceKind::File,
                "{name}"
            );
            assert_eq!(
                snapshot.size(),
                expected["snapshot"]["size"].as_i64().unwrap(),
                "{name}"
            );
            assert_eq!(snapshot.sha256(), expected["snapshot"]["sha256"], "{name}");
            if input["snapshot"] == "closed" {
                snapshot.close().unwrap();
            }
            Some(snapshot)
        };
        if input["close_before"] == true {
            client.close().await.unwrap();
        }
        for (index, expected_result) in expected["results"].as_array().unwrap().iter().enumerate() {
            assert!(index < input["repeat"].as_u64().unwrap() as usize);
            match input["operation"].as_str().unwrap() {
                "preflight" => check_error(
                    client.preflight(caller.clone()).await.as_ref().err(),
                    &expected_result["error"],
                    name,
                ),
                "search" => {
                    let outcome = client.search(caller.clone(), snapshot.clone()).await;
                    check_error(outcome.as_ref().err(), &expected_result["error"], name);
                    if let Ok(response) = outcome {
                        let mut actual =
                            json!({"provider": response.provider, "matches": response.matches});
                        if let Some(quota) = response.quota {
                            actual["quota"] = serde_json::to_value(quota).unwrap();
                        }
                        assert_eq!(actual, expected_result["response"], "{name}");
                        assert_eq!(expected_result["matches_nil"], false, "{name}");
                        assert_eq!(expected_result["quota_nil"], false, "{name}");
                    } else {
                        assert_eq!(
                            expected_result["response"],
                            json!({"provider":"", "matches":null}),
                            "Go zero response accompanies its error; Rust Result has no response"
                        );
                        assert_eq!(expected_result["matches_nil"], true, "{name}");
                        assert_eq!(expected_result["quota_nil"], true, "{name}");
                    }
                }
                "close" => {
                    if is_nil_receiver {
                        assert!(expected_result["error"].is_null());
                    } else {
                        check_error(
                            client.close().await.as_ref().err(),
                            &expected_result["error"],
                            name,
                        );
                    }
                }
                operation => panic!("unknown sealed operation {operation}"),
            }
        }
        assert_eq!(
            transport.requests.lock().unwrap().len(),
            expected["requests"].as_array().unwrap().len(),
            "{name}"
        );
        assert_eq!(
            transport.idle.load(Ordering::SeqCst),
            expected["idle_close_calls_before_final_close"]
                .as_u64()
                .unwrap() as usize,
            "{name}"
        );
        if !is_nil_receiver {
            client.close().await.unwrap();
            client.close().await.unwrap();
        }
        assert_eq!(
            transport.idle.load(Ordering::SeqCst),
            expected["idle_close_calls_after_final_close"]
                .as_u64()
                .unwrap() as usize,
            "{name}"
        );
        assert!(
            expected["first_close_error"].is_null() && expected["second_close_error"].is_null()
        );
        let bodies = transport.bodies.lock().unwrap();
        assert_eq!(
            bodies.len(),
            expected["response_bodies"].as_array().unwrap().len(),
            "{name}"
        );
        for (body, expected_body) in bodies
            .iter()
            .zip(expected["response_bodies"].as_array().unwrap())
        {
            let body = body.lock().unwrap();
            assert_eq!(
                body.bytes,
                expected_body["bytes_read"].as_u64().unwrap() as usize,
                "{name}: response bytes consumed"
            );
            assert_eq!(
                body.closes,
                expected_body["close_calls"].as_u64().unwrap() as usize,
                "{name}: once-only response close"
            );
            assert_eq!(
                body.reads,
                expected_body["read_calls"].as_u64().unwrap() as usize,
                "{name}: pinned decoder read behavior"
            );
        }
        match context.error() {
            None => assert!(expected["context_after"].is_null(), "{name}"),
            Some(error) => {
                assert_eq!(
                    error.to_string(),
                    expected["context_after"]["message"],
                    "{name}"
                );
                assert_eq!(
                    error == ContextError::Canceled,
                    expected["context_after"]["canceled"].as_bool().unwrap(),
                    "{name}"
                );
            }
        }
        assert_eq!(
            expected["caller_http_client_unchanged"], true,
            "the Rust provider retains its supplied transport by Arc"
        );
        if let Some(snapshot) = snapshot {
            if input["snapshot"] != "closed" {
                let mut reader = snapshot.open().unwrap();
                let mut retained = Vec::new();
                std::io::Read::read_to_end(&mut reader, &mut retained).unwrap();
                assert_eq!(retained, payload, "{name}: caller retains its snapshot");
            }
            snapshot.close().unwrap();
        }
        }).catch_unwind().await;
        if outcome.is_err() {
            failures.push(name);
        }
    }
    assert!(
        failures.is_empty(),
        "sealed provider rows with differences: {failures:?}"
    );
    assert_eq!(
        go_nil_receivers,
        [
            "validation/preflight_nil_nil",
            "validation/preflight_nil_canceled",
            "validation/preflight_nil_deadline",
            "validation/preflight_nil_background",
            "validation/search_nil_nil",
            "validation/search_nil_canceled",
            "validation/search_nil_deadline",
            "validation/search_nil_background",
            "close/nil_idempotent"
        ]
    );
}
