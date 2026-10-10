mod fanbox_resource_support;
#[allow(dead_code)]
mod fanbox_solver_support;
use base64::{Engine, engine::general_purpose::STANDARD};
use fanbox_resource_support::{FixtureTransport, error, ownership, project};
use pixiv_sdk::{
    context::{Context, ContextKey, RequestContext},
    dto::ResourceDto,
    fanbox::{
        Client, CreatorRequest, FlareSolverrOptions, Options, PostRequest, SessionCredentials,
        transport::{
            ExternalError, RawBody, RawRequest, RawResponse, RawTransport, TransportFuture,
        },
    },
    resource::{OpenResourceRequest, Resource, ResourceRef},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error as StdError,
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

fn snapshot(resource: &Resource) -> Value {
    let payload = String::from_utf8(resource.reference.payload().unwrap()).unwrap();
    assert!(
        !payload.contains("https://")
            && !payload.contains("signature")
            && !payload.contains(fanbox_resource_support::SESSION)
    );
    json!({"ref":resource.reference.as_str(),"payload":payload,"url":resource.url,"request_headers":resource.request_headers,
        "expires_at":resource.expires_at,"requires_credentials":resource.requires_credentials,"dto":ResourceDto::from_resource(resource)})
}
async fn generate(
    client: &Client,
    context: Arc<dyn RequestContext>,
    kind: &str,
) -> std::result::Result<(Resource, Vec<Value>), pixiv_sdk::Error> {
    let mut all = vec![];
    if kind.starts_with("creator_") {
        let creator = client
            .creator(
                context,
                CreatorRequest {
                    creator_id: "resource-creator".into(),
                },
            )
            .await?;
        for resource in [&creator.icon.resource, &creator.cover.resource] {
            if !resource.reference.is_zero() {
                all.push(snapshot(resource));
            }
        }
        Ok((
            if kind == "creator_icon" {
                creator.icon.resource
            } else {
                creator.cover.resource
            },
            all,
        ))
    } else {
        let post = client
            .post(
                context,
                PostRequest {
                    post_id: "resource-post".into(),
                },
            )
            .await?;
        let mut selected = Resource::default();
        if let Some(body) = post.body {
            for asset in body.assets.unwrap_or_default() {
                all.push(snapshot(&asset.resource));
                if format!("post_{}", asset.kind.as_str()) == kind {
                    selected = asset.resource;
                }
            }
        }
        assert!(
            !selected.reference.is_zero(),
            "synthetic producer did not emit selected resource"
        );
        Ok((selected, all))
    }
}
struct SolverTraceTransport {
    inner: Arc<FixtureTransport>,
    controls: Arc<Mutex<Vec<Value>>>,
    seen: Mutex<usize>,
}
impl SolverTraceTransport {
    fn flush(&self) {
        let controls = self.controls.lock().unwrap();
        let mut seen = self.seen.lock().unwrap();
        for request in &controls[*seen..] {
            self.inner.trace.lock().unwrap().push(format!(
                "solver_control:{} {}",
                request["method"].as_str().unwrap(),
                request["path"].as_str().unwrap()
            ));
        }
        *seen = controls.len();
    }
}
impl RawTransport for SolverTraceTransport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, std::result::Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            self.flush();
            self.inner.send(request).await
        })
    }
    fn close_idle_connections(&self) {
        self.inner.close_idle_connections();
    }
}
async fn observe(input: &Value) -> Value {
    observe_with(input, None, "").await
}
async fn observe_with(
    input: &Value,
    control: Option<&fanbox_solver_support::Control>,
    proxy: &str,
) -> Value {
    let cancel = Context::new().with_value(
        ContextKey::new("resource-context-key"),
        Arc::new("resource-context".to_owned()),
    );
    let mut context: Arc<dyn RequestContext> = Arc::new(cancel.clone());
    let producer_transport = Arc::new(FixtureTransport::new(
        "producer",
        input["producer_document"].as_str().unwrap(),
        input,
        &cancel,
        context.clone(),
    ));
    let consumer_transport = Arc::new(FixtureTransport::new(
        "consumer",
        input["reopen_document"].as_str().unwrap(),
        input,
        &cancel,
        context.clone(),
    ));
    let recording = control.map(|control| {
        Arc::new(SolverTraceTransport {
            inner: producer_transport.clone(),
            controls: control.requests.clone(),
            seen: Mutex::new(0),
        })
    });
    let producer = if let Some(recording) = &recording {
        Client::open_with(
            SessionCredentials {
                fanbox_sessid: fanbox_resource_support::SESSION.into(),
            },
            Options {
                http_client: Some(recording.clone()),
                user_agent: "resource-injected-agent".into(),
                flare_solverr: control.map(|control| FlareSolverrOptions {
                    url: format!("{}/", control.url),
                    proxy_url: proxy.into(),
                }),
                ..Default::default()
            },
        )
        .unwrap()
    } else {
        producer_transport.client()
    };
    let consumer = (input["mode"] == "fresh").then(|| consumer_transport.client());
    let client = consumer.as_ref().unwrap_or(&producer);
    let mut result = json!({"generated_resources":[],"generation_error":error(None,false),"reference_error":error(None,false),"outcomes":[]});
    let mut reference = ResourceRef::default();
    let kind = input["kind"].as_str().unwrap();
    let ready = if !kind.is_empty() {
        match generate(&producer, context.clone(), kind).await {
            Ok((mut resource, all)) => {
                result["generated_resources"] = json!(all);
                reference = resource.reference.clone();
                if input["mutate_resource_header"] == true {
                    resource
                        .request_headers
                        .insert("Cookie".into(), "resource-raw-error-secret-canary".into());
                    resource
                        .request_headers
                        .insert("Referer".into(), "https://untrusted.invalid/".into());
                }
                true
            }
            Err(failure) => {
                result["generation_error"] = error(Some(&failure), false);
                false
            }
        }
    } else if let Some(payload) = input["ref_payload"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        match ResourceRef::new(input["ref_product"].as_str().unwrap(), payload.as_bytes()) {
            Ok(value) => {
                reference = value;
                true
            }
            Err(failure) => {
                result["reference_error"] = error(Some(&failure), false);
                false
            }
        }
    } else if input["parse_only"] == true || !input["parse_text"].as_str().unwrap().is_empty() {
        match ResourceRef::parse(input["parse_text"].as_str().unwrap()) {
            Ok(value) => {
                reference = value;
                input["parse_only"] != true
            }
            Err(failure) => {
                result["reference_error"] = error(Some(&failure), false);
                false
            }
        }
    } else {
        true
    };
    if ready {
        result["selected_ref"] = json!(reference.as_str());
        match input["context"].as_str().unwrap() {
            "canceled" => cancel.cancel(),
            "deadline" => {
                context =
                    Arc::new(cancel.child_with_deadline(Instant::now() - Duration::from_secs(1)))
            }
            _ => {}
        }
        let mut outcomes = vec![];
        for _ in 0..input["repeat"].as_u64().unwrap_or(0).max(1) {
            producer_transport
                .trace
                .lock()
                .unwrap()
                .push("action:open".into());
            let opened = client
                .open_resource(
                    context.clone(),
                    OpenResourceRequest {
                        reference: reference.clone(),
                        method: input["method"].as_str().unwrap().into(),
                        range: input["range"].as_str().unwrap().into(),
                        if_none_match: input["if_none_match"].as_str().unwrap().into(),
                        if_modified_since: input["if_modified_since"].as_str().unwrap().into(),
                        if_range: input["if_range"].as_str().unwrap().into(),
                    },
                )
                .await;
            let mut outcome = json!({"returned":opened.is_ok(),"open_error":error(opened.as_ref().err().map(|e|e as &dyn StdError),false),
                "steps":[],"ownership_at_return":ownership(&producer_transport,&consumer_transport)});
            if let Ok(mut response) = opened {
                let mut headers = response.header();
                outcome["response"] = json!({"status":response.status_code,"headers":headers,"content_type":response.content_type(),
                    "content_length":response.content_length(),"content_range":response.content_range(),"accept_ranges":response.accept_ranges(),
                    "etag":response.etag(),"last_modified":response.last_modified(),"cache_control":response.cache_control(),"body_non_nil":true});
                if let Some(values) = headers.get_mut("Content-Type")
                    && let Some(value) = values.first_mut()
                {
                    *value = "mutated-header".into();
                }
                headers.insert(
                    "Set-Cookie".into(),
                    vec!["resource-raw-error-secret-canary".into()],
                );
                outcome["headers_after_caller_copy_mutation"] = json!(response.header());
                let mut steps = vec![];
                for action in input["actions"].as_array().unwrap() {
                    producer_transport
                        .trace
                        .lock()
                        .unwrap()
                        .push(format!("action:{}", action["kind"].as_str().unwrap()));
                    let mut step =
                        json!({"action":action,"data":"","bytes":0,"error":error(None,false)});
                    match action["kind"].as_str().unwrap() {
                        "read" => {
                            let mut buffer = vec![0; action["size"].as_u64().unwrap() as usize];
                            let read = response.body.read(&mut buffer).await;
                            let bytes = &buffer[..read.count];
                            step["data_bytes"] = json!(STANDARD.encode(bytes));
                            step["data_hex"] = json!(
                                bytes
                                    .iter()
                                    .map(|byte| format!("{byte:02x}"))
                                    .collect::<String>()
                            );
                            step["data_sha256"] = json!(format!("{:x}", Sha256::digest(bytes)));
                            step["data"] = json!(std::str::from_utf8(bytes).ok());
                            step["bytes"] = json!(read.count);
                            step["error"] = error(
                                read.error.as_deref().map(|error| error as &dyn StdError),
                                read.eof,
                            );
                        }
                        "close" => {
                            let closed = response.body.close().await;
                            step["error"] = error(
                                closed.as_ref().err().map(|e| e.as_ref() as &dyn StdError),
                                false,
                            );
                        }
                        "cancel" => cancel.cancel(),
                        "idle" => client.close_idle_connections(),
                        kind => panic!("unknown resource action {kind}"),
                    }
                    step["ownership"] = ownership(&producer_transport, &consumer_transport);
                    steps.push(step);
                }
                outcome["steps"] = json!(steps);
            }
            outcomes.push(outcome);
        }
        result["outcomes"] = json!(outcomes);
    }
    producer.close_idle_connections();
    producer.close_idle_connections();
    if let Some(consumer) = consumer {
        consumer.close_idle_connections();
        consumer.close_idle_connections();
    }
    let requests = producer_transport
        .requests
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .chain(consumer_transport.requests.lock().unwrap().iter().cloned())
        .collect::<Vec<_>>();
    result["requests"] = json!(requests);
    result["final_ownership"] = ownership(&producer_transport, &consumer_transport);
    result["producer_close_idle_calls"] =
        json!(producer_transport.idle_calls.load(Ordering::SeqCst));
    result["consumer_close_idle_calls"] =
        json!(consumer_transport.idle_calls.load(Ordering::SeqCst));
    if let Some(recording) = &recording {
        recording.flush();
    }
    if let Some(control) = control {
        result["control_requests"] = json!(*control.requests.lock().unwrap());
        result["trace"] = json!(*producer_transport.trace.lock().unwrap());
    }
    result
}
fn differences(actual: &Value, expected: &Value, path: &str, result: &mut Vec<String>) {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            let keys: std::collections::BTreeSet<_> =
                actual.keys().chain(expected.keys()).collect();
            for key in keys {
                differences(
                    actual.get(key).unwrap_or(&Value::Null),
                    expected.get(key).unwrap_or(&Value::Null),
                    &format!("{path}/{key}"),
                    result,
                );
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            if actual.len() != expected.len() {
                result.push(format!(
                    "{path}/length: actual={} expected={}",
                    actual.len(),
                    expected.len()
                ));
            }
            for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                differences(actual, expected, &format!("{path}/{index}"), result);
            }
        }
        _ if actual != expected => {
            result.push(format!("{path}: actual={actual} expected={expected}"))
        }
        _ => {}
    }
}
#[tokio::test]
async fn all_frozen_public_resource_observations_match() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-resource-reads.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 243);
    let mut failures = vec![];
    for row in cases {
        let mut actual = observe(&row["input"]).await;
        let mut expected = row["observation"].clone();
        project(&mut actual);
        project(&mut expected);
        if actual != expected {
            let mut details = vec![];
            differences(&actual, &expected, "", &mut details);
            failures.push(format!("{}\n{}", row["name"], details.join("\n")));
        }
    }
    assert!(
        failures.is_empty(),
        "{} resource mismatches\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn media_error(error: Option<&(dyn StdError + 'static)>, eof: bool) -> Value {
    let value = fanbox_resource_support::error(error, eof);
    json!({"message":value["message"],"eof":value["eof"],"unexpected_eof":value["unexpected_eof"],
        "canceled":value["canceled"],"deadline":value["deadline_exceeded"],"source_failure_retained":false})
}
fn media_source(transport: &FixtureTransport) -> Value {
    json!(
        transport
            .ownership()
            .into_iter()
            .filter(|value| value["name"].as_str().unwrap().contains("/media/"))
            .map(|value| json!({"bytes_read":value["bytes_read"],"closes":value["close_calls"]}))
            .collect::<Vec<_>>()
    )
}
fn expected_media_error(value: &Value) -> Value {
    let mut value = value.clone();
    value.as_object_mut().unwrap().remove("type");
    value
}
fn expected_media_source(value: &Value) -> Value {
    json!(
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|value| json!({"bytes_read":value["bytes_read"],"closes":value["closes"]}))
            .collect::<Vec<_>>()
    )
}
#[tokio::test]
async fn standard_injected_media_body_rows_reuse_the_public_resource_boundary() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-media-body.json")).unwrap();
    let mut checked = 0;
    let excluded = [
        "apex_root_protocol_only",
        "empty_path_protocol_only",
        "http_rejected",
        "userinfo_rejected",
        "suffix_confusion_rejected",
        "pixiv_apex_rejected",
    ];
    for row in fixture["cases"].as_array().unwrap() {
        let input = &row["input"];
        let name = input["name"].as_str().unwrap();
        if input["boundary"] != "standard_injected" || excluded.contains(&name) {
            continue;
        }
        checked += 1;
        let expected = &row["result"];
        let cancel = Context::new().with_value(
            ContextKey::new("resource-context-key"),
            Arc::new("resource-context".to_owned()),
        );
        let context: Arc<dyn RequestContext> = Arc::new(cancel.clone());
        let document=json!({"body":{"creatorId":"resource-creator","user":{"name":"Owned creator","iconUrl":input["url"]}}}).to_string();
        let specs: Vec<_> = input["bodies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|body| {
                let mut body = body.clone();
                body["transport_content_length"] = body["content_length"].clone();
                body["header"]["Content-Length"] =
                    json!([body["content_length"].as_i64().unwrap().to_string()]);
                if !body["encoding"].as_str().unwrap().is_empty() {
                    body["header"]["Content-Encoding"] = json!([body["encoding"]]);
                }
                body["close_error"] = json!(if body["close_error"] == true {
                    "raw"
                } else {
                    ""
                });
                body
            })
            .collect();
        let adapted = json!({"media":specs,"metadata":{},"transport_error":input["transport_error"],"_media_preowned":true});
        let transport = Arc::new(FixtureTransport::new(
            "producer",
            &document,
            &adapted,
            &cancel,
            context.clone(),
        ));
        let client = transport.client_with("synthetic-owned-media", "owned-media-agent");
        let creator = client
            .creator(
                context.clone(),
                CreatorRequest {
                    creator_id: "resource-creator".into(),
                },
            )
            .await
            .unwrap();
        if input["context"] == "canceled" {
            cancel.cancel();
        }
        let request = &input["request"];
        // The public SDK accepts canonical GET; this adapter preserves the Session fixture's emitted method.
        let opened = client
            .open_resource(
                context.clone(),
                OpenResourceRequest {
                    reference: creator.icon.resource.reference,
                    method: request["Method"]
                        .as_str()
                        .unwrap()
                        .trim()
                        .to_ascii_uppercase(),
                    range: request["Range"].as_str().unwrap().into(),
                    if_none_match: request["IfNoneMatch"].as_str().unwrap().into(),
                    if_modified_since: request["IfModifiedSince"].as_str().unwrap().into(),
                    if_range: request["IfRange"].as_str().unwrap().into(),
                },
            )
            .await;
        let cause = opened.as_ref().err().and_then(|error| error.source());
        assert_eq!(
            media_error(cause, false),
            expected_media_error(&expected["open_error"]),
            "{name} open error"
        );
        assert_eq!(
            media_source(&transport),
            expected_media_source(&expected["ownership_at_return"]),
            "{name} ownership at return"
        );
        assert_eq!(
            opened.is_ok(),
            expected["response"]["returned"] == true,
            "{name} response presence"
        );
        if input["context"] == "cancel_after_open" {
            cancel.cancel();
        }
        if let Ok(mut response) = opened {
            assert_eq!(
                response.status_code,
                expected["response"]["status"].as_i64().unwrap(),
                "{name} status"
            );
            let expected_headers: pixiv_sdk::resource::ResourceHeaders =
                serde_json::from_value(expected["response"]["headers"].clone()).unwrap();
            let allowed: pixiv_sdk::resource::ResourceHeaders = expected_headers
                .into_iter()
                .filter(|(name, values)| {
                    !values.is_empty()
                        && [
                            "Content-Type",
                            "Content-Length",
                            "Content-Range",
                            "Accept-Ranges",
                            "Etag",
                            "Last-Modified",
                            "Cache-Control",
                        ]
                        .contains(&name.as_str())
                })
                .collect();
            assert_eq!(
                response.header(),
                allowed,
                "{name} public representation headers"
            );
            for (action, step) in input["actions"]
                .as_array()
                .unwrap()
                .iter()
                .zip(expected["steps"].as_array().unwrap())
            {
                let mut bytes = vec![];
                let failure = match action["kind"].as_str().unwrap() {
                    "read_all" => {
                        let mut buffer = vec![0; 512];
                        loop {
                            let read = response.body.read(&mut buffer).await;
                            bytes.extend_from_slice(&buffer[..read.count]);
                            if let Some(error) = read.error {
                                break Some(error);
                            }
                            if read.eof {
                                break None;
                            }
                        }
                    }
                    "close" => response.body.close().await.err(),
                    kind => panic!("uncovered media action {kind}"),
                };
                assert_eq!(
                    media_error(
                        failure.as_deref().map(|error| error as &dyn StdError),
                        false
                    ),
                    expected_media_error(&step["error"]),
                    "{name} action error {action}"
                );
                assert_eq!(
                    bytes.len() as u64,
                    step["bytes"].as_u64().unwrap(),
                    "{name} action bytes"
                );
                if action["kind"] == "read_all" {
                    assert_eq!(
                        STANDARD.encode(&bytes),
                        step["data"].as_str().unwrap_or(""),
                        "{name} exact bytes"
                    );
                    assert_eq!(
                        format!("{:x}", Sha256::digest(&bytes)),
                        step["sha256"].as_str().unwrap(),
                        "{name} sha256"
                    );
                    assert_eq!(
                        std::str::from_utf8(&bytes).unwrap_or(""),
                        step["text"].as_str().unwrap_or(""),
                        "{name} text"
                    );
                }
                assert_eq!(
                    media_source(&transport),
                    expected_media_source(&step["sources"]),
                    "{name} source ownership"
                );
            }
        }
        let requests=transport.requests.lock().unwrap().iter().filter(|request|!request["url"].as_str().unwrap().starts_with("https://api.fanbox.cc/")).map(|request|{
            json!({"method":request["method"],"url":request["url"],"host":request["host"],"headers":request["headers"],
                "context_error":media_error(request["context_error"]["canceled"].as_bool().filter(|v|*v).map(|_|&pixiv_sdk::context::ContextError::Canceled as &dyn StdError),false),
                "has_body":request["has_body"],"content_length":request["content_length"]})
        }).collect::<Vec<_>>();
        let expected_requests = expected["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|request| {
                let mut request = request.clone();
                request["context_error"] = expected_media_error(&request["context_error"]);
                request
            })
            .collect::<Vec<_>>();
        assert_eq!(requests, expected_requests, "{name} media requests");
        assert_eq!(
            media_source(&transport),
            expected_media_source(&expected["final_ownership"]),
            "{name} final source ownership"
        );
    }
    assert_eq!(checked, 31);
}

#[tokio::test]
async fn public_media_challenge_recovery_matches_frozen_go() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-media-solver.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for row in cases {
        let input = &row["input"];
        let mut control = fanbox_solver_support::Control::new(
            &json!({"control_steps":input["solver_steps"],"mode":""}),
        )
        .await;
        let mut actual = observe_with(
            &input["resource"],
            Some(&control),
            input["solver_proxy"].as_str().unwrap(),
        )
        .await;
        let mut expected = row["observation"].clone();
        expected["control_requests"] =
            fanbox_solver_support::expected_control(&expected["control_requests"]);
        project(&mut actual);
        project(&mut expected);
        assert_eq!(actual, expected, "{} public media solver", row["name"]);
        control.stop().await;
    }
}
