mod fanbox_browser_support;

use fanbox_browser_support::{Observed, adapter};
use pixiv_app::lifecycle::Context;
use pixiv_cli_rs::{
    CommandError,
    fanbox_auth::{BrowserProvider, SystemBrowserProvider},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    sync::Arc,
    task::{Context as TaskContext, Poll, Waker},
};

const FIXTURE: &[u8] = include_bytes!("fixtures/fanbox-auth.json");
const SEALED_SHA: &str = "1d3bab3ca00810abace1ebb34125f61df2d692db548baa53e31ecd0b814c2351";

#[tokio::test]
async fn normal_adapter_matches_all_twelve_sealed_go_provider_scenarios() {
    assert_eq!(format!("{:x}", Sha256::digest(FIXTURE)), SEALED_SHA);
    let fixture: Value = serde_json::from_slice(FIXTURE).unwrap();
    let mut scenarios = 0;
    for case in fixture["cases"].as_array().unwrap() {
        for (step, expected) in case["steps"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["observations"].as_array().unwrap())
        {
            let Some(mode) = step["system_browser_mode"]
                .as_str()
                .filter(|mode| !mode.is_empty())
            else {
                continue;
            };
            let args = step["args"].as_array().unwrap();
            let browser = args[args.iter().position(|arg| arg == "--from-browser").unwrap() + 1]
                .as_str()
                .unwrap();
            let profile = args
                .iter()
                .position(|arg| arg == "--profile")
                .map_or("", |index| args[index + 1].as_str().unwrap());
            let observed = Arc::new(Observed::default());
            let adapter = adapter(mode, observed.clone());
            let context = Context::new();
            let result = adapter.read_session(&context, browser, profile).await;
            let stderr = expected["stderr"].as_str().unwrap();
            if stderr.is_empty() {
                assert_eq!(
                    result.unwrap(),
                    "synthetic-browser-secret",
                    "{}",
                    case["name"]
                );
            } else {
                let message = if stderr.starts_with('{') {
                    serde_json::from_str::<Value>(stderr).unwrap()["error"]["message"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                } else {
                    stderr
                        .strip_prefix("error: ")
                        .unwrap()
                        .strip_suffix('\n')
                        .unwrap()
                        .to_owned()
                };
                assert_eq!(result.unwrap_err().to_string(), message, "{}", case["name"]);
            }
            let expected_trace = expected["trace"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .filter(|entry| entry.starts_with("provider."))
                .collect::<Vec<_>>();
            assert_eq!(
                *observed.trace.lock().unwrap(),
                expected_trace,
                "{}",
                case["name"]
            );
            assert!(
                observed
                    .contexts
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|address| *address == &context as *const Context as usize)
            );
            scenarios += 1;
        }
    }
    assert_eq!(scenarios, 12);
}

#[test]
fn dropping_an_inflight_adapter_future_closes_its_provider_once() {
    for mode in ["pending-discover", "pending-read"] {
        let observed = Arc::new(Observed::default());
        let adapter = adapter(mode, observed.clone());
        let context = Context::new();
        let mut future = adapter.read_session(&context, "fixture-fanbox", "");
        let mut task_context = TaskContext::from_waker(Waker::noop());
        assert!(matches!(
            Future::poll(future.as_mut(), &mut task_context),
            Poll::Pending
        ));
        assert!(
            !observed
                .trace
                .lock()
                .unwrap()
                .iter()
                .any(|event| event == "provider.close")
        );
        drop(future);
        assert_eq!(
            observed
                .trace
                .lock()
                .unwrap()
                .iter()
                .filter(|event| *event == "provider.close")
                .count(),
            1
        );
        drop(adapter);
        assert_eq!(
            observed
                .trace
                .lock()
                .unwrap()
                .iter()
                .filter(|event| *event == "provider.close")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn canceled_context_is_forwarded_to_the_provider_without_adapter_short_circuiting() {
    let observed = Arc::new(Observed::default());
    let adapter = adapter("success", observed.clone());
    let context = Context::new();
    context.cancel();
    assert_eq!(
        adapter
            .read_session(&context, "fixture-fanbox", "")
            .await
            .unwrap(),
        "synthetic-browser-secret"
    );
    assert_eq!(observed.contexts.lock().unwrap().len(), 2);
    assert!(
        observed
            .contexts
            .lock()
            .unwrap()
            .iter()
            .all(|address| *address == &context as *const Context as usize)
    );
}

#[tokio::test]
async fn failed_provider_construction_stops_before_any_provider_lifecycle() {
    let adapter = SystemBrowserProvider::with_factory(Arc::new(|name| {
        assert_eq!(name, "fixture-fanbox");
        Err(CommandError::Message(
            "synthetic provider construction failure",
        ))
    }));
    assert_eq!(
        adapter
            .read_session(&Context::new(), " FiXtUrE-FaNbOx ", "")
            .await
            .unwrap_err()
            .to_string(),
        "synthetic provider construction failure"
    );
}

#[tokio::test]
async fn system_registry_validates_names_and_reports_native_extraction_as_pending() {
    let adapter = SystemBrowserProvider::system();
    let context = Context::new();
    for browser in [
        "",
        "unsupported",
        "/owned-private-path-canary",
        "chrome-beta",
    ] {
        assert_eq!(
            adapter
                .read_session(&context, browser, "")
                .await
                .unwrap_err()
                .to_string(),
            "browsercookies: unknown browser"
        );
    }
    for browser in [
        "chrome",
        "edge",
        "firefox",
        "safari",
        "\u{0085}ChRoMe\u{00a0}",
        "F\u{0130}REFOX",
        "SAFAR\u{0130}",
    ] {
        assert_eq!(
            adapter
                .read_session(&context, browser, "")
                .await
                .unwrap_err()
                .to_string(),
            "browsercookies: native browser cookie extraction is not implemented"
        );
    }
}
