use serde_json::Value;
use std::process::Command;

#[test]
fn explicit_json_failure_has_go_envelope_and_no_sdk_internal_metadata() {
    let output = Command::new(env!("CARGO_BIN_EXE_pixiv"))
        .args(["detail", "42", "--json"])
        .env("PIXIV_ACCESS_TOKEN", "")
        .env_remove("https_proxy")
        .env_remove("HTTPS_PROXY")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let envelope: Value = serde_json::from_slice(&output.stderr).unwrap();
    let contract: Value = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/errors.json"
    ))
    .unwrap();
    let case = contract["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "unauthorized")
        .unwrap();
    assert_eq!(
        envelope,
        serde_json::json!({"error":{"code":case["reason"],"message":case["message"]}})
    );
    assert_eq!(String::from_utf8(output.stderr).unwrap().lines().count(), 1);
}

#[test]
fn plain_failure_reports_the_same_classified_message() {
    let output = Command::new(env!("CARGO_BIN_EXE_pixiv"))
        .args(["detail", "42"])
        .env("PIXIV_ACCESS_TOKEN", "")
        .env_remove("https_proxy")
        .env_remove("HTTPS_PROXY")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "error: pixiv:Artwork: unauthorized"
    );
}
