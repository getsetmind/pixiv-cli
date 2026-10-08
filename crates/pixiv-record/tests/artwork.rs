use pixiv_sdk::{
    Client, Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Case {
    name: String,
    id: i64,
    body: Value,
    mode: String,
    output: String,
    error: String,
}
struct Fixture(Value);
impl Transport for Fixture {
    async fn send(&self, _: Request) -> Result<Response> {
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.0.clone(),
        })
    }
}
#[tokio::test]
async fn artwork_records_match_go_canonical_identity_dtos_and_kind_rejection() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/detail-output.json"
    ))
    .unwrap();
    let mut compared = 0;
    for case in cases.into_iter().filter(|case| case.mode == "ndjson") {
        let client = Client::with_transport("fixture-access", Fixture(case.body));
        let Ok(artwork) = client.artwork(case.id).await else {
            continue;
        };
        let result = pixiv_record::from_artwork(&artwork);
        if case.error.is_empty() {
            assert_eq!(
                result.unwrap(),
                serde_json::from_str::<Value>(&case.output).unwrap(),
                "{}",
                case.name
            );
        } else {
            assert_eq!(result.unwrap_err().to_string(), case.error, "{}", case.name);
        }
        compared += 1;
    }
    assert_eq!(compared, 17);
}
