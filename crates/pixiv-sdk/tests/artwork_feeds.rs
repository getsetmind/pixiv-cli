use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    dto::ArtworkDto,
    pixiv::{RecommendedArtworksRequest, RelatedArtworksRequest},
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
    id: i64,
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
        let endpoint = if self.operation == "RelatedArtworks" {
            "/v2/illust/related"
        } else {
            "/v1/illust/recommended"
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
        "../../../docs/migration/contracts/artwork-feeds.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 155);
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
        let mut id = case.id;
        for (index, expected) in case.results.iter().enumerate() {
            let mut actual = json!({"items":[],"cursor":null,"reason":"","message":""});
            let result = if case.operation == "RelatedArtworks" {
                client
                    .related_artworks(RelatedArtworksRequest {
                        artwork_id: id,
                        cursor: cursor.clone(),
                    })
                    .await
            } else {
                client
                    .recommended_artworks(RecommendedArtworksRequest {
                        cursor: cursor.clone(),
                    })
                    .await
            };
            match result {
                Ok(page) => {
                    actual["items"] = serde_json::to_value(
                        page.items.iter().map(ArtworkDto::from).collect::<Vec<_>>(),
                    )
                    .unwrap();
                    actual["cursor"] = cursor_value(&page.next);
                    cursor = page.next;
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
            if case.next_mode == "other_query" {
                id += 1;
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
