#[path = "support/browser_chromium.rs"]
mod support;

use pixiv_app::{
    browser_chromium::{
        ChromiumCookieDecoder, ChromiumKeySource, ChromiumKind, PlatformChromiumKeyStore,
    },
    host_process::ProcessStdio,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use support::*;

#[test]
fn actual_provider_capture_covers_every_blob_vector_without_helper_output_projection() {
    let cases = provider_fixture();
    let old = crypto_fixture();
    let expected_source_ids: BTreeSet<_> = old
        .iter()
        .filter(|case| {
            matches!(
                string(case, "operation"),
                "decrypt_gcm"
                    | "decrypt_cbc"
                    | "decrypt_chromium_value"
                    | "decrypt_legacy_value"
                    | "decrypt_encrypted"
            )
        })
        .map(|case| string(case, "id"))
        .collect();
    let mut observed_source_ids = BTreeSet::new();
    for case in cases
        .iter()
        .filter(|case| string(case, "operation") == "decrypt_encrypted")
    {
        let input = &case["input"];
        let expected = &case["output"];
        assert_eq!(string(input, "key_source"), "encryption_key_override");
        let keys = Keys::from_input(input);
        let decoder =
            ChromiumCookieDecoder::new(pixiv_app::host_process::HostPlatform::Linux, keys.clone());
        assert_value(
            decoder.decode_value(
                &context(string(input, "context")),
                &hex(string(input, "blob_hex")),
            ),
            expected,
            string(case, "id"),
        );
        assert_eq!(
            keys.call_count(),
            expected["key_calls"].as_u64().unwrap() as usize,
            "{} key calls",
            string(case, "id")
        );
        assert!(observed_source_ids.insert(string(input, "source_case_id")));
    }
    assert_eq!(expected_source_ids.len(), 61);
    assert_eq!(observed_source_ids, expected_source_ids);
}

#[test]
fn decoded_rows_preserve_raw_bytes_host_digest_precedence_and_partial_prior_values() {
    let mut compared = 0;
    for case in crypto_fixture()
        .iter()
        .filter(|case| string(case, "operation") == "rows_to_snapshot")
    {
        let input = &case["input"];
        let expected = &case["output"];
        let keys = Keys::from_input(input);
        let decoder =
            ChromiumCookieDecoder::new(pixiv_app::host_process::HostPlatform::Linux, keys.clone());
        let rows = input["rows_hex"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                row.as_array()
                    .unwrap()
                    .iter()
                    .map(|value| hex(value.as_str().unwrap()))
                    .collect()
            })
            .collect();
        let result = decoder.decode_rows(&context(string(input, "context")), rows);
        assert_failure(result.error.as_ref(), expected, string(case, "id"));
        let expected_values: Vec<_> = expected["cookie_values_hex"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| hex(value.as_str().unwrap()))
            .collect();
        assert_eq!(
            result
                .values
                .iter()
                .map(|value| value.as_bytes().to_vec())
                .collect::<Vec<_>>(),
            expected_values,
            "{} partial values",
            string(case, "id")
        );
        assert_eq!(
            result
                .values
                .iter()
                .map(|value| format!("{value:?}"))
                .collect::<Vec<_>>(),
            expected["secret_strings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>(),
            "{} redaction",
            string(case, "id")
        );
        let failed = result.error.is_some();
        assert_eq!(
            result.into_result().is_err(),
            failed,
            "{} public read conversion",
            string(case, "id")
        );
        assert_eq!(
            keys.call_count(),
            expected["key_calls"].as_u64().unwrap() as usize,
            "{} key calls",
            string(case, "id")
        );
        compared += 1;
    }
    assert_eq!(compared, 12);
}

struct KeyStoreFixture {
    store: PlatformChromiumKeyStore,
    calls: Arc<Mutex<Vec<SecretCall>>>,
    events: Arc<Mutex<Vec<&'static str>>>,
}

fn key_store(input: &Value) -> KeyStoreFixture {
    let events = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let files = Arc::new(StateFile {
        mode: string(input, "state_mode").into(),
        body: hex(string(input, "state_body_hex")),
        events: events.clone(),
    });
    let process = Arc::new(SecretHost {
        mode: string(input, "secret_mode").into(),
        password: hex(string(input, "password_hex")),
        calls: calls.clone(),
        events: events.clone(),
    });
    let kind = if string(input, "browser") == "edge" {
        ChromiumKind::Edge
    } else {
        ChromiumKind::Chrome
    };
    KeyStoreFixture {
        store: PlatformChromiumKeyStore::new(
            kind,
            PathBuf::from("/synthetic/browser"),
            files,
            process,
            unused_dpapi(),
        ),
        calls,
        events,
    }
}

fn assert_secret_calls(
    calls: &Mutex<Vec<SecretCall>>,
    events: &Mutex<Vec<&'static str>>,
    expected: &Value,
    id: &str,
) {
    let calls = calls.lock().unwrap();
    let args: Vec<_> = calls
        .iter()
        .filter_map(|call| match call {
            SecretCall::Lookup(program) => {
                assert_eq!(program, "secret-tool", "{id} lookup");
                None
            }
            SecretCall::Run(program, args, stdio) => {
                assert_eq!(program, "secret-tool", "{id} executable");
                assert_eq!(*stdio, ProcessStdio::CaptureStdout, "{id} capture");
                Some(
                    args.iter()
                        .map(|arg| arg.to_str().unwrap().to_owned())
                        .collect::<Vec<_>>(),
                )
            }
        })
        .collect();
    let expected_args: Vec<String> = expected["command_args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().into())
        .collect();
    assert_eq!(
        args,
        if expected_args.is_empty() {
            vec![]
        } else {
            vec![expected_args]
        },
        "{id} command arguments"
    );
    let events = events.lock().unwrap();
    if events.contains(&"state") {
        assert_eq!(
            events.as_slice(),
            &["secret", "state"],
            "{id} secret before state"
        );
    } else {
        assert!(
            events.is_empty() || events.as_slice() == ["secret"],
            "{id} no state after secret failure"
        );
    }
}

#[test]
fn actual_linux_keysource_capture_covers_candidates_unwraps_and_duplicate_null_fold_state_edges() {
    let mut compared = 0;
    for case in provider_fixture()
        .iter()
        .filter(|case| string(case, "operation") == "linux_encryption_keys")
    {
        let input = &case["input"];
        let expected = &case["output"];
        let KeyStoreFixture {
            store,
            calls,
            events,
        } = key_store(input);
        let result = store.keys(&context(string(input, "context")));
        assert_eq!(
            result.is_err(),
            expected["keys_nil"].as_bool().unwrap(),
            "{} key presence",
            string(case, "id")
        );
        let keys = assert_error(result, expected, string(case, "id"));
        let observed: Vec<_> = keys
            .unwrap_or_default()
            .into_iter()
            .map(|key| key.into_bytes())
            .collect();
        let expected_keys: Vec<_> = expected["keys_hex"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| hex(value.as_str().unwrap()))
            .collect();
        assert_eq!(observed, expected_keys, "{} key bytes", string(case, "id"));
        assert_secret_calls(&calls, &events, expected, string(case, "id"));
        compared += 1;
    }
    assert_eq!(compared, 34);
}

#[test]
fn actual_linux_decoder_capture_uses_secret_service_and_local_state_without_key_override() {
    let mut compared = 0;
    for case in provider_fixture()
        .iter()
        .filter(|case| string(case, "operation") == "linux_decrypt_encrypted")
    {
        let input = &case["input"];
        let expected = &case["output"];
        assert_eq!(string(input, "key_source"), "linux_secret_tool");
        let KeyStoreFixture {
            store,
            calls,
            events,
        } = key_store(input);
        let decoder = ChromiumCookieDecoder::new(
            pixiv_app::host_process::HostPlatform::Linux,
            Arc::new(store),
        );
        assert_value(
            decoder.decode_value(
                &context(string(input, "context")),
                &hex(string(input, "blob_hex")),
            ),
            expected,
            string(case, "id"),
        );
        assert_secret_calls(&calls, &events, expected, string(case, "id"));
        compared += 1;
    }
    assert_eq!(compared, 9);
}

#[test]
fn private_helper_corpus_and_public_read_capture_are_accounted_without_claiming_helper_projection()
{
    let cases = provider_fixture();
    let counts = cases.iter().fold(BTreeMap::new(), |mut counts, case| {
        *counts.entry(string(case, "operation")).or_insert(0) += 1;
        counts
    });
    assert_eq!(
        counts,
        BTreeMap::from([
            ("decrypt_encrypted", 61),
            ("linux_decrypt_encrypted", 9),
            ("linux_encryption_keys", 34),
            ("local_state_encrypted_key", 21),
            ("provider_read", 22),
        ])
    );
    let key_state_cases: BTreeSet<_> = cases
        .iter()
        .filter(|case| string(case, "operation") == "linux_encryption_keys")
        .map(|case| string(case, "id"))
        .collect();
    let private_state_cases: Vec<_> = cases
        .iter()
        .filter(|case| string(case, "operation") == "local_state_encrypted_key")
        .collect();
    for case in private_state_cases {
        // Local State remains private; these direct Go outputs are not Rust provider expectations.
        assert!(key_state_cases.contains(format!("keys-{}", string(case, "id")).as_str()));
    }
    let old_counts = crypto_fixture()
        .iter()
        .fold(BTreeMap::new(), |mut counts, case| {
            *counts
                .entry(string(case, "operation").to_owned())
                .or_insert(0) += 1;
            counts
        });
    assert_eq!(
        old_counts,
        BTreeMap::from([
            ("chromium_key_candidates".into(), 3),
            ("decrypt_cbc".into(), 18),
            ("decrypt_chromium_value".into(), 9),
            ("decrypt_encrypted".into(), 10),
            ("decrypt_gcm".into(), 17),
            ("decrypt_legacy_value".into(), 7),
            ("derive_chrome_key".into(), 3),
            ("derive_pbkdf2_sha1".into(), 7),
            ("legacy_blob_supported".into(), 5),
            ("linux_encryption_keys".into(), 13),
            ("linux_secret_password".into(), 11),
            ("local_state_encrypted_key".into(), 20),
            ("map_linux_secret_error".into(), 7),
            ("rows_to_snapshot".into(), 12),
            ("strip_host_digest".into(), 5),
            ("strip_legacy_prefix".into(), 5),
            ("unpad_cbc".into(), 7),
        ])
    );
}
