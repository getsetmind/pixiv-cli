use pixiv_sdk::{
    Client, Reason,
    dto::UserDetailDto,
    oauth,
    pixiv::CurrentUserRequest,
    transport::{JsonResponse, Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Deserialize)]
struct Case {
    name: String,
    source: String,
    source_case: String,
    verified: bool,
    status: u16,
    username: String,
    id: i64,
    dto: Value,
    message: String,
    requests: Value,
}
struct Fixture {
    body: Vec<u8>,
    status: u16,
    requests: Arc<Mutex<Vec<Value>>>,
    oauth_calls: Arc<AtomicUsize>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.operation, "Open");
        assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
        self.oauth_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({"access_token":"new-access-token","refresh_token":"new-refresh-token","expires_in":3600,"user":{"id":42,"name":"tester"}}),
        })
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        assert_eq!(request.operation, "CurrentUser");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/user/detail");
        assert!(
            request
                .headers
                .iter()
                .any(|(key, value)| key == "Authorization" && value == "Bearer new-access-token")
        );
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        self.requests
            .lock()
            .unwrap()
            .push(json!({"method":request.method.as_str(),"path":"/v1/user/detail","query":query,"user_id_header":request.headers.iter().find(|(key,_)| key.eq_ignore_ascii_case("X-User-Id")).map(|(_,value)| value.as_str()).unwrap_or("")}));
        Ok(JsonResponse {
            status: self.status,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}
fn shared_body(case: &Case) -> Vec<u8> {
    if case.source.is_empty() {
        return br#"{"error":{"message":"fixture failure"}}"#.to_vec();
    }
    let source = match case.source.as_str() {
        "user-detail.json" => include_str!("../../../docs/migration/contracts/user-detail.json"),
        "user-wire.json" => include_str!("../../../docs/migration/contracts/user-wire.json"),
        other => panic!("unknown fixture {other}"),
    };
    let rows: Vec<Value> = serde_json::from_str(source).unwrap();
    let row = rows
        .iter()
        .find(|row| {
            row["name"] == case.source_case
                && (row["operation"].is_null() || row["operation"] == "User")
        })
        .unwrap();
    if case.source == "user-wire.json" {
        row["body"].as_str().unwrap().as_bytes().to_vec()
    } else {
        serde_json::to_vec(&row["body"]).unwrap()
    }
}
#[tokio::test]
async fn current_user_preserves_go_verified_identity_requests_shared_detail_and_errors() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/current-user.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 9);
    for case in cases {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let oauth_calls = Arc::new(AtomicUsize::new(0));
        let transport = Fixture {
            body: shared_body(&case),
            status: case.status,
            requests: requests.clone(),
            oauth_calls: oauth_calls.clone(),
        };
        let client = if case.verified {
            let mut credentials = oauth::refresh(&transport, "fixture-refresh").await.unwrap();
            let client = Client::from_credentials(&credentials, transport);
            credentials.username = "later credential name".into();
            credentials.user_id = 999;
            client
        } else {
            Client::with_transport("", transport)
                .with_expiry(chrono::Utc::now() - chrono::TimeDelta::seconds(1))
        };
        let client = Arc::new(client);
        let cloned = Arc::clone(&client);
        assert_eq!(
            cloned.username(),
            case.username,
            "{} cached username",
            case.name
        );
        assert_eq!(cloned.user_id(), case.id, "{} cached ID", case.name);
        assert_eq!(
            oauth_calls.load(Ordering::SeqCst),
            usize::from(case.verified)
        );
        assert!(requests.lock().unwrap().is_empty());
        let debug = format!("{cloned:?}");
        assert!(!debug.contains("new-access-token"));
        assert!(!debug.contains("tester"));
        let result = cloned.current_user(CurrentUserRequest::default()).await;
        match result {
            Ok(detail) => {
                assert!(
                    case.message.is_empty(),
                    "{}: expected {}",
                    case.name,
                    case.message
                );
                assert_eq!(
                    serde_json::to_value(UserDetailDto::from(&detail)).unwrap(),
                    case.dto,
                    "{} DTO",
                    case.name
                );
            }
            Err(error) => {
                assert_eq!(error.to_string(), case.message, "{} error", case.name);
            }
        }
        assert_eq!(
            cloned.username(),
            case.username,
            "{} after detail",
            case.name
        );
        assert_eq!(
            serde_json::to_value(&*requests.lock().unwrap()).unwrap(),
            case.requests,
            "{} requests",
            case.name
        );
        assert_eq!(
            oauth_calls.load(Ordering::SeqCst),
            usize::from(case.verified)
        );
    }
}

struct NoRequests;
impl Transport for NoRequests {
    async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
        panic!("identity accessors or unknown current user reached transport")
    }
}
#[tokio::test]
async fn new_clients_have_empty_cached_username_and_reject_unknown_identity_before_transport() {
    for token in ["fixture-access", ""] {
        let client = Client::with_transport(token, NoRequests);
        assert_eq!(client.username(), "");
        assert_eq!(client.user_id(), 0);
        let error = client
            .current_user(CurrentUserRequest {})
            .await
            .unwrap_err();
        assert_eq!(error.code, Reason::Unauthorized);
        assert_eq!(
            error.to_string(),
            "pixiv:CurrentUser: unauthorized: current user identity is unknown"
        );
    }
}

struct IdentityHeaders {
    verified: bool,
    requests: Arc<AtomicUsize>,
}
impl Transport for IdentityHeaders {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"access_token":"fixture-access","refresh_token":"fixture-refresh","expires_in":3600,"user":{"id":42,"name":"tester"}}),
            });
        }
        let header = request
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("X-User-Id"))
            .map(|(_, value)| value.as_str());
        assert_eq!(
            header,
            if self.verified { Some("42") } else { None },
            "{} identity header",
            request.operation
        );
        self.requests.fetch_add(1, Ordering::SeqCst);
        let body = match request.operation {
            "User" => {
                assert_eq!(request.url, "https://app-api.pixiv.net/v1/user/detail");
                json!({"user":{"id":31},"profile":{},"profile_publicity":{},"workspace":{}})
            }
            "FollowingArtworks" => {
                assert_eq!(request.url, "https://app-api.pixiv.net/v2/illust/follow");
                json!({"illusts":[],"next_url":null})
            }
            other => panic!("unexpected identity request {other}"),
        };
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}
#[tokio::test]
async fn user_and_feed_get_headers_use_verified_identity_and_omit_unknown_identity() {
    for verified in [false, true] {
        let requests = Arc::new(AtomicUsize::new(0));
        let transport = IdentityHeaders {
            verified,
            requests: requests.clone(),
        };
        let client = if verified {
            let credentials = oauth::refresh(&transport, "fixture-refresh").await.unwrap();
            Client::from_credentials(&credentials, transport)
        } else {
            Client::with_transport("fixture-access", transport)
        };
        client
            .user(pixiv_sdk::pixiv::UserRequest { user_id: 31 })
            .await
            .unwrap();
        client
            .following_artworks(pixiv_sdk::pixiv::FollowingArtworksRequest::default())
            .await
            .unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }
}
