use pixiv_app::config::{ConfigError, Snapshot};
use serde_json::Value;
#[test]
fn captured_control_character_config_error_remains_a_go_diagnostic() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../pixiv-cli/tests/fixtures/fanbox-content-reads.json"
    ))
    .unwrap();
    let row = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "saved-invalid-agent")
        .unwrap();
    let body = row["input"]["config"].as_str().unwrap();
    let error = Snapshot::parse(body, []).err().unwrap();
    if let ConfigError::Syntax(raw) = &error {
        eprintln!(
            "actualparser {:?} span {:?} rawbytes {:?}",
            raw.message(),
            raw.span(),
            body.as_bytes()
        );
    }
    assert_eq!(
        error.to_string(),
        row["observation"]["errors"][0].as_str().unwrap()
    );
}
