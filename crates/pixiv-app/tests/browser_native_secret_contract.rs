#![cfg(target_pointer_width = "64")]

#[path = "support/browser_native_secret.rs"]
mod support;

use pixiv_app::{
    browser_chromium::{
        ChromiumCookieDecoder, ChromiumKeySource, ChromiumKind, PlatformChromiumKeyStore,
    },
    browser_dpapi::{DpapiBlob, WindowsDpapi},
    browser_secrets::{Keychain, SecretService},
    host_process::HostPlatform,
    lifecycle::Context,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use support::{CheckedContext, NativeDpapi, SecretProcess, StateFiles, decode_hex, encode_hex};

const NATIVE: &str = include_str!("../../pixiv-cli/tests/fixtures/browser-native-secret.json");
const CRYPTO: &str = include_str!("../../pixiv-cli/tests/fixtures/browser-chromium-crypto.json");

fn native_fixture() -> Value {
    let fixture: Value = serde_json::from_str(NATIVE).unwrap();
    assert_eq!(fixture["schema_version"], 1);
    assert_eq!(
        fixture["reference_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["darwin_secret_cases"].as_array().unwrap().len(), 14);
    assert_eq!(fixture["darwin_mapping_cases"].as_array().unwrap().len(), 8);
    assert_eq!(fixture["windows_dpapi_cases"].as_array().unwrap().len(), 11);
    assert_eq!(
        fixture["windows_routing_cases"].as_array().unwrap().len(),
        21
    );
    fixture
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap()
}

fn error_or_bytes<E: std::fmt::Display>(
    result: Result<pixiv_app::browser_cookies::SecretBytes, E>,
) -> (Vec<u8>, String) {
    match result {
        Ok(secret) => (secret.into_bytes(), String::new()),
        Err(error) => (Vec::new(), error.to_string()),
    }
}

fn context_for(case: &Value) -> Context {
    let context = Context::new();
    if text(case, "cancel") == "before" {
        context.cancel();
    }
    context
}

fn assert_command(process: &SecretProcess, case: &Value, program: &str) {
    assert_eq!(
        process.started_args(),
        case["expected"]["command_args"],
        "{} args",
        text(case, "name")
    );
    let calls = process.calls.lock().unwrap();
    assert_eq!(
        !calls.is_empty(),
        case["expected"]["command_started"].as_bool().unwrap(),
        "{} start",
        text(case, "name")
    );
    for call in calls.iter() {
        assert_eq!(call.program, program);
    }
}

#[test]
fn keychain_uses_frozen_command_bytes_errors_and_cancellation_order() {
    let fixture = native_fixture();
    for case in fixture["darwin_secret_cases"].as_array().unwrap() {
        let process = Arc::new(SecretProcess::new(
            HostPlatform::Darwin,
            text(case, "mode"),
            decode_hex(text(case, "output_hex")),
        ));
        let (bytes, error) = error_or_bytes(Keychain::new(process.clone()).get_password(
            &context_for(case),
            text(case, "service"),
            text(case, "account"),
        ));
        assert_eq!(
            encode_hex(&bytes),
            case["expected"]["value_hex"],
            "{} value",
            text(case, "name")
        );
        assert_eq!(
            error,
            case["expected"]["error"],
            "{} error",
            text(case, "name")
        );
        assert_command(&process, case, "security");
    }
}

#[test]
fn dpapi_shared_algorithm_matches_all_eleven_frozen_native_boundary_observations() {
    let fixture = native_fixture();
    for case in fixture["windows_dpapi_cases"].as_array().unwrap() {
        let blob = decode_hex(text(case, "input_hex"));
        let output = decode_hex(text(case, "output_hex"));
        let native = Arc::new(NativeDpapi::new(
            text(case, "mode"),
            output.clone(),
            (!blob.is_empty()).then_some(blob.as_ptr()),
            None,
        ));
        let context = CheckedContext::new(text(case, "cancel"), 2);
        let (bytes, error) =
            error_or_bytes(WindowsDpapi::new(native.clone()).unprotect(&context, &blob));
        let mut observed = native.observation();
        let copied =
            error.is_empty() && observed["allocation_overwritten"] == true && bytes == output;
        observed["value_hex"] = json!(encode_hex(&bytes));
        observed["keys_hex"] = Value::Null;
        observed["error"] = json!(error);
        observed["copied_output"] = json!(copied);
        observed["context_checks"] = json!(context.checks.load(Ordering::SeqCst));
        assert_eq!(
            observed,
            case["expected"],
            "{} boundary",
            text(case, "name")
        );
        if observed["allocated"] == true && observed["free_calls"] == 0 {
            assert!(
                native.residual_allocation_is_owned(),
                "mock retains source-edge allocations until teardown"
            );
        } else {
            assert!(!native.residual_allocation_is_owned());
        }
    }
}

#[test]
fn darwin_keysource_selects_safe_storage_attributes_and_keeps_access_error_mapping() {
    let fixture = native_fixture();
    let mut inaccessible_enum_cases = Vec::new();
    for case in fixture["darwin_mapping_cases"].as_array().unwrap() {
        if text(case, "browser") == "unknown-kind" {
            inaccessible_enum_cases.push(text(case, "name"));
            continue;
        }
        let kind = if text(case, "browser") == "edge" {
            ChromiumKind::Edge
        } else {
            ChromiumKind::Chrome
        };
        let process = Arc::new(SecretProcess::new(
            HostPlatform::Darwin,
            text(case, "mode"),
            decode_hex(text(case, "output_hex")),
        ));
        let root = PathBuf::from("/owned-native-darwin-profile-root");
        let files = Arc::new(StateFiles::new(root.clone(), None));
        let native = Arc::new(NativeDpapi::new("failure", Vec::new(), None, None));
        let keys = PlatformChromiumKeyStore::new(
            kind,
            root,
            files,
            process.clone(),
            Arc::new(WindowsDpapi::new(native.clone())),
        );
        let result = keys.keys(&context_for(case));
        let error = match result {
            Ok(keys) => {
                assert!(!keys.is_empty());
                assert!(keys.iter().all(|key| !key.as_bytes().is_empty()));
                String::new()
            }
            Err(error) => error.to_string(),
        };
        assert_eq!(
            error,
            case["expected"]["error"],
            "{} provider error",
            text(case, "name")
        );
        assert_command(&process, case, "security");
        assert_eq!(native.observation()["native_calls"], 0);
        // The genuine key-source returns derived keys, so raw helper password output is checked through Keychain.
        if error.is_empty() {
            let process = Arc::new(SecretProcess::new(
                HostPlatform::Darwin,
                text(case, "mode"),
                decode_hex(text(case, "output_hex")),
            ));
            let (bytes, error) = error_or_bytes(Keychain::new(process).get_password(
                &Context::new(),
                text(case, "service"),
                text(case, "account"),
            ));
            assert!(error.is_empty());
            assert_eq!(encode_hex(&bytes), case["expected"]["value_hex"]);
        }
    }
    // A Rust enum cannot invoke Go's same-package kind(77) helper input.
    assert_eq!(inaccessible_enum_cases, ["unknown-kind-defaults-to-chrome"]);
}

#[test]
fn windows_keys_and_decoder_route_frozen_bytes_through_shared_native_allocation_owner() {
    let fixture = native_fixture();
    for case in fixture["windows_routing_cases"].as_array().unwrap() {
        let context = context_for(case);
        // Context has no injected Err callback. Cancellation here is at native return, while the direct ABI test observes exact post-copy checks.
        let cancel = (text(case, "cancel") == "after-copy").then(|| context.clone());
        let output = decode_hex(text(case, "output_hex"));
        let native = Arc::new(NativeDpapi::new(
            text(case, "mode"),
            output.clone(),
            None,
            cancel,
        ));
        let process = Arc::new(SecretProcess::new(
            HostPlatform::Windows,
            "success",
            Vec::new(),
        ));
        let root = PathBuf::from("/owned-native-windows-profile-root");
        let state = case["local_state"]
            .as_str()
            .map(|body| body.as_bytes().to_vec());
        let files = Arc::new(StateFiles::new(root.clone(), state));
        let keys = Arc::new(PlatformChromiumKeyStore::new(
            ChromiumKind::Chrome,
            root,
            files,
            process.clone(),
            Arc::new(WindowsDpapi::new(native.clone())),
        ));
        let (bytes, key_bytes, error) = if text(case, "operation") == "keys" {
            match keys.keys(&context) {
                Ok(keys) => (
                    Vec::new(),
                    Some(
                        keys.into_iter()
                            .map(|key| key.into_bytes())
                            .collect::<Vec<_>>(),
                    ),
                    String::new(),
                ),
                Err(error) => (Vec::new(), None, error.to_string()),
            }
        } else {
            let (bytes, error) = error_or_bytes(
                ChromiumCookieDecoder::new(HostPlatform::Windows, keys)
                    .decode_value(&context, &decode_hex(text(case, "input_hex"))),
            );
            (bytes, None, error)
        };
        let mut observed = native.observation();
        let copied = error.is_empty()
            && observed["allocation_overwritten"] == true
            && if text(case, "operation") == "keys" {
                key_bytes
                    .as_ref()
                    .is_some_and(|keys| keys.len() == 1 && keys[0] == output)
            } else if !decode_hex(text(case, "input_hex")).starts_with(b"v10")
                && !decode_hex(text(case, "input_hex")).starts_with(b"v11")
            {
                bytes == output
            } else {
                false
            };
        observed["value_hex"] = json!(encode_hex(&bytes));
        observed["keys_hex"] = key_bytes
            .map(|keys| json!(keys.iter().map(|key| encode_hex(key)).collect::<Vec<_>>()))
            .unwrap_or(Value::Null);
        observed["error"] = json!(error);
        observed["copied_output"] = json!(copied);
        let mut expected = case["expected"].clone();
        expected.as_object_mut().unwrap().remove("context_checks");
        assert_eq!(
            observed,
            expected,
            "{} connected boundary excluding Go-only check instrumentation",
            text(case, "name")
        );
        assert!(process.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn secret_service_matches_eleven_frozen_linux_password_cases() {
    let fixture: Value = serde_json::from_str(CRYPTO).unwrap();
    let cases = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| text(case, "operation") == "linux_secret_password")
        .collect::<Vec<_>>();
    assert_eq!(cases.len(), 11);
    for case in cases {
        let input = &case["input"];
        let context = match text(input, "context") {
            "deadline" => Context::with_deadline(Instant::now() - Duration::from_secs(1)),
            "canceled" => {
                let context = Context::new();
                context.cancel();
                context
            }
            "active" => Context::new(),
            other => panic!("unknown synthetic context: {other}"),
        };
        let process = Arc::new(SecretProcess::new(
            HostPlatform::Linux,
            text(input, "secret_mode"),
            decode_hex(text(input, "password_hex")),
        ));
        let (bytes, error) = error_or_bytes(
            SecretService::new(process.clone()).get_password(&context, text(input, "application")),
        );
        assert_eq!(
            encode_hex(&bytes),
            case["output"]["value_hex"],
            "{} value",
            text(case, "id")
        );
        assert_eq!(error, case["output"]["error"], "{} error", text(case, "id"));
        assert_eq!(
            !error.is_empty(),
            case["output"]["value_nil"].as_bool().unwrap()
        );
        assert_eq!(
            process.started_args(),
            case["output"]["command_args"],
            "{} argv",
            text(case, "id")
        );
        for call in process.calls.lock().unwrap().iter() {
            assert_eq!(call.program, "secret-tool");
        }
    }
}

#[test]
fn successful_secret_process_result_is_not_rejected_by_a_new_post_success_context_check() {
    for platform in [HostPlatform::Linux, HostPlatform::Darwin] {
        let context = Context::new();
        let process = Arc::new(SecretProcess::new(
            platform,
            "success-cancels",
            b"synthetic-password\r\n".to_vec(),
        ));
        let result = if platform == HostPlatform::Linux {
            SecretService::new(process).get_password(&context, "chrome")
        } else {
            Keychain::new(process).get_password(&context, "Chrome Safe Storage", "Chrome")
        };
        assert_eq!(result.unwrap().as_bytes(), b"synthetic-password");
        assert!(context.error().is_some());
    }
}

#[test]
fn shared_blob_layout_matches_the_captured_sixty_four_bit_abi() {
    assert_eq!(std::mem::size_of::<DpapiBlob>(), 16);
    assert_eq!(std::mem::offset_of!(DpapiBlob, data), 8);
    assert_eq!(
        std::mem::align_of::<DpapiBlob>(),
        std::mem::align_of::<*mut u8>()
    );
}
