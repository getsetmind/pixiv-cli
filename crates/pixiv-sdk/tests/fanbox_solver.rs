mod fanbox_solver_support;
use fanbox_solver_support::{
    Control, Native, Probe, SESSION, StreamControl, body_bytes, expected_control, expected_native,
    expected_outcome, outcome, receive, registered, scoped,
};
use pixiv_sdk::{
    context::{ContextKey, RequestContext},
    fanbox::{Client, CurrentUserRequest, FlareSolverrOptions, Options, SessionCredentials},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/fanbox-solver-public.json")).unwrap()
}
fn client(input: &Value, native: Arc<Native>, url: &str) -> Client {
    client_result(input, native, url).unwrap()
}
fn client_result(input: &Value, native: Arc<Native>, url: &str) -> pixiv_sdk::Result<Client> {
    Client::open_with(
        SessionCredentials {
            fanbox_sessid: SESSION.into(),
        },
        Options {
            http_client: Some(native),
            proxy_url: "http://native-proxy.example:8888".into(),
            user_agent: "synthetic-native-agent".into(),
            flare_solverr: (input["solver_disabled"] != true).then(|| FlareSolverrOptions {
                url: format!("{url}/"),
                proxy_url: input["solver_proxy"].as_str().unwrap().into(),
            }),
        },
    )
}
#[test]
fn frozen_public_solver_retains_source_and_numeric_domain_boundaries() {
    let fixture = fixture();
    assert_eq!(
        fixture["source_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["source_fixture_case_count"], 124);
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 93);
    assert_eq!(
        cases
            .iter()
            .filter(|case| case["name"].as_str().unwrap().starts_with("solution/"))
            .count(),
        31
    );
    assert_eq!(
        cases
            .iter()
            .filter(|case| case["name"].as_str().unwrap().starts_with("expiry/"))
            .count(),
        22
    );
    let original: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-solver.json")).unwrap();
    assert_eq!(original["cases"].as_array().unwrap().len(), 124);
    assert!(
        original["cases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|case| case["family"] == "go_only_waiter_identity")
    );
    assert_eq!(
        serde_json::from_str::<Value>(include_str!("fixtures/fanbox-solver-redirect.json"))
            .unwrap()["cases"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}
#[tokio::test]
async fn actual_public_solver_matches_frozen_challenge_replay_payload_cache_and_errors() {
    let fixture = fixture();
    let mut checked = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["input"]["mode"] == "")
    {
        checked += 1;
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let expected = &case["observation"];
        let mut control = Control::new(input).await;
        if input["control_unavailable"] == true {
            control.stop().await;
        }
        let native = Arc::new(Native::new(input));
        let client = client(input, native.clone(), &control.url);
        let events = Arc::new(Mutex::new(vec![]));
        let exact_context: Arc<dyn RequestContext> =
            Arc::new(scoped("single", 71, false, events.clone()));
        let mut results = vec![];
        for _ in 0..input["calls"].as_u64().unwrap() {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                client.current_user(exact_context.clone(), CurrentUserRequest {}),
            )
            .await
            .unwrap();
            results.push(outcome(&result));
        }
        assert_eq!(
            results,
            expected["outcomes"]["single"]
                .as_array()
                .unwrap()
                .iter()
                .map(expected_outcome)
                .collect::<Vec<_>>(),
            "{name} public outcomes"
        );
        client.close_idle_connections();
        client.close_idle_connections();
        assert_eq!(
            native.view(),
            expected_native(&expected["native"]),
            "{name} native requests/body ownership"
        );
        assert_eq!(
            Value::Array(control.requests.lock().unwrap().clone()),
            expected_control(&expected["control_requests"]),
            "{name} anonymous ordinary control"
        );
        assert_eq!(
            *events.lock().unwrap(),
            expected["events"].as_array().unwrap().clone(),
            "{name} diagnostic events"
        );
        for forwarded in native.contexts.lock().unwrap().iter() {
            assert!(
                Arc::ptr_eq(forwarded, &exact_context),
                "{name} exact native caller context"
            );
        }
    }
    assert_eq!(checked, 89);
}
async fn call(client: Arc<Client>, context: Arc<dyn RequestContext>) -> Value {
    let result = client.current_user(context, CurrentUserRequest {}).await;
    outcome(&result)
}
#[tokio::test]
async fn public_shared_solver_owns_detached_scope_and_independent_waiter_cancellation() {
    let fixture = fixture();
    let mut checked = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["input"]["mode"] != "")
    {
        checked += 1;
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let mode = input["mode"].as_str().unwrap();
        let expected = &case["observation"];
        let mut control = Control::new(input).await;
        let native = Arc::new(Native::new(input));
        let client = Arc::new(client(input, native.clone(), &control.url));
        let events = Arc::new(Mutex::new(vec![]));
        let first = Arc::new(Probe::new(scoped("first", 71, true, events.clone())));
        let second = Arc::new(Probe::new(scoped("second", 91, false, events.clone())));
        native.probe("first", &first);
        native.probe("second", &second);
        let first_exact: Arc<dyn RequestContext> = first.clone();
        let second_exact: Arc<dyn RequestContext> = second.clone();
        let a = tokio::spawn(call(client.clone(), first_exact.clone()));
        assert_eq!(receive(&mut control.entered).await, 0);
        registered(&first).await;
        let b = tokio::spawn(call(client.clone(), second_exact.clone()));
        registered(&second).await;
        let mut outcomes = BTreeMap::new();
        if matches!(
            mode,
            "one_waiter_cancels" | "all_waiters_cancel_replacement"
        ) {
            first.context.cancel();
            outcomes.insert(
                "first",
                tokio::time::timeout(std::time::Duration::from_secs(5), a)
                    .await
                    .unwrap()
                    .unwrap(),
            );
            if mode == "all_waiters_cancel_replacement" {
                second.context.cancel();
                outcomes.insert(
                    "second",
                    tokio::time::timeout(std::time::Duration::from_secs(5), b)
                        .await
                        .unwrap()
                        .unwrap(),
                );
                assert_eq!(receive(&mut control.canceled).await, 0);
                control.releases[0].notify_one();
                assert_eq!(receive(&mut control.finished).await, 0);
                let third: Arc<dyn RequestContext> =
                    Arc::new(scoped("third", 101, false, events.clone()));
                let c = tokio::spawn(call(client.clone(), third));
                assert_eq!(receive(&mut control.entered).await, 1);
                control.releases[1].notify_one();
                outcomes.insert(
                    "third",
                    tokio::time::timeout(std::time::Duration::from_secs(5), c)
                        .await
                        .unwrap()
                        .unwrap(),
                );
                assert_eq!(receive(&mut control.finished).await, 1);
            } else {
                control.releases[0].notify_one();
                outcomes.insert(
                    "second",
                    tokio::time::timeout(std::time::Duration::from_secs(5), b)
                        .await
                        .unwrap()
                        .unwrap(),
                );
                assert_eq!(receive(&mut control.finished).await, 0);
            }
        } else {
            control.releases[0].notify_one();
            outcomes.insert(
                "first",
                tokio::time::timeout(std::time::Duration::from_secs(5), a)
                    .await
                    .unwrap()
                    .unwrap(),
            );
            outcomes.insert(
                "second",
                tokio::time::timeout(std::time::Duration::from_secs(5), b)
                    .await
                    .unwrap()
                    .unwrap(),
            );
            assert_eq!(receive(&mut control.finished).await, 0);
        }
        let expected_outcomes: BTreeMap<_, _> = expected["outcomes"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.as_str(), expected_outcome(value)))
            .collect();
        assert_eq!(outcomes, expected_outcomes, "{name} caller results");
        client.close_idle_connections();
        client.close_idle_connections();
        assert_eq!(
            native.view(),
            expected_native(&expected["native"]),
            "{name} native scope and byte/Close totals"
        );
        assert_eq!(
            Value::Array(control.requests.lock().unwrap().clone()),
            expected_control(&expected["control_requests"]),
            "{name} independent anonymous control"
        );
        let mut groups = BTreeMap::<String, Vec<Value>>::new();
        for event in events.lock().unwrap().iter() {
            groups
                .entry(event["RequestID"].to_string())
                .or_default()
                .push(event.clone());
        }
        assert_eq!(
            serde_json::to_value(groups).unwrap(),
            expected["events_by_caller_scope"],
            "{name} detached first caller scope"
        );
        for context in native.contexts.lock().unwrap().iter() {
            let label = context
                .value(&ContextKey::new("solver-caller"))
                .unwrap()
                .downcast::<String>()
                .unwrap();
            if *label == "first" {
                assert!(Arc::ptr_eq(context, &first_exact));
            } else if *label == "second" {
                assert!(Arc::ptr_eq(context, &second_exact));
            }
        }
    }
    assert_eq!(checked, 4);
}

