use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    dto::UserPreviewDto,
    pixiv::{
        RelatedUsersRequest, UserBlockedUsersRequest, UserFollowersRequest, UserFollowingRequest,
    },
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
    user_id: i64,
    identity: i64,
    restrict: String,
    cursor: String,
    change: String,
    bodies: Vec<String>,
    results: Vec<Value>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
}
type Queries = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
#[derive(Clone)]
struct Fixture {
    bodies: Vec<String>,
    queries: Queries,
    identity: i64,
    operation: String,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
        Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":self.identity}}),
        })
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        let path = match self.operation.as_str() {
            "UserFollowing" => "/v1/user/following",
            "UserFollowers" => "/v1/user/follower",
            "RelatedUsers" => "/v1/user/related",
            "UserBlockedUsers" => "/v2/user/list",
            _ => panic!("unknown operation"),
        };
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, format!("https://app-api.pixiv.net{path}"));
        assert_eq!(request.operation, self.operation);
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let mut queries = self.queries.lock().unwrap();
        let index = queries.len().min(self.bodies.len() - 1);
        queries.push(query);
        Ok(JsonResponse {
            status: 200,
            retry_after: None,
            body: self.bodies[index].as_bytes().to_vec(),
        })
    }
}
async fn client(fixture: Fixture) -> Client<Fixture> {
    if fixture.identity > 0 {
        let credentials = pixiv_sdk::oauth::refresh(&fixture, "fixture-refresh")
            .await
            .unwrap();
        Client::from_credentials(&credentials, fixture)
    } else {
        Client::with_transport("fixture-access", fixture)
    }
}
#[tokio::test]
async fn user_relationships_match_frozen_go_requests_raw_dtos_pages_and_scoped_cursors() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/user-relationships.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 524);
    for case in cases {
        let queries = Arc::new(Mutex::new(vec![]));
        let mut fixture = Fixture {
            bodies: case.bodies,
            queries: queries.clone(),
            identity: case.identity,
            operation: case.operation.clone(),
        };
        let mut client = client(fixture.clone()).await;
        let mut cursor = if case.cursor.is_empty() {
            Cursor::default()
        } else {
            Cursor::parse(&case.cursor).unwrap()
        };
        let mut user_id = case.user_id;
        let mut restrict = case.restrict;
        for (index, expected) in case.results.iter().enumerate() {
            let result = match case.operation.as_str() {
                "UserFollowing" => {
                    client
                        .user_following(UserFollowingRequest {
                            user_id,
                            restrict: restrict.clone(),
                            cursor: cursor.clone(),
                        })
                        .await
                }
                "UserFollowers" => {
                    client
                        .user_followers(UserFollowersRequest {
                            user_id,
                            restrict: restrict.clone(),
                            cursor: cursor.clone(),
                        })
                        .await
                }
                "RelatedUsers" => {
                    client
                        .related_users(RelatedUsersRequest {
                            user_id,
                            cursor: cursor.clone(),
                        })
                        .await
                }
                "UserBlockedUsers" => {
                    client
                        .user_blocked_users(UserBlockedUsersRequest {
                            user_id,
                            cursor: cursor.clone(),
                        })
                        .await
                }
                _ => panic!("unknown operation"),
            };
            let mut actual = json!({"items":[],"cursor":null});
            match result {
                Ok(page) => {
                    actual["items"] = serde_json::to_value(
                        page.items
                            .iter()
                            .map(UserPreviewDto::from)
                            .collect::<Vec<_>>(),
                    )
                    .unwrap();
                    if !page.next.is_zero() {
                        let mut value: Value = serde_json::from_slice(
                            &URL_SAFE_NO_PAD.decode(page.next.as_str()).unwrap(),
                        )
                        .unwrap();
                        if value.get("i").is_some() {
                            value["i"] = json!("instance");
                        }
                        actual["cursor"] = value;
                    }
                    cursor = page.next;
                }
                Err(error) => {
                    actual["reason"] = json!(pixiv_sdk::error::reason_of(&error).unwrap().as_str());
                    actual["message"] = json!(error.to_string());
                }
            }
            assert_eq!(&actual, expected, "{} step {index}", case.name);
            if index == 0 {
                match case.change.as_str() {
                    "user" => user_id += 1,
                    "identity" => {
                        fixture.identity += 1;
                        client = crate::client(fixture.clone()).await;
                    }
                    "instance" | "foreign-ephemeral" => {
                        fixture.identity = 0;
                        client = crate::client(fixture.clone()).await;
                    }
                    "restrict" => restrict = "private".into(),
                    _ => {}
                }
            }
        }
        assert_eq!(
            *queries.lock().unwrap(),
            case.queries,
            "{} requests",
            case.name
        );
    }
}
