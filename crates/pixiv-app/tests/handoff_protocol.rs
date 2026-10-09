use pixiv_app::handoff_protocol::{
    canonical_relay_origin, is_allowed_pixiv_callback_url, parse_remote_login_link,
    relay_endpoint_url, validate_authorization_url, validate_relay_result_url,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Case {
    name: String,
    operation: String,
    input: String,
    #[serde(default)]
    base: String,
    #[serde(default)]
    suffix: String,
    #[serde(default)]
    session: String,
    output: Value,
    error: String,
}

#[test]
fn pure_handoff_protocol_matches_frozen_go_contracts() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/handoff_protocol.json")).unwrap();
    for case in cases {
        let result = match case.operation.as_str() {
            "origin" => canonical_relay_origin(&case.input).map(Value::String),
            "endpoint" => relay_endpoint_url(&case.input, &case.suffix, &case.session).map(Value::String),
            "result" => validate_relay_result_url(&case.base, &case.input).map(|()| Value::Null),
            "authorization" => validate_authorization_url(&case.input).map(|()| Value::Null),
            "callback" => Ok(Value::Bool(is_allowed_pixiv_callback_url(&case.input))),
            "link" => parse_remote_login_link(&case.input).map(|start| json!({"origin": start.origin, "session_id": start.session_id, "proof": start.proof})),
            _ => panic!("unknown operation"),
        };
        let (output, error) = match result {
            Ok(value) => (value, String::new()),
            Err(error) => (Value::Null, error.to_string()),
        };
        assert_eq!((output, error), (case.output, case.error), "{}", case.name);
    }
}

#[test]
fn capability_debug_output_is_redacted() {
    let start = parse_remote_login_link("pixiv://account/remote-login?origin=https%3A%2F%2Frelay.example.test&session=synthetic-session&access=synthetic-proof").unwrap();
    let debug = format!("{start:?}");
    assert!(!debug.contains("synthetic-session"));
    assert!(!debug.contains("synthetic-proof"));
}
