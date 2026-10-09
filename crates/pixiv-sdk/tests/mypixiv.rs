use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    dto::{ArtworkDto, NovelDto, UserPreviewDto},
    pixiv::{MyPixivArtworksRequest, MyPixivNovelsRequest, MyPixivUsersRequest},
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
    cursor: String,
    next_mode: String,
    bodies: Vec<Box<serde_json::value::RawValue>>,
    results: Vec<Value>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    raw_queries: Vec<String>,
}
type Queries = Arc<Mutex<Vec<Vec<(String, String)>>>>;
#[derive(Clone)]
struct Fixture {
    operation: String,
    user_id: i64,
    bodies: Vec<Box<serde_json::value::RawValue>>,
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
        let endpoint = endpoint(&self.operation);
        assert_eq!(request.url, format!("https://app-api.pixiv.net{endpoint}"));
        let mut queries = self.queries.lock().unwrap();
        let index = queries.len().min(self.bodies.len() - 1);
        queries.push(request.parameters);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: serde_json::from_str(self.bodies[index].get()).unwrap(),
        })
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.operation, self.operation);
        assert_eq!(
            request.url,
            format!("https://app-api.pixiv.net{}", endpoint(&self.operation))
        );
        let mut queries = self.queries.lock().unwrap();
        let index = queries.len().min(self.bodies.len() - 1);
        queries.push(request.parameters);
        Ok(JsonResponse {
            status: 200,
            retry_after: None,
            body: self.bodies[index].get().as_bytes().to_vec(),
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
async fn my_pixiv_matches_go_dtos_validation_pages_and_cursor_bindings() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/mypixiv.json")).unwrap();
    assert_eq!(cases.len(), 195);
    for case in cases {
        let queries = Arc::new(Mutex::new(vec![]));
        let fixture = Fixture {
            operation: case.operation.clone(),
            user_id: case.user_id,
            bodies: case.bodies,
            queries: queries.clone(),
        };
        let mut client = make_client(fixture.clone()).await;
        assert_eq!(client.user_id(), case.user_id);
        let mut cursor = if case.cursor.is_empty() {
            Cursor::default()
        } else {
            Cursor::parse(&case.cursor).unwrap()
        };
        for (index, expected) in case.results.iter().enumerate() {
            let mut actual = json!({"items":[],"cursor":null,"reason":"","message":""});
            let result = match case.operation.as_str() {
                "MyPixivArtworks" => client
                    .my_pixiv_artworks(MyPixivArtworksRequest {
                        cursor: cursor.clone(),
                    })
                    .await
                    .map(|page| {
                        (
                            serde_json::to_value(
                                page.items.iter().map(ArtworkDto::from).collect::<Vec<_>>(),
                            )
                            .unwrap(),
                            page.next,
                        )
                    }),
                "MyPixivNovels" => client
                    .my_pixiv_novels(MyPixivNovelsRequest {
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
                    }),
                "MyPixivUsers" => client
                    .my_pixiv_users(MyPixivUsersRequest {
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
                    }),
                _ => panic!("unknown operation"),
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
                "same_account" | "other_client" => client = make_client(fixture.clone()).await,
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

fn endpoint(operation: &str) -> &'static str {
    match operation {
        "MyPixivArtworks" => "/v2/illust/mypixiv",
        "MyPixivNovels" => "/v1/novel/mypixiv",
        "MyPixivUsers" => "/v1/user/mypixiv",
        _ => panic!("unknown operation"),
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
async fn my_pixiv_resources_remain_openable_without_detail_refetch() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/mypixiv.json")).unwrap();
    for operation in ["MyPixivArtworks", "MyPixivNovels"] {
        let case = cases
            .iter()
            .find(|case| case.name == format!("{operation}:next:<nil>"))
            .unwrap();
        let api_requests = Arc::new(Mutex::new(vec![]));
        let resource_urls = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            ResourceFixture {
                body: serde_json::from_str(case.bodies[0].get()).unwrap(),
                api_requests: api_requests.clone(),
                resource_urls: resource_urls.clone(),
            },
        );
        let resources = match operation {
            "MyPixivArtworks" => {
                let page = client
                    .my_pixiv_artworks(MyPixivArtworksRequest::default())
                    .await
                    .unwrap();
                assert!(page.items[0].pages.is_empty());
                vec![page.items[0].cover.resource.clone()]
            }
            "MyPixivNovels" => {
                let page = client
                    .my_pixiv_novels(MyPixivNovelsRequest::default())
                    .await
                    .unwrap();
                vec![
                    page.items[0].cover.resource.clone(),
                    page.items[0].user.profile_image.resource.clone(),
                ]
            }
            _ => unreachable!(),
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
            *api_requests.lock().unwrap(),
            vec![format!("https://app-api.pixiv.net{}", endpoint(operation))]
        );
        let expected = if operation.ends_with("Artworks") {
            vec!["https://i.pximg.net/art.jpg"]
        } else {
            vec![
                "https://i.pximg.net/novel.jpg",
                "https://i.pximg.net/profile.jpg",
            ]
        };
        assert_eq!(*resource_urls.lock().unwrap(), expected);
    }
}