#[tokio::test]
async fn public_solver_unicode_simplefold_aliases_match_seven_actual_frozen_go_rows() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-solver-public-aliases.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 7);
    assert_eq!(fixture["source_public_fixture_case_count"], 93);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let expected = &case["observation"];
        let control = Control::new(input).await;
        let native = Arc::new(Native::new(input));
        let client = Client::open_with(
            SessionCredentials {
                fanbox_sessid: SESSION.into(),
            },
            Options {
                http_client: Some(native.clone()),
                proxy_url: "http://native-proxy.example:8888".into(),
                user_agent: "synthetic-native-agent".into(),
                flare_solverr: Some(FlareSolverrOptions {
                    url: format!("{}/", control.url),
                    proxy_url: String::new(),
                }),
            },
        )
        .unwrap();
        let events = Arc::new(Mutex::new(vec![]));
        let context: Arc<dyn RequestContext> =
            Arc::new(scoped("single", 71, false, events.clone()));
        let mut results = vec![];
        for _ in 0..input["calls"].as_u64().unwrap() {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                client.current_user(context.clone(), CurrentUserRequest {}),
            )
            .await
            .unwrap();
            results.push(outcome(&result));
        }
        assert_eq!(
            results,
            expected["outcomes"]["single"]
                .as_array()
                .unwrap()
                .iter()
                .map(expected_outcome)
                .collect::<Vec<_>>(),
            "{name} public errors/DTO/source"
        );
        client.close_idle_connections();
        client.close_idle_connections();
        assert_eq!(
            native.view(),
            expected_native(&expected["native"]),
            "{name} native alias/expiry/cache recognition"
        );
        assert_eq!(
            Value::Array(control.requests.lock().unwrap().clone()),
            expected_control(&expected["control_requests"]),
            "{name} anonymous control payload"
        );
        assert_eq!(
            *events.lock().unwrap(),
            expected["events"].as_array().unwrap().clone(),
            "{name} diagnostics"
        );
        for forwarded in native.contexts.lock().unwrap().iter() {
            assert!(
                Arc::ptr_eq(forwarded, &context),
                "{name} exact native caller context"
            );
        }
    }
}

