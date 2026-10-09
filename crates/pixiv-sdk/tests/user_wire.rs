use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_sdk::{
    Client,
    dto::{UserDetailDto, UserPreviewDto},
    pixiv::{SearchUsersRequest, UserRequest},
    transport::{JsonResponse, Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Deserialize)]
struct Case {
    name: String,
    operation: String,
    body: String,
    dto: Value,
    #[serde(default)]
    cursor: Value,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    message: String,
    requests: Value,
}
struct Fixture {
    operation: String,
    body: String,
    requests: Arc<Mutex<Vec<Value>>>,
}
impl Transport for Fixture {
    async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
        panic!("user operations must consume the lossless JSON response capability")
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        assert_eq!(request.operation, self.operation);
        assert!(
            request
                .headers
                .iter()
                .any(|(key, value)| key == "Authorization" && value == "Bearer fixture-access")
        );
        let path = if self.operation == "User" {
            "/v1/user/detail"
        } else {
            "/v1/search/user"
        };
        assert_eq!(request.url, format!("https://app-api.pixiv.net{path}"));
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        self.requests
            .lock()
            .unwrap()
            .push(json!({"method":request.method.as_str(),"path":path,"query":query}));
        Ok(JsonResponse {
            status: 200,
            retry_after: None,
            body: self.body.as_bytes().to_vec(),
        })
    }
}

#[tokio::test]
async fn user_wire_matches_go_case_folding_ordered_merges_nulls_and_validation() {
    let cases: Vec<Case> = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/migration/contracts/user-wire.json"
        ))
        .expect("capture the fixed Go user-wire contracts first"),
    )
    .unwrap();
    assert_eq!(cases.len(), 177);
    for case in cases {
        let requests = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                operation: case.operation.clone(),
                body: case.body,
                requests: requests.clone(),
            },
        );
        let result = if case.operation == "User" {
            client.user(UserRequest { user_id: 31 }).await.map(|user| {
                (
                    serde_json::to_value(UserDetailDto::from(&user)).unwrap(),
                    Value::Null,
                )
            })
        } else {
            assert_eq!(case.operation, "SearchUsers");
            client
                .search_users(SearchUsersRequest {
                    word: "artist".into(),
                    ..Default::default()
                })
                .await
                .map(|page| {
                    let cursor = if page.next.is_zero() {
                        Value::Null
                    } else {
                        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(page.next.as_str()).unwrap())
                            .unwrap()
                    };
                    (
                        serde_json::to_value(
                            page.items
                                .iter()
                                .map(UserPreviewDto::from)
                                .collect::<Vec<_>>(),
                        )
                        .unwrap(),
                        cursor,
                    )
                })
        };
        match result {
            Ok((dto, cursor)) => {
                assert!(
                    case.reason.is_empty(),
                    "{} expected {}",
                    case.name,
                    case.message
                );
                assert_eq!(dto, case.dto, "{} dto", case.name);
                assert_eq!(cursor, case.cursor, "{} cursor", case.name);
            }
            Err(error) => {
                assert_eq!(
                    pixiv_sdk::error::reason_of(&error).unwrap().as_str(),
                    case.reason,
                    "{} reason",
                    case.name
                );
                assert_eq!(error.to_string(), case.message, "{} message", case.name);
            }
        }
        assert_eq!(
            serde_json::to_value(&*requests.lock().unwrap()).unwrap(),
            case.requests,
            "{} requests",
            case.name
        );
    }
}
