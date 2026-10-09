use pixiv_sdk::resource::ResourceRef;
use serde::Deserialize;
use std::error::Error as _;

#[derive(Deserialize)]
struct Case {
    name: String,
    text: String,
    product: String,
    operation: String,
    reason: String,
    detail: String,
    cause: String,
    message: String,
    http_status: u16,
    transport: String,
    retry_safe: bool,
}

#[test]
fn resource_reference_diagnostics_preserve_frozen_go_classification_and_causes() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/resource_ref_diagnostics.json")).unwrap();
    for case in cases {
        let error = ResourceRef::parse(&case.text).unwrap_err();
        assert_eq!(error.product, case.product, "{}", case.name);
        assert_eq!(error.operation, case.operation, "{}", case.name);
        assert_eq!(error.code.as_str(), case.reason, "{}", case.name);
        assert_eq!(
            error.detail.as_deref().unwrap_or_default(),
            case.detail,
            "{}",
            case.name
        );
        assert_eq!(
            error.source().map(ToString::to_string).unwrap_or_default(),
            case.cause,
            "{}",
            case.name
        );
        assert_eq!(error.to_string(), case.message, "{}", case.name);
        assert_eq!(
            error.http_status.unwrap_or_default(),
            case.http_status,
            "{}",
            case.name
        );
        assert_eq!(
            error
                .transport
                .map(|value| format!("{value:?}").to_lowercase())
                .unwrap_or_default(),
            case.transport,
            "{}",
            case.name
        );
        assert_eq!(error.retry.safe, case.retry_safe, "{}", case.name);
        assert!(error.retry.after.is_none(), "{}", case.name);
    }
}
