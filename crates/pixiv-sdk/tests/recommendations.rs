use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    dto::{NovelDto, UserPreviewDto},
    pixiv::{RecommendedNovelsRequest, RecommendedUsersRequest},
    transport::{Request, Response, Transport},
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
    cursor: String,
    next_mode: String,
    bodies: Vec<Value>,
    results: Vec<Value>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    raw_queries: Vec<String>,
}
type Queries = Arc<Mutex<Vec<Vec<(String, String)>>>>;
#[derive(Clone)]
struct Fixture {
    operation: String,
    user_id: i64,
    bodies: Vec<Value>,
    queries: Queries,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.url == "https://oauth.secure.pixiv.net/auth/token" {
            assert_eq!(request.method.as_str(), "POST");
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":self.user_id,"name":"fixture"}}),
            });
        }
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.operation, self.operation);
        let endpoint = if self.operation == "RecommendedUsers" {
            "/v1/user/recommended"
        } else {
            "/v1/novel/recommended"
        };
        assert_eq!(request.url, format!("https://app-api.pixiv.net{endpoint}"));
        let mut queries = self.queries.lock().unwrap();
        let index = queries.len().min(self.bodies.len() - 1);
        queries.push(request.parameters);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}
async fn make_client(fixture: Fixture) -> Client<Fixture> {
    if fixture.user_id > 0 {
        let credentials = pixiv_sdk::oauth::refresh(&fixture, "fixture-refresh")
            .await
            .unwrap();
        Client::from_credentials(&credentials, fixture)
    } else {
        Client::with_transport("fixture-access", fixture)
    }
}
fn cursor_value(cursor: &Cursor) -> Value {
    if cursor.is_zero() {
        return Value::Null;
    }
    let mut value: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor.as_str()).unwrap()).unwrap();
    if value.get("i").is_some() {
        value["i"] = json!("instance");
    }
    value
}
#[tokio::test]
async fn feeds_match_frozen_go_dtos_structured_cursors_validation_and_exact_queries() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/novel-user-recommendations.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 166);
    for case in cases {
        let queries = Arc::new(Mutex::new(vec![]));
        let fixture = Fixture {
            operation: case.operation.clone(),
            user_id: case.user_id,
            bodies: case.bodies,
            queries: queries.clone(),
        };
        let mut client = make_client(fixture.clone()).await;
        let mut cursor = if case.cursor.is_empty() {
            Cursor::default()
        } else {
            Cursor::parse(&case.cursor).unwrap()
        };
        for (index, expected) in case.results.iter().enumerate() {
            let mut actual = json!({"items":[],"cursor":null,"reason":"","message":""});
            let result = if case.operation == "RecommendedUsers" {
                client
                    .recommended_users(RecommendedUsersRequest {
                        cursor: cursor.clone(),
                    })
                    .await
                    .map(|page| {
                        (
                            serde_json::to_value(
                                page.items
                                    .iter()
                                    .map(UserPreviewDto::from)
                                    .collect::<Vec<_>>(),
                            )
                            .unwrap(),
                            page.next,
                        )
                    })
            } else {
                client
                    .recommended_novels(RecommendedNovelsRequest {
                        cursor: cursor.clone(),
                    })
                    .await
                    .map(|page| {
                        (
                            serde_json::to_value(
                                page.items.iter().map(NovelDto::from).collect::<Vec<_>>(),
                            )
                            .unwrap(),
                            page.next,
                        )
                    })
            };
            match result {
                Ok((items, next)) => {
                    actual["items"] = items;
                    actual["cursor"] = cursor_value(&next);
                    cursor = next;
                }
                Err(error) => {
                    actual["reason"] = json!(pixiv_sdk::error::reason_of(&error).unwrap().as_str());
                    actual["message"] = json!(error.to_string());
                }
            }
            assert_eq!(&actual, expected, "{} step {index}", case.name);
            match case.next_mode.as_str() {
                "same_account" => client = make_client(fixture.clone()).await,
                "other_account" | "anonymous_account" => {
                    let mut next_fixture = fixture.clone();
                    next_fixture.user_id = if case.next_mode == "other_account" {
                        8
                    } else {
                        0
                    };
                    client = make_client(next_fixture).await;
                }
                _ => {}
            }
            if case.next_mode == "other_client" {
                client = Client::with_transport("fixture-access", fixture.clone());
            }
        }
        let actual = queries.lock().unwrap();
        let maps: Vec<_> = actual
            .iter()
            .map(|query| {
                let mut map = BTreeMap::<String, Vec<String>>::new();
                for (key, value) in query {
                    map.entry(key.clone()).or_default().push(value.clone());
                }
                map
            })
            .collect();
        assert_eq!(maps, case.queries, "{} query multiplicity", case.name);
        let raw: Vec<_> = actual
            .iter()
            .map(|query| {
                url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(query)
                    .finish()
            })
            .collect();
        assert_eq!(raw, case.raw_queries, "{} ordered wire query", case.name);
    }
}

