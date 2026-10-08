use base64::{Engine, engine::general_purpose::STANDARD};
use pixiv_sdk::{
    Reason, Result,
    cursor::{Cursor, CursorOptions, Page},
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    product: String,
    operation: String,
    binding: i64,
    query: String,
    payload: Option<String>,
    identity: String,
    ephemeral: bool,
    instance: Option<String>,
    text: String,
    json: String,
    reason: String,
    validate_reason: String,
    instance_reason: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Contract {
    construct: Vec<Case>,
    parse: Vec<Case>,
    validate: Vec<Case>,
    instances: Vec<Case>,
    #[serde(rename = "JSON")]
    json: Vec<Case>,
}
fn contract() -> Contract {
    serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cursors.json"
    ))
    .unwrap()
}
fn assert_reason(name: &str, result: Result<()>, reason: &str) {
    if reason.is_empty() {
        result.unwrap_or_else(|error| panic!("{name}: {error}"));
    } else {
        assert_eq!(result.unwrap_err().code.as_str(), reason, "{name}");
    }
}

#[test]
fn construction_matches_go_cursor_bytes_and_option_validation() {
    for case in contract().construct {
        let payload = case
            .payload
            .as_ref()
            .map(|value| STANDARD.decode(value).unwrap())
            .unwrap_or_default();
        let result = Cursor::new(
            &case.product,
            &case.operation,
            case.binding,
            &case.query,
            &payload,
            CursorOptions {
                identity: case.identity.clone(),
                ephemeral: case.ephemeral,
                instance: case.instance.clone(),
            },
        );
        if case.reason.is_empty() {
            let cursor = result.unwrap();
            assert_eq!(cursor.as_str(), case.text, "{}", case.name);
            assert_eq!(cursor.marshal_json().unwrap(), case.json, "{}", case.name);
            assert_eq!(cursor.is_ephemeral(), case.ephemeral, "{}", case.name);
        } else {
            assert_eq!(
                result.unwrap_err().code.as_str(),
                case.reason,
                "{}",
                case.name
            );
        }
    }
}

#[test]
fn parsing_preserves_go_encodings_without_premature_product_validation() {
    for case in contract().parse {
        let result = Cursor::parse(&case.text);
        if case.reason.is_empty() {
            let cursor = result.unwrap();
            assert_eq!(cursor.as_str(), case.text, "{}", case.name);
            assert_eq!(cursor.marshal_json().unwrap(), case.json, "{}", case.name);
            assert_eq!(
                cursor.identity().unwrap_or_default(),
                case.identity,
                "{}",
                case.name
            );
            assert_eq!(cursor.is_ephemeral(), case.ephemeral, "{}", case.name);
            assert_eq!(
                cursor.payload().unwrap(),
                case.payload
                    .as_ref()
                    .map(|value| STANDARD.decode(value).unwrap())
                    .unwrap_or_default(),
                "{}",
                case.name
            );
        } else {
            assert_eq!(
                result.unwrap_err().code,
                Reason::InvalidCursor,
                "{}",
                case.name
            );
        }
    }
}

#[test]
fn cursor_bindings_reject_other_products_operations_versions_and_queries() {
    for case in contract().validate {
        let cursor = Cursor::parse(&case.text).unwrap();
        assert_reason(
            &case.name,
            cursor.validate(&case.product, &case.operation, case.binding, &case.query),
            &case.validate_reason,
        );
    }
}

#[test]
fn ephemeral_cursors_reject_other_clients_and_missing_instances() {
    for case in contract().instances {
        let cursor = if case.text.is_empty() {
            Cursor::default()
        } else {
            Cursor::parse(&case.text).unwrap()
        };
        assert_reason(
            &case.name,
            cursor.validate_instance(case.instance.as_deref().unwrap()),
            &case.instance_reason,
        );
    }
}

#[test]
fn json_cursor_decoder_matches_go_and_retains_identity_on_failure() {
    for case in contract().json {
        let mut cursor = Cursor::default();
        let result = cursor.unmarshal_json(case.json.as_bytes());
        assert_reason(&case.name, result, &case.reason);
        assert_eq!(cursor.as_str(), case.text, "{}", case.name);
    }
    let mut cursor = Cursor::parse(&contract().construct[0].text).unwrap();
    let original = cursor.clone();
    assert!(cursor.unmarshal_text("%%% ").is_err());
    assert_eq!(cursor, original);
    assert!(cursor.unmarshal_json(b"42").is_err());
    assert_eq!(cursor, original);
}

#[test]
fn zero_cursor_means_no_page_and_cannot_be_marshaled_or_validated() {
    let page: Page<u64> = Page::default();
    assert!(page.items.is_empty());
    assert!(page.next.is_zero());
    assert_eq!(
        page.next.marshal_text().unwrap_err().code,
        Reason::InvalidArgument
    );
    assert_eq!(
        page.next.marshal_json().unwrap_err().code,
        Reason::InvalidArgument
    );
    assert_eq!(page.next.payload().unwrap_err().code, Reason::InvalidCursor);
    assert_eq!(
        page.next
            .validate("pixiv", "SearchArtworks", 2, "digest")
            .unwrap_err()
            .code,
        Reason::InvalidCursor
    );
    assert!(!page.next.is_ephemeral());
    assert_eq!(page.next.identity(), None);
}
