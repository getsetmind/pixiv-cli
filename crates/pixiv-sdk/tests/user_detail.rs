use pixiv_sdk::{
    Client,
    dto::UserDetailDto,
    pixiv::UserRequest,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Deserialize)]
struct Case {
    name: String,
    id: i64,
    body: Value,
    dto: Value,
    reason: String,
    message: String,
    requests: Value,
}
struct Fixture {
    body: Value,
    requests: Arc<Mutex<Vec<Value>>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/user/detail");
        assert_eq!(request.operation, "User");
        let query: std::collections::BTreeMap<_, _> = request
            .parameters
            .iter()
            .map(|(k, v)| (k.clone(), vec![v.clone()]))
            .collect();
        self.requests
            .lock()
            .unwrap()
            .push(json!({"method":request.method.as_str(),"path":"/v1/user/detail","query":query}));
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}
#[tokio::test]
async fn user_detail_preserves_go_fields_visibility_errors_and_requests() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/user-detail.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 145);
    for case in cases {
        let requests = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                body: case.body,
                requests: requests.clone(),
            },
        );
        match client.user(UserRequest { user_id: case.id }).await {
            Ok(user) => {
                assert!(
                    case.reason.is_empty(),
                    "{} expected {}",
                    case.name,
                    case.message
                );
                assert_eq!(
                    serde_json::to_value(UserDetailDto::from(&user)).unwrap(),
                    case.dto,
                    "{} dto",
                    case.name
                );
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
