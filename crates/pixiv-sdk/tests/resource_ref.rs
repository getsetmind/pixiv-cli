use base64::{Engine, engine::general_purpose::STANDARD};
use pixiv_sdk::{Reason, resource::ResourceRef};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    product: String,
    payload: Option<String>,
    text: String,
    json: String,
    reason: String,
}

#[derive(Deserialize)]
struct Contract {
    construct: Vec<Case>,
    parse: Vec<Case>,
    json: Vec<Case>,
}

fn contract() -> Contract {
    serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/resource-ref.json"
    ))
    .unwrap()
}

fn assert_result(case: &Case, result: pixiv_sdk::Result<ResourceRef>) {
    if case.reason.is_empty() {
        let reference = result.unwrap_or_else(|error| panic!("{}: {error}", case.name));
        assert_eq!(reference.as_str(), case.text, "{}", case.name);
        assert_eq!(reference.product().unwrap(), case.product, "{}", case.name);
        assert_eq!(
            serde_json::to_string(&reference).unwrap(),
            case.json,
            "{}",
            case.name
        );
        assert_eq!(
            reference.payload().unwrap(),
            STANDARD.decode(case.payload.as_ref().unwrap()).unwrap(),
            "{}",
            case.name
        );
    } else {
        assert_eq!(
            result.unwrap_err().code,
            Reason::InvalidArgument,
            "{}",
            case.name
        );
    }
}

#[test]
fn construction_matches_go_byte_encoding_and_validation() {
    for case in contract().construct {
        let payload = case
            .payload
            .as_ref()
            .map(|value| STANDARD.decode(value).unwrap())
            .unwrap_or_default();
        let result = ResourceRef::new(&case.product, &payload);
        if case.reason.is_empty() {
            assert_eq!(
                serde_json::to_string(result.as_ref().unwrap()).unwrap(),
                case.json,
                "{}",
                case.name
            );
        }
        assert_result(&case, result);
    }
}

#[test]
fn parsing_preserves_go_reference_text_and_envelope_rules() {
    for case in contract().parse {
        assert_result(&case, ResourceRef::parse(&case.text));
    }
}

#[test]
fn json_codec_matches_go_string_contract() {
    for case in contract().json {
        let mut reference = ResourceRef::default();
        let result = reference
            .unmarshal_json(case.json.as_bytes())
            .map(|()| reference);
        assert_result(&case, result);
        let result = serde_json::from_str::<ResourceRef>(&case.json);
        if case.reason.is_empty() {
            assert_result(&case, Ok(result.unwrap()));
        } else {
            assert!(result.is_err(), "{}", case.name);
        }
    }
}

#[test]
fn zero_reference_cannot_be_marshaled_or_read() {
    let reference = ResourceRef::default();
    assert!(reference.is_zero());
    assert_eq!(reference.as_str(), "");
    assert_eq!(
        reference.marshal_json().unwrap_err().code,
        Reason::InvalidArgument
    );
    assert_eq!(
        reference.marshal_text().unwrap_err().code,
        Reason::InvalidArgument
    );
    assert!(serde_json::to_string(&reference).is_err());
    assert_eq!(
        reference.product().unwrap_err().code,
        Reason::InvalidArgument
    );
    assert_eq!(
        reference.payload().unwrap_err().code,
        Reason::InvalidArgument
    );
}

#[test]
fn failed_unmarshal_does_not_replace_existing_identity() {
    let mut reference = ResourceRef::new("pixiv", b"artwork:42:page:0").unwrap();
    let original = reference.clone();
    assert!(
        reference
            .unmarshal_text("https://evil.invalid/asset")
            .is_err()
    );
    assert_eq!(reference, original);
    assert!(reference.unmarshal_json(b"42").is_err());
    assert_eq!(reference, original);
    reference
        .unmarshal_text(ResourceRef::new("fanbox", b"post:7").unwrap().as_str())
        .unwrap();
    assert_eq!(reference.product().unwrap(), "fanbox");
}
