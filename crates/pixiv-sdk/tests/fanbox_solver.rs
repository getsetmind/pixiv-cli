mod fanbox_solver_support;
use fanbox_solver_support::{
    Control, Native, Probe, SESSION, expected_control, expected_native, expected_outcome, outcome,
    receive, registered, scoped,
};
use pixiv_sdk::{
    context::{ContextKey, RequestContext},
    fanbox::{Client, CurrentUserRequest, FlareSolverrOptions, Options, SessionCredentials},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/fanbox-solver-public.json")).unwrap()
}
fn client(input: &Value, native: Arc<Native>, url: &str) -> Client {
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
    .unwrap()
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