const JSON_DATES_FIXTURE: &[u8] = include_bytes!("fixtures/fanbox-solver-json-dates.json");
fn json_dates_rows() -> Vec<Value> {
    let fixture: Value = serde_json::from_slice(JSON_DATES_FIXTURE).unwrap();
    assert_eq!(
        fixture["source_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["public_operation"], "fanbox.Client.CurrentUser");
    assert_eq!(fixture["source_public_fixture_case_count"], 93);
    assert_eq!(fixture["reused_public_case"], "cache/native_future");
    let cases = fixture["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 79);
    cases
}
fn json_dates_family(family: &str, count: usize) -> Vec<Value> {
    let cases: Vec<_> = json_dates_rows()
        .into_iter()
        .filter(|case| case["family"] == family)
        .collect();
    assert_eq!(cases.len(), count, "{family} captured rows");
    cases
}
#[test]
fn frozen_public_solver_json_dates_preserve_exact_79_capture_inputs() {
    assert_eq!(JSON_DATES_FIXTURE.len(), 1_038_438);
    assert_eq!(
        format!("{:x}", Sha256::digest(JSON_DATES_FIXTURE)),
        "6793ce0d58bca871bb3b081808ebdff1aa6af87f06404110107dfebfc0f9572e"
    );
    let cases = json_dates_rows();
    assert_eq!(
        cases
            .iter()
            .map(|case| case["name"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        79
    );
    for (family, count) in [
        ("json_string", 23),
        ("json_utf8", 12),
        ("json_depth", 6),
        ("json_stream", 6),
        ("json_root", 4),
        ("expiry_rfc3339", 14),
        ("expiry_http_date", 14),
    ] {
        json_dates_family(family, count);
    }
    for case in cases {
        let input = &case["input"];
        assert_eq!(input["calls"], 2, "{} public cache calls", case["name"]);
        assert!(input["control_response_hex"].is_string());
        assert!(input.get("control_steps").is_none());
        assert!(!body_bytes(&json!({"body_hex":input["control_response_hex"]})).is_empty());
        for step in input["native_steps"].as_array().unwrap() {
            body_bytes(step);
        }
    }
}
async fn assert_public_json_dates_cases(cases: &[Value]) {
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let mut input = case["input"].clone();
        input["mode"] = json!("");
        input["solver_proxy"] = json!("");
        let expected = &case["observation"];
        let control = Control::new(&input).await;
        let native = Arc::new(Native::new(&input));
        let client = client(&input, native.clone(), &control.url);
        let events = Arc::new(Mutex::new(vec![]));
        let context: Arc<dyn RequestContext> =
            Arc::new(scoped("single", 71, false, events.clone()));
        let mut results = vec![];
        for _ in 0..input["calls"].as_u64().unwrap() {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                client.current_user(context.clone(), CurrentUserRequest {}),
            )
            .await
            .unwrap();
            results.push(outcome(&result));
        }
        assert_eq!(
            results,
            expected["outcomes"]["single"]
                .as_array()
                .unwrap()
                .iter()
                .map(expected_outcome)
                .collect::<Vec<_>>(),
            "{name} public errors/DTO/source"
        );
        client.close_idle_connections();
        client.close_idle_connections();
        assert_eq!(
            native.view(),
            expected_native(&expected["native"]),
            "{name} native headers/cache/byte and Close ownership"
        );
        assert_eq!(
            Value::Array(control.requests.lock().unwrap().clone()),
            expected_control(&expected["control_requests"]),
            "{name} anonymous control payload"
        );
        assert_eq!(
            *events.lock().unwrap(),
            expected["events"].as_array().unwrap().clone(),
            "{name} diagnostics"
        );
        for forwarded in native.contexts.lock().unwrap().iter() {
            assert!(
                Arc::ptr_eq(forwarded, &context),
                "{name} exact native caller context"
            );
        }
    }
}
#[tokio::test]
async fn captured_public_solver_json_strings_preserve_replacement_and_header_validation() {
    assert_public_json_dates_cases(&json_dates_family("json_string", 23)).await;
}
#[tokio::test]
async fn captured_public_solver_json_utf8_preserves_per_byte_replacement() {
    assert_public_json_dates_cases(&json_dates_family("json_utf8", 12)).await;
}
#[tokio::test]
async fn captured_public_solver_json_depth_counts_enclosing_and_ignored_containers() {
    assert_public_json_dates_cases(&json_dates_family("json_depth", 6)).await;
}
#[tokio::test]
async fn captured_public_solver_json_stream_decodes_only_the_first_complete_value() {
    assert_public_json_dates_cases(&json_dates_family("json_stream", 6)).await;
}
#[tokio::test]
async fn captured_public_solver_json_root_types_preserve_safe_public_errors() {
    assert_public_json_dates_cases(&json_dates_family("json_root", 4)).await;
}
#[tokio::test]
async fn captured_public_solver_rfc3339_expiry_preserves_cache_acceptance_boundaries() {
    assert_public_json_dates_cases(&json_dates_family("expiry_rfc3339", 14)).await;
}
#[tokio::test]
async fn captured_public_solver_http_date_expiry_preserves_exact_layout_acceptance() {
    assert_public_json_dates_cases(&json_dates_family("expiry_http_date", 14)).await;
}

const STREAM_COMPLETION_FIXTURE: &[u8] =
    include_bytes!("fixtures/fanbox-solver-stream-completion.json");
fn stream_completion_rows() -> Vec<Value> {
    let fixture: Value = serde_json::from_slice(STREAM_COMPLETION_FIXTURE).unwrap();
    assert_eq!(
        fixture["source_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(
        fixture["published_baseline"],
        "3dee232bf2571abbb9835fcd5ba9d5042aa0ec03"
    );
    assert_eq!(fixture["public_operation"], "fanbox.Client.CurrentUser");
    assert_eq!(fixture["source_public_fixture_case_count"], 93);
    assert_eq!(fixture["reused_public_case"], "cache/native_future");
    let cases = fixture["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 2);
    cases
}
fn stream_bytes(descriptor: &Value) -> Vec<u8> {
    assert!(descriptor["hex"].is_string());
    assert!(descriptor["length"].is_u64());
    assert!(descriptor["sha256"].is_string());
    body_bytes(
        &json!({"body_hex":descriptor["hex"],"body_length":descriptor["length"],"body_sha256":descriptor["sha256"]}),
    )
}
fn stream_prefix_depth(bytes: &[u8]) -> usize {
    let (mut depth, mut maximum, mut quoted, mut escaped) = (0, 0, false, false);
    for &byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth += 1;
                    maximum = maximum.max(depth);
                }
                b'}' | b']' => depth -= 1,
                _ => {}
            }
        }
    }
    maximum
}
#[test]
fn frozen_public_solver_stream_completion_preserves_exact_two_capture_inputs() {
    assert_eq!(STREAM_COMPLETION_FIXTURE.len(), 99_131);
    assert_eq!(
        format!("{:x}", Sha256::digest(STREAM_COMPLETION_FIXTURE)),
        "bd16cb555f25b671541e6f3876f52c954d9d08f9c1965196d467734cc4fc6825"
    );
    let rows = stream_completion_rows();
    for (case, (name, depth, native_count, prefix_length, prefix_sha, tail_length, tail_sha)) in
        rows.iter().zip([
            (
                "stream_completion/depth_10001_before_remainder",
                10_001,
                1,
                10_135,
                "b486cdf138077688a2654cccc19f672038e9324b6ba3cd2c53ecafccbbe8c760",
                10_002,
                "3a4c967a99eb91c76264efac7896fe9e62d87ce4dac9bab1ff280d3484171b53",
            ),
            (
                "stream_completion/first_object_before_invalid_deep_tail",
                4,
                2,
                151,
                "f0b6ce47ffcc12c361648d54a6455df654efab5932f288fffcf12b7d246bcb82",
                20_006,
                "9aa610d9b65191162fd4d1de7d64b4df3ec8b0685539d0e90362fa623949c224",
            ),
        ])
    {
        assert_eq!(case["name"], name);
        let input = &case["input"];
        let prefix = stream_bytes(&input["flushed_prefix"]);
        let remainder = stream_bytes(&input["held_remainder"]);
        assert_eq!(prefix.len(), prefix_length, "{name} exact prefix length");
        assert_eq!(
            format!("{:x}", Sha256::digest(&prefix)),
            prefix_sha,
            "{name} exact prefix SHA256"
        );
        assert_eq!(remainder.len(), tail_length, "{name} exact held length");
        assert_eq!(
            format!("{:x}", Sha256::digest(&remainder)),
            tail_sha,
            "{name} exact held SHA256"
        );
        assert!(prefix.len() + remainder.len() <= 128 * 1024);
        assert_eq!(input["prefix_global_depth"], depth);
        assert_eq!(stream_prefix_depth(&prefix), depth);
        let steps = input["native_steps"].as_array().unwrap();
        assert_eq!(steps.len(), native_count);
        for step in steps {
            body_bytes(step);
        }
    }
}
async fn assert_public_stream_completion(name: &str) {
    let case = stream_completion_rows()
        .into_iter()
        .find(|case| case["name"] == name)
        .unwrap();
    let mut input = case["input"].clone();
    input["mode"] = json!("");
    input["solver_proxy"] = json!("");
    let prefix = stream_bytes(&input["flushed_prefix"]);
    let prefix_length = prefix.len();
    let remainder = stream_bytes(&input["held_remainder"]);
    let expected = &case["observation"];
    let native = Arc::new(Native::new(&input));
    let mut control = StreamControl::new(prefix, remainder).await;
    let client = match client_result(&input, native.clone(), &control.url) {
        Ok(client) => Arc::new(client),
        Err(error) => {
            let cleanup = control.finish().await;
            assert_eq!(cleanup, Ok(()), "{name} setup-failure handler cleanup");
            panic!("{name} public client could not open: {error}");
        }
    };
    let events = Arc::new(Mutex::new(vec![]));
    let caller_context = Arc::new(scoped("single", 71, false, events.clone()));
    let exact_context: Arc<dyn RequestContext> = caller_context.clone();
    let mut caller = tokio::spawn(call(client.clone(), exact_context.clone()));
    let mut consumed = false;
    let completion = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let written = control
            .flushed
            .recv()
            .await
            .ok_or("owned prefix flush signal missing")?
            .map_err(|error| format!("owned prefix flush failed: {error}"))?;
        if written != prefix_length {
            return Err("owned prefix flush length differs".into());
        }
        let result = (&mut caller).await;
        consumed = true;
        let result = result.map_err(|error| format!("public caller failed: {error}"))?;
        Ok::<_, String>((result, control.completion(written)))
    })
    .await;
    control.release();
    let caller_cleanup = if consumed {
        Ok(())
    } else {
        caller_context.cancel();
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut caller).await {
            Ok(result) => result
                .map(|_| ())
                .map_err(|error| format!("public caller cleanup failed: {error}")),
            Err(_) => {
                caller.abort();
                let _ = caller.await;
                Err("five-second failure guard: public caller did not drain".into())
            }
        }
    };
    let handler_cleanup = control.finish().await;
    assert_eq!(caller_cleanup, Ok(()), "{name} caller cleanup");
    assert_eq!(handler_cleanup, Ok(()), "{name} owned handler cleanup");
    let (result, streaming) = completion
        .expect("five-second failure guard: CurrentUser waited for held remainder or EOF")
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(
        streaming, expected["stream_completion"],
        "{name} prefix flush/public return before remainder and EOF release"
    );
    assert_eq!(
        vec![result],
        expected["outcomes"]["single"]
            .as_array()
            .unwrap()
            .iter()
            .map(expected_outcome)
            .collect::<Vec<_>>(),
        "{name} genuine public outcome"
    );
    client.close_idle_connections();
    client.close_idle_connections();
    assert_eq!(
        native.view(),
        expected_native(&expected["native"]),
        "{name} native challenge/replay headers and body ownership"
    );
    assert_eq!(
        Value::Array(control.requests.lock().unwrap().clone()),
        expected_control(&expected["control_requests"]),
        "{name} anonymous ordinary HTTP/1 POST control"
    );
    assert_eq!(
        *events.lock().unwrap(),
        expected["events"].as_array().unwrap().clone(),
        "{name} public completion and replay diagnostics"
    );
    assert!(caller_context.error().is_none(), "{name} uncanceled caller");
    for forwarded in native.contexts.lock().unwrap().iter() {
        assert!(
            Arc::ptr_eq(forwarded, &exact_context),
            "{name} exact native caller context"
        );
    }
}
#[tokio::test]
async fn public_solver_rejects_depth_10001_before_remainder_or_eof() {
    assert_public_stream_completion("stream_completion/depth_10001_before_remainder").await;
}
#[tokio::test]
async fn public_solver_replays_first_object_before_deep_invalid_tail_or_eof() {
    assert_public_stream_completion("stream_completion/first_object_before_invalid_deep_tail")
        .await;
}
