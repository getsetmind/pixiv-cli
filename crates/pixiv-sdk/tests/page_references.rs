use pixiv_sdk::{
    Error,
    reference::{Reference, parse_url},
};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Deserialize)]
struct Case {
    input: Option<String>,
    reference: WireReference,
    canonical: String,
    error: Option<Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WireReference {
    kind: String,
    #[serde(rename = "ID")]
    id: i64,
    #[serde(rename = "OwnerUserID")]
    owner_user_id: i64,
}
fn error_value(error: Error) -> Value {
    json!({"Reason":error.code.as_str(),"Product":error.product,"Operation":error.operation,"Detail":error.detail.unwrap_or_default(),"HTTPStatus":error.http_status.unwrap_or_default(),"Transport":error.transport.map(|kind|serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()).unwrap_or_default(),"Retry":{"Safe":error.retry.safe,"After":error.retry.after.map(|date|date.to_rfc3339()).unwrap_or_else(||"0001-01-01T00:00:00Z".into()),"HasAfter":error.retry.after.is_some()}})
}
#[test]
fn page_reference_kinds_ids_owners_canonical_urls_and_errors_match_go() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/page-references.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 68);
    for case in cases {
        let mut reference = Reference::default();
        let mut canonical = String::new();
        let mut error = None;
        let result = match &case.input {
            Some(input) => parse_url(input),
            None => Ok(Reference {
                kind: case.reference.kind.clone(),
                id: case.reference.id,
                owner_user_id: case.reference.owner_user_id,
            }),
        };
        match result {
            Ok(value) => {
                reference = value;
                match reference.canonical_url() {
                    Ok(value) => canonical = value,
                    Err(value) => error = Some(error_value(value)),
                }
            }
            Err(value) => error = Some(error_value(value)),
        }
        assert_eq!(
            reference.kind, case.reference.kind,
            "input={:?}",
            case.input
        );
        assert_eq!(reference.id, case.reference.id);
        assert_eq!(reference.owner_user_id, case.reference.owner_user_id);
        assert_eq!(canonical, case.canonical);
        assert_eq!(error, case.error, "input={:?}", case.input);
    }
}
