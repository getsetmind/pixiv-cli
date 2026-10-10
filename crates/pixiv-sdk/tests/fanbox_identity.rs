mod fanbox_support;
use fanbox_support::{
    FixtureTransport, SESSION, body_bytes, context_for, error_projection, expected_error,
};
use pixiv_sdk::{
    context::{Context, RequestContext},
    fanbox::{Client, CurrentUserRequest, FlareSolverrOptions, Options, SessionCredentials, User},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    sync::{Arc, atomic::Ordering},
};

fn rows() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("fixtures/fanbox-identity-protocol.json")).unwrap()["cases"].as_array().unwrap().clone()
}
fn public_operation(operation: &str) -> bool {
    matches!(
        operation,
        "credentials_format"
            | "user_dto"
            | "sdk_open"
            | "sdk_open_empty"
            | "sdk_open_with"
            | "sdk_current_user"
            | "sdk_validate_session"
    )
}
fn options(input: &Value, transport: Arc<FixtureTransport>) -> Options {
    Options {
        http_client: (input["client"] != "native").then_some(transport),
        proxy_url: input["proxy"].as_str().unwrap_or("").into(),
        user_agent: input["user_agent"].as_str().unwrap_or("").into(),
        flare_solverr: input.get("solver").map(|solver| FlareSolverrOptions {
            url: solver["URL"].as_str().unwrap().into(),
            proxy_url: solver["ProxyURL"].as_str().unwrap().into(),
        }),
    }
}
#[test]
fn frozen_rows_have_explicit_public_and_go_only_boundaries() {
    let cases = rows();
    assert_eq!(cases.len(), 309);
    assert_eq!(
        cases
            .iter()
            .filter(|case| public_operation(case["input"]["operation"].as_str().unwrap()))
            .count(),
        158
    );
    let excluded: BTreeSet<_> = cases
        .iter()
        .filter(|case| {
            public_operation(case["input"]["operation"].as_str().unwrap())
                && case["input"]["client"] == "implicit"
        })
        .map(|case| case["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        excluded,
        BTreeSet::from([
            "constructor/03",
            "constructor/13",
            "constructor/14",
            "constructor/15"
        ])
    );
    // Invalid options still precede the unrepresentable Go nil-Transport state.
    for name in ["constructor/03", "constructor/15"] {
        let case = cases.iter().find(|case| case["name"] == name).unwrap();
        assert_eq!(
            case["observation"]["constructor_error"]["message"],
            "fanbox:Open: credentials_expired: FANBOX injected HTTP client requires an explicit transport"
        );
    }
    assert_eq!(
        cases
            .iter()
            .filter(|case| !public_operation(case["input"]["operation"].as_str().unwrap()))
            .count(),
        151
    );
}
#[tokio::test]
async fn public_identity_options_and_ownership_match_156_frozen_rows() {
    let mut checked = 0;
    for case in rows() {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let operation = input["operation"].as_str().unwrap();
        if !public_operation(operation) || matches!(name, "constructor/03" | "constructor/15") {
            continue;
        }
        checked += 1;
        let expected = &case["observation"];
        if operation == "credentials_format" {
            let credentials = SessionCredentials {
                fanbox_sessid: input["cookie"].as_str().unwrap().into(),
            };
            assert_eq!(credentials.to_string(), expected["string"], "{name}");
            assert_eq!(format!("{credentials:?}"), expected["go_string"], "{name}");
            assert_eq!(format!("{credentials:20.3}"), expected["string"], "{name}");
            assert_eq!(
                serde_json::to_value(&credentials).unwrap(),
                expected["json"],
                "{name}"
            );
            continue;
        }
        if operation == "user_dto" {
            let user = &input["user"];
            let user = User {
                user_id: user["UserID"].as_i64().unwrap(),
                display_name: user["DisplayName"].as_str().unwrap().into(),
                creator_id: user["CreatorID"].as_str().unwrap().into(),
                creator_status: user["CreatorStatus"].as_str().unwrap().into(),
                is_creator: user["IsCreator"].as_bool().unwrap(),
            };
            assert_eq!(
                serde_json::to_value(user.to_dto()).unwrap(),
                expected["dto"],
                "{name}"
            );
            continue;
        }
        let context = context_for(input);
        let exact_context: Arc<dyn RequestContext> = Arc::new(context.clone());
        let transport = Arc::new(FixtureTransport::new(
            input["steps"].as_array().cloned().unwrap_or_default(),
            context,
        ));
        let cookie = input["cookie"]
            .as_str()
            .unwrap_or(if operation == "sdk_open_empty" {
                ""
            } else {
                SESSION
            });
        let credentials = SessionCredentials {
            fanbox_sessid: cookie.into(),
        };
        let constructed = if matches!(operation, "sdk_open" | "sdk_open_empty") {
            Client::open(credentials)
        } else {
            Client::open_with(credentials, options(input, transport.clone()))
        };
        assert_eq!(
            constructed.is_ok(),
            expected["constructed"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(
            error_projection(constructed.as_ref().err()),
            expected_error(&expected["constructor_error"]),
            "{name}"
        );
        if let Ok(client) = constructed {
            let repeats = input["repeat"].as_u64().unwrap_or(1);
            for index in 0..repeats as usize {
                let expected_outcome = &expected["outcomes"][index];
                let actual = match operation {
                    "sdk_current_user" => {
                        let result = client
                            .current_user(exact_context.clone(), CurrentUserRequest {})
                            .await;
                        json!({"dto":result.as_ref().map(|user|user.to_dto()).unwrap_or_default(),"error":error_projection(result.as_ref().err())})
                    }
                    "sdk_validate_session" => {
                        let result = client.validate_session(exact_context.clone()).await;
                        json!({"error":error_projection(result.as_ref().err())})
                    }
                    _ => json!({"error":error_projection(None)}),
                };
                let mut expected_outcome = expected_outcome.clone();
                expected_outcome["error"] = expected_error(&expected_outcome["error"]);
                assert_eq!(actual, expected_outcome, "{name} outcome {index}");
            }
            assert!(
                !format!("{client:?} {client}").contains(cookie),
                "{name} formats expose credentials"
            );
            client.close_idle_connections();
            client.close_idle_connections();
        }
        assert_eq!(
            *transport.requests.lock().unwrap(),
            expected["requests"].as_array().unwrap().clone(),
            "{name} requests"
        );
        for forwarded in transport.contexts.lock().unwrap().iter() {
            assert!(
                Arc::ptr_eq(forwarded, &exact_context),
                "{name} exact context identity"
            );
        }
        let expected_bodies = Value::Array(expected["bodies"].as_array().unwrap().iter().map(|body| json!({"injected_body":body["injected_body"],"bytes_read":body["bytes_read"],"close_calls":body["close_calls"]})).collect());
        assert_eq!(
            transport.body_projection(),
            expected_bodies,
            "{name} bodies"
        );
        assert_eq!(
            transport.idle_calls.load(Ordering::SeqCst),
            expected["close_idle_calls"].as_u64().unwrap() as usize,
            "{name} idle ownership"
        );
    }
    assert_eq!(checked, 156);
}
#[tokio::test]
async fn captured_public_html_identity_preserves_all_67_parser_cases() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-identity-public-html.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 67);
    assert_public_html_cases(cases).await;
}
const JSON_BYTES_FIXTURE: &[u8] = include_bytes!("fixtures/fanbox-identity-json-bytes.json");
fn json_bytes_rows() -> Vec<Value> {
    let fixture: Value = serde_json::from_slice(JSON_BYTES_FIXTURE).unwrap();
    assert_eq!(
        fixture["source_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["public_operation"], "fanbox.Client.CurrentUser");
    let cases = fixture["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 47);
    cases
}
fn json_bytes_family(family: &str, count: usize) -> Vec<Value> {
    let prefix = format!("{family}/");
    let cases: Vec<_> = json_bytes_rows()
        .into_iter()
        .filter(|case| case["name"].as_str().unwrap().starts_with(&prefix))
        .collect();
    assert_eq!(cases.len(), count, "{family} captured rows");
    cases
}
#[test]
fn frozen_public_identity_json_bytes_preserve_exact_47_capture_inputs() {
    assert_eq!(JSON_BYTES_FIXTURE.len(), 773_942);
    assert_eq!(
        format!("{:x}", Sha256::digest(JSON_BYTES_FIXTURE)),
        "8baf77f9538cb08d5a5b6bc0766e06b49dd8b3ef7f88f192027e7d58c9bdeb60"
    );
    let cases = json_bytes_rows();
    assert_eq!(
        cases
            .iter()
            .map(|case| case["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        47
    );
    for (family, count) in [
        ("surrogate", 11),
        ("syntax", 7),
        ("utf8", 11),
        ("depth", 9),
        ("html_preprocessing", 4),
        ("precedence", 5),
    ] {
        json_bytes_family(family, count);
    }
    for case in cases {
        assert_eq!(case["input"]["operation"], "sdk_current_user");
        for step in case["input"]["steps"].as_array().unwrap() {
            assert!(
                step.get("body").is_none(),
                "{} must remain raw bytes",
                case["name"]
            );
            assert!(step["body_hex"].is_string());
            assert!(step["body_length"].is_u64());
            assert!(step["body_sha256"].is_string());
            body_bytes(step);
        }
    }
}
#[tokio::test]
async fn captured_public_identity_json_strings_replace_surrogates_in_known_and_ignored_fields() {
    assert_public_html_cases(&json_bytes_family("surrogate", 11)).await;
}
#[tokio::test]
async fn captured_public_identity_json_strings_reject_malformed_escapes_everywhere() {
    assert_public_html_cases(&json_bytes_family("syntax", 7)).await;
}
#[tokio::test]
async fn captured_public_identity_json_utf8_preserves_per_byte_replacement_and_syntax_errors() {
    assert_public_html_cases(&json_bytes_family("utf8", 11)).await;
}
#[tokio::test]
async fn captured_public_identity_json_depth_counts_enclosing_and_ignored_containers() {
    assert_public_html_cases(&json_bytes_family("depth", 9)).await;
}
#[tokio::test]
async fn captured_public_identity_html_preprocessing_preserves_raw_json_bytes() {
    assert_public_html_cases(&json_bytes_family("html_preprocessing", 4)).await;
}
#[tokio::test]
async fn captured_public_identity_json_errors_preserve_read_close_and_validation_precedence() {
    assert_public_html_cases(&json_bytes_family("precedence", 5)).await;
}
async fn assert_public_html_cases(cases: &[Value]) {
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let expected = &case["observation"];
        let context = context_for(input);
        let exact_context: Arc<dyn RequestContext> = Arc::new(context.clone());
        let transport = Arc::new(FixtureTransport::new(
            input["steps"].as_array().unwrap().clone(),
            context,
        ));
        let client = Client::open_with(
            SessionCredentials {
                fanbox_sessid: SESSION.into(),
            },
            options(input, transport.clone()),
        )
        .unwrap();
        let result = client
            .current_user(exact_context.clone(), CurrentUserRequest {})
            .await;
        assert_eq!(
            serde_json::to_value(
                result
                    .as_ref()
                    .map(|user| user.to_dto())
                    .unwrap_or_default()
            )
            .unwrap(),
            expected["outcomes"][0]["dto"],
            "{name}"
        );
        assert_eq!(
            error_projection(result.as_ref().err()),
            expected_error(&expected["outcomes"][0]["error"]),
            "{name}"
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .map(|error| error.code.as_str())
                .unwrap_or(""),
            expected["outcomes"][0]["error"]["code"],
            "{name} code"
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .and_then(|error| std::error::Error::source(error))
                .map(ToString::to_string)
                .unwrap_or_default(),
            expected["outcomes"][0]["error"]["source_message"],
            "{name} safe source"
        );
        assert_eq!(
            *transport.requests.lock().unwrap(),
            expected["requests"].as_array().unwrap().clone(),
            "{name} requests"
        );
        for forwarded in transport.contexts.lock().unwrap().iter() {
            assert!(
                Arc::ptr_eq(forwarded, &exact_context),
                "{name} exact context identity"
            );
        }
        let expected_bodies = Value::Array(expected["bodies"].as_array().unwrap().iter().map(|body|json!({"injected_body":body["injected_body"],"bytes_read":body["bytes_read"],"close_calls":body["close_calls"]})).collect());
        assert_eq!(
            transport.body_projection(),
            expected_bodies,
            "{name} bodies"
        );
        client.close_idle_connections();
        client.close_idle_connections();
        assert_eq!(
            transport.idle_calls.load(Ordering::SeqCst),
            2,
            "{name} idle ownership"
        );
    }
}
fn source_regression_rows() -> Vec<Value> {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/fanbox-identity-tokenizer-regressions.json"
    ))
    .unwrap();
    let rows = fixture["cases"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 14);
    rows
}
#[tokio::test]
async fn captured_public_comment_and_rawtext_regressions_preserve_duplicate_attributes() {
    let cases: Vec<_> = source_regression_rows()
        .into_iter()
        .filter(|case| {
            let name = case["name"].as_str().unwrap();
            name.starts_with("comment/") || name.starts_with("rawtext/")
        })
        .collect();
    assert_eq!(cases.len(), 6);
    assert_public_html_cases(&cases).await;
}
#[tokio::test]
async fn captured_public_json_field_names_use_go_unicode_simple_fold() {
    let cases: Vec<_> = source_regression_rows()
        .into_iter()
        .filter(|case| case["name"].as_str().unwrap().starts_with("json/"))
        .collect();
    assert_eq!(cases.len(), 3);
    assert_public_html_cases(&cases).await;
}
#[test]
fn captured_public_decimal_proxy_ports_preserve_go_option_validation() {
    let cases: Vec<_> = source_regression_rows()
        .into_iter()
        .filter(|case| case["input"]["operation"] == "sdk_open_with")
        .collect();
    assert_eq!(cases.len(), 5);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let expected = &case["observation"];
        let transport = Arc::new(FixtureTransport::new(vec![], Context::new()));
        let result = Client::open_with(
            SessionCredentials {
                fanbox_sessid: SESSION.into(),
            },
            options(input, transport.clone()),
        );
        assert_eq!(
            result.is_ok(),
            expected["constructed"].as_bool().unwrap(),
            "{name} constructor"
        );
        assert_eq!(
            error_projection(result.as_ref().err()),
            expected_error(&expected["constructor_error"]),
            "{name} constructor error"
        );
        if let Ok(client) = result {
            client.close_idle_connections();
            client.close_idle_connections();
        }
        assert_eq!(
            transport.idle_calls.load(Ordering::SeqCst),
            expected["close_idle_calls"].as_u64().unwrap() as usize,
            "{name} idle ownership"
        );
        assert_eq!(
            *transport.requests.lock().unwrap(),
            expected["requests"].as_array().unwrap().clone(),
            "{name} requests"
        );
        assert_eq!(transport.body_projection(), json!([]), "{name} bodies");
    }
}
#[test]
fn client_formatting_retains_private_session_redaction_evidence() {
    let cases = rows();
    assert_eq!(
        cases
            .iter()
            .filter(|case| case["input"]["operation"] == "session_format")
            .count(),
        5
    );
    let transport = Arc::new(FixtureTransport::new(vec![], Context::new()));
    let client = Client::open_with(
        SessionCredentials {
            fanbox_sessid: SESSION.into(),
        },
        Options {
            http_client: Some(transport),
            ..Options::default()
        },
    )
    .unwrap();
    for rendered in [
        format!("{client}"),
        format!("{client:?}"),
        format!("{client:#?}"),
        format!("{client:20.3}"),
    ] {
        assert!(!rendered.contains(SESSION));
        assert!(!rendered.contains("FANBOXSESSID="));
    }
}

struct ContextTransport {
    inner: Arc<FixtureTransport>,
    observing: bool,
    entered: tokio::sync::Notify,
    snapshots: std::sync::Mutex<Vec<Value>>,
}
impl pixiv_sdk::fanbox::transport::RawTransport for ContextTransport {
    fn send(
        &self,
        request: pixiv_sdk::fanbox::transport::RawRequest,
    ) -> pixiv_sdk::fanbox::transport::TransportFuture<
        '_,
        std::result::Result<
            Option<pixiv_sdk::fanbox::transport::RawResponse>,
            pixiv_sdk::fanbox::transport::ExternalError,
        >,
    > {
        Box::pin(async move {
            let context = request.context.as_ref();
            self.snapshots.lock().unwrap().push(json!({"method":request.method,"url":request.url,
                "state":{"canceled":context.error()==Some(pixiv_sdk::context::ContextError::Canceled),
                "deadline":context.error()==Some(pixiv_sdk::context::ContextError::DeadlineExceeded),
                "has_deadline":context.deadline().is_some(),"scope_present":context.scope().is_some(),
                "value":context.value(&pixiv_sdk::context::ContextKey::new("fanbox-fixture")).and_then(|value|value.downcast::<String>().ok()).map(|value|(*value).clone())}}));
            request.context.emit(pixiv_sdk::diagnostics::Event {
                module: "FANBOX FlareSolverr".into(),
                kind: "started".into(),
                operation: "owned transport entered".into(),
                ..Default::default()
            });
            self.entered.notify_one();
            if self.observing {
                self.inner
                    .contexts
                    .lock()
                    .unwrap()
                    .push(request.context.clone());
                let error = request.context.cancelled().await;
                Err(Box::new(error) as pixiv_sdk::fanbox::transport::ExternalError)
            } else {
                pixiv_sdk::fanbox::transport::RawTransport::send(self.inner.as_ref(), request).await
            }
        })
    }
    fn close_idle_connections(&self) {
        pixiv_sdk::fanbox::transport::RawTransport::close_idle_connections(self.inner.as_ref());
    }
}
fn diagnostic_projection(event: &pixiv_sdk::diagnostics::Event) -> Value {
    json!({"Module":event.module,"Kind":event.kind,"Operation":event.operation,"Resource":event.resource,"Route":event.route,"Target":event.target,"Proxy":event.proxy,"UserAgent":event.user_agent,"Reason":event.reason,"Status":event.status,"Count":event.count,"RequestID":event.request_id,"Duration":event.duration_ns})
}
fn assert_context_state(context: &Context, expected: &Value, name: &str) {
    assert_eq!(
        context.error() == Some(pixiv_sdk::context::ContextError::Canceled),
        expected["error"]["canceled"],
        "{name}"
    );
    assert_eq!(
        context.error() == Some(pixiv_sdk::context::ContextError::DeadlineExceeded),
        expected["error"]["deadline_exceeded"],
        "{name}"
    );
    assert_eq!(
        context.deadline().is_some(),
        expected["has_deadline"],
        "{name}"
    );
    assert_eq!(
        context.scope().is_some(),
        expected["scope_present"],
        "{name}"
    );
    assert_eq!(
        context
            .value::<String>(&pixiv_sdk::context::ContextKey::new("fanbox-fixture"))
            .map(|value| (*value).clone()),
        expected["value"].as_str().map(str::to_owned),
        "{name}"
    );
}
#[tokio::test]
async fn public_identity_preserves_eight_owned_context_and_diagnostic_rows() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-context-ownership.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let nil = cases
        .iter()
        .find(|case| case["name"] == "sdk_current_user/nil_context")
        .unwrap();
    assert_eq!(
        nil["observation"]["error"]["message"],
        "fanbox:CurrentUser: upstream_error: build FANBOX request"
    );
    let mut checked = 0;
    for case in cases.iter().filter(|case| {
        case["input"]["operation"] == "fanbox.Client.CurrentUser"
            && case["name"] != "sdk_current_user/nil_context"
    }) {
        checked += 1;
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let mode = input["mode"].as_str().unwrap();
        let expected = &case["observation"];
        let events = Arc::new(std::sync::Mutex::new(vec![]));
        let captured = events.clone();
        let parent = Context::new().with_value(
            pixiv_sdk::context::ContextKey::new("fanbox-fixture"),
            Arc::new(input["parent_value"].as_str().unwrap().to_owned()),
        );
        let scoped = if mode.starts_with("absent_scope") {
            parent.clone()
        } else {
            parent
                .with_scope(pixiv_sdk::diagnostics::Scope::new(
                    Some(Arc::new(move |event: pixiv_sdk::diagnostics::Event| {
                        captured.lock().unwrap().push(diagnostic_projection(&event))
                    })),
                    "parent scope",
                    41,
                ))
                .with_child_scope("FANBOX FlareSolverr", 42)
        };
        let context = if mode.starts_with("detached") {
            scoped.without_cancel().child()
        } else if mode.contains("expired") {
            scoped
                .child_with_deadline(std::time::Instant::now() - std::time::Duration::from_secs(1))
        } else {
            scoped
        };
        if mode.contains("precanceled") || mode.starts_with("detached") {
            parent.cancel();
        }
        assert_context_state(&context, &expected["before"], name);
        let exact_context: Arc<dyn RequestContext> = Arc::new(context.clone());
        let inner = Arc::new(FixtureTransport::new(
            vec![json!({"status":input["response_status"],"body":input["response_body"]})],
            context.clone(),
        ));
        let transport = Arc::new(ContextTransport {
            inner: inner.clone(),
            observing: mode.ends_with("observing"),
            entered: tokio::sync::Notify::new(),
            snapshots: std::sync::Mutex::new(vec![]),
        });
        let client = Client::open_with(
            SessionCredentials {
                fanbox_sessid: SESSION.into(),
            },
            Options {
                http_client: Some(transport.clone()),
                user_agent: input["user_agent"].as_str().unwrap().into(),
                ..Options::default()
            },
        )
        .unwrap();
        let request = client.current_user(exact_context.clone(), CurrentUserRequest {});
        let result = if mode.contains("cancel_during") {
            let cancellation = async {
                transport.entered.notified().await;
                if mode.starts_with("detached") {
                    context.cancel()
                } else {
                    parent.cancel()
                }
            };
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tokio::join!(request, cancellation).0
            })
            .await
            .unwrap()
        } else {
            tokio::time::timeout(std::time::Duration::from_secs(2), request)
                .await
                .unwrap()
        };
        assert_context_state(&context, &expected["after"], name);
        assert_context_state(&parent, &expected["parent_after"], name);
        assert_eq!(
            serde_json::to_value(
                result
                    .as_ref()
                    .map(|user| user.to_dto())
                    .unwrap_or_default()
            )
            .unwrap(),
            expected["dto"],
            "{name}"
        );
        let mut expected_error_value = expected["error"].clone();
        expected_error_value["deadline"] = expected_error_value["deadline_exceeded"].clone();
        if let Some(sdk) = expected_error_value.get_mut("sdk") {
            sdk.as_object_mut()
                .unwrap()
                .remove("go_only_reason_sentinel_matches");
        }
        assert_eq!(
            error_projection(result.as_ref().err()),
            expected_error(&expected_error_value),
            "{name}"
        );
        assert_eq!(
            *events.lock().unwrap(),
            expected["events"].as_array().unwrap().clone(),
            "{name}"
        );
        let expected_requests: Vec<Value> = expected["requests"].as_array().unwrap().iter().map(|request| {
            let state=&request["state"];
            json!({"method":request["method"],"url":request["url"],"state":{"canceled":state["error"]["canceled"],"deadline":state["error"]["deadline_exceeded"],"has_deadline":state["has_deadline"],"scope_present":state["scope_present"],"value":state["value"]}})
        }).collect();
        assert_eq!(
            *transport.snapshots.lock().unwrap(),
            expected_requests,
            "{name} forwarded request state"
        );
        let contexts = inner.contexts.lock().unwrap();
        assert_eq!(contexts.len(), 1, "{name}");
        assert!(Arc::ptr_eq(&contexts[0], &exact_context), "{name}");
        let bodies = inner.bodies.lock().unwrap();
        let (bytes, closes) = bodies
            .iter()
            .map(|(_, record)| {
                let record = record.lock().unwrap();
                (record.bytes, record.closes)
            })
            .fold((0, 0), |(bytes, closes), (b, c)| (bytes + b, closes + c));
        assert_eq!(
            bytes,
            expected["response_bytes_read"].as_u64().unwrap() as usize,
            "{name}"
        );
        assert_eq!(
            closes,
            expected["response_body_close_calls"].as_u64().unwrap() as usize,
            "{name}"
        );
    }
    assert_eq!(checked, 8);
}

#[tokio::test]
async fn truncated_markup_releases_owned_body_without_panicking() {
    for document in [
        "<meta ",
        "<meta name='metadata' ",
        "<meta name='metadata' content='unterminated",
    ] {
        let transport = Arc::new(FixtureTransport::new(
            vec![json!({"status":200,"body":document})],
            Context::new(),
        ));
        let client = Client::open_with(
            SessionCredentials {
                fanbox_sessid: SESSION.into(),
            },
            Options {
                http_client: Some(transport.clone()),
                ..Options::default()
            },
        )
        .unwrap();
        let result = client
            .current_user(Arc::new(Context::background()), CurrentUserRequest {})
            .await;
        assert!(result.is_err());
        assert_eq!(
            transport.bodies.lock().unwrap()[0].1.lock().unwrap().closes,
            1
        );
    }
}