#[derive(Clone)]
struct ResourceFixture {
    body: Value,
    api_requests: Arc<Mutex<Vec<String>>>,
    resource_urls: Arc<Mutex<Vec<String>>>,
}
impl Transport for ResourceFixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        self.api_requests.lock().unwrap().push(request.url);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}
impl pixiv_sdk::transport::ResourceTransport for ResourceFixture {
    type Body = std::io::Cursor<Vec<u8>>;
    async fn open_resource(
        &self,
        request: pixiv_sdk::transport::ResourceReadRequest,
    ) -> pixiv_sdk::Result<pixiv_sdk::resource::ResourceResponse<Self::Body>> {
        request.validate.as_ref().unwrap()(&request.url)?;
        self.resource_urls.lock().unwrap().push(request.url);
        Ok(pixiv_sdk::resource::ResourceResponse::new(
            200,
            &Default::default(),
            std::io::Cursor::new(b"fixture".to_vec()),
        ))
    }
}
#[tokio::test]
async fn recommended_cover_profile_and_sample_resources_reuse_remembered_urls() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/novel-user-recommendations.json"
    ))
    .unwrap();
    for operation in ["RecommendedNovels", "RecommendedUsers"] {
        let case = cases
            .iter()
            .find(|case| case.operation == operation && case.name.ends_with(":next:<nil>"))
            .unwrap();
        let api_requests = Arc::new(Mutex::new(Vec::new()));
        let resource_urls = Arc::new(Mutex::new(Vec::new()));
        let client = Client::with_transport(
            "fixture-access",
            ResourceFixture {
                body: case.bodies[0].clone(),
                api_requests: api_requests.clone(),
                resource_urls: resource_urls.clone(),
            },
        );
        let resources = if operation == "RecommendedNovels" {
            let page = client
                .recommended_novels(RecommendedNovelsRequest::default())
                .await
                .unwrap();
            vec![
                page.items[0].cover.resource.clone(),
                page.items[0].user.profile_image.resource.clone(),
            ]
        } else {
            let page = client
                .recommended_users(RecommendedUsersRequest::default())
                .await
                .unwrap();
            let preview = &page.items[0];
            vec![
                preview.user.profile_image.resource.clone(),
                preview.illusts[0].cover.resource.clone(),
                preview.novels[0].cover.resource.clone(),
                preview.novels[0].user.profile_image.resource.clone(),
            ]
        };
        for resource in resources {
            client
                .open_resource(pixiv_sdk::resource::OpenResourceRequest {
                    reference: resource.reference.clone(),
                    ..Default::default()
                })
                .await
                .unwrap();
        }
        assert_eq!(
            api_requests.lock().unwrap().as_slice(),
            [format!(
                "https://app-api.pixiv.net/v1/{}/recommended",
                if operation == "RecommendedNovels" {
                    "novel"
                } else {
                    "user"
                }
            )]
        );
        let expected: Vec<String> = if operation == "RecommendedNovels" {
            vec![
                "https://i.pximg.net/novel.jpg",
                "https://i.pximg.net/profile.jpg",
            ]
        } else {
            vec![
                "https://i.pximg.net/profile.jpg",
                "https://i.pximg.net/art.jpg",
                "https://i.pximg.net/novel.jpg",
                "https://i.pximg.net/profile.jpg",
            ]
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(*resource_urls.lock().unwrap(), expected);
    }
}

#[derive(Deserialize)]
struct SampleWireCase {
    name: String,
    body: Value,
    result: Value,
}
#[tokio::test]
async fn frozen_sample_defaults_type_errors_and_novel_cursor_escaping_match_go() {
    let cases: Vec<SampleWireCase> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/recommendation-sample-wire-gaps.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 6);
    for case in cases
        .into_iter()
        .filter(|case| case.name != "uppercase-preview")
    {
        let operation = if case.name == "escaped-novel-cursor" {
            "RecommendedNovels"
        } else {
            "RecommendedUsers"
        };
        let fixture = Fixture {
            operation: operation.into(),
            user_id: 0,
            bodies: vec![case.body],
            queries: Arc::new(Mutex::new(Vec::new())),
        };
        let client = make_client(fixture).await;
        let result = if operation == "RecommendedNovels" {
            client
                .recommended_novels(RecommendedNovelsRequest::default())
                .await
                .map(|page| {
                    (
                        serde_json::to_value(
                            page.items.iter().map(NovelDto::from).collect::<Vec<_>>(),
                        )
                        .unwrap(),
                        page.next,
                    )
                })
        } else {
            client
                .recommended_users(RecommendedUsersRequest::default())
                .await
                .map(|page| {
                    (
                        serde_json::to_value(
                            page.items
                                .iter()
                                .map(UserPreviewDto::from)
                                .collect::<Vec<_>>(),
                        )
                        .unwrap(),
                        page.next,
                    )
                })
        };
        let mut actual = json!({"items":[],"cursor":null,"reason":"","message":""});
        match result {
            Ok((items, next)) => {
                actual["items"] = items;
                actual["cursor"] = cursor_value(&next);
            }
            Err(error) => {
                actual["reason"] = json!(error.code.as_str());
                actual["message"] = json!(error.to_string());
            }
        }
        assert_eq!(actual, case.result, "{}", case.name);
    }
}
