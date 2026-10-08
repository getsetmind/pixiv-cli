use pixiv_sdk::{
    Client, Result,
    dto::ArtworkDto,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Deserialize)]
struct RecordedRequest {
    method: String,
    host: String,
    path: String,
    query: BTreeMap<String, Vec<String>>,
    headers: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    id: i64,
    body: Value,
    dto: Value,
    reason: String,
    message: String,
    requests: Vec<RecordedRequest>,
}
struct Fixture<'a> {
    case: &'a Case,
    requests: &'a Mutex<Vec<Request>>,
}
impl Transport for Fixture<'_> {
    async fn send(&self, request: Request) -> Result<Response> {
        self.requests.lock().unwrap().push(request);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.case.body.clone(),
        })
    }
}

#[tokio::test]
async fn artwork_detail_matches_go_dto_resources_errors_and_requests() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/artwork-detail.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 19);
    for case in cases {
        let requests = Mutex::new(vec![]);
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                case: &case,
                requests: &requests,
            },
        );
        let result = client.artwork(case.id).await;
        if case.reason.is_empty() {
            let artwork = result.unwrap_or_else(|error| panic!("{}: {error}", case.name));
            let actual = serde_json::to_value(ArtworkDto::from(&artwork)).unwrap();
            assert_eq!(actual, case.dto, "{}", case.name);
            let encoded = serde_json::to_string(&actual).unwrap();
            assert!(!encoded.contains("fixture-signature"), "{}", case.name);
            assert!(
                !format!("{artwork:?}").contains("fixture-signature"),
                "{}",
                case.name
            );
            for page in &artwork.pages {
                assert!(!page.image.resource.reference.is_zero());
                assert!(page.image.resource.url.starts_with("https://i.pximg.net/"));
            }
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code.as_str(), case.reason, "{}", case.name);
            assert_eq!(error.to_string(), case.message, "{}", case.name);
        }
        let actual = requests.lock().unwrap();
        assert_eq!(actual.len(), case.requests.len(), "{}", case.name);
        for (actual, expected) in actual.iter().zip(&case.requests) {
            let url = url::Url::parse(&actual.url).unwrap();
            assert_eq!(actual.method.as_str(), expected.method);
            assert_eq!(url.host_str().unwrap(), expected.host);
            assert_eq!(url.path(), expected.path);
            let mut query: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (key, value) in &actual.parameters {
                query.entry(key.clone()).or_default().push(value.clone());
            }
            assert_eq!(query, expected.query, "{}", case.name);
            for (key, value) in &expected.headers {
                assert_eq!(
                    actual
                        .headers
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case(key))
                        .map(|(_, value)| value),
                    Some(value),
                    "{} {key}",
                    case.name
                );
            }
        }
    }
}
