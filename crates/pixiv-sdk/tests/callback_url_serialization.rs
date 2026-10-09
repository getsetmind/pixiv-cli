use pixiv_sdk::oauth::LoginUrl;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    endpoints: Vec<Endpoint>,
    relay: Vec<Relay>,
}

#[derive(Deserialize)]
struct Endpoint {
    name: String,
    input: String,
    output: String,
    error: String,
}

#[derive(Deserialize)]
struct Relay {
    name: String,
    endpoint: String,
    callback: String,
    output: String,
}

#[test]
fn callback_endpoints_preserve_go_serialization() {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../pixiv-app/tests/fixtures/callback_dispatch.json"
    ))
    .unwrap();
    for case in fixture
        .endpoints
        .iter()
        .filter(|case| case.error.is_empty())
    {
        let parsed = LoginUrl::parse(case.input.trim()).unwrap();
        assert_eq!(parsed.with_fragment(""), case.output, "{}", case.name);
    }
}

#[test]
fn callback_fragments_preserve_the_original_input_and_go_escaping() {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../pixiv-app/tests/fixtures/callback_dispatch.json"
    ))
    .unwrap();
    for case in fixture.relay {
        let endpoint = LoginUrl::parse(case.endpoint.trim()).unwrap();
        assert_eq!(
            endpoint.with_fragment(&case.callback),
            case.output,
            "{}",
            case.name
        );
    }
}
