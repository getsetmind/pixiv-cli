use serde_json::Value;
use std::process::Command;

fn detail_failure(machine: bool) -> std::process::Output {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".pixiv-cli");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("config.toml"), "").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
    command
        .args(["detail", "42"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("PIXIV_ACCESS_TOKEN", "")
        .env_remove("https_proxy")
        .env("HTTPS_PROXY", "");
    if machine {
        command.arg("--json");
    }
    command.output().unwrap()
}

fn saved_account_failure(machine: bool) -> String {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/detail-accounts.json"
    ))
    .unwrap();
    cases
        .iter()
        .find(|case| case["name"] == "no_account" && case["json"] == machine)
        .unwrap()["stderr"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn explicit_json_failure_has_go_envelope_and_no_sdk_internal_metadata() {
    let output = detail_failure(true);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let envelope: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        envelope,
        serde_json::from_str::<Value>(&saved_account_failure(true)).unwrap()
    );
    assert_eq!(String::from_utf8(output.stderr).unwrap().lines().count(), 1);
}

#[test]
fn plain_failure_reports_the_same_classified_message() {
    let output = detail_failure(false);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        saved_account_failure(false)
    );
}
