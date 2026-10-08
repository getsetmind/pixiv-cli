use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pixiv_sdk::{
    Client, Result,
    cursor::Cursor,
    dto::ArtworkDto,
    oauth,
    pixiv::SearchArtworksRequest,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Deserialize)]
struct Input {
    word: String,
    target: String,
    sort: String,
    duration: String,
    start_date: String,
    end_date: String,
    content_type: String,
    ai_mode: String,
    aspect_ratio: String,
    resolution: String,
    tool: String,
    bookmark_min: Option<i64>,
    bookmark_max: Option<i64>,
    cursor_context: String,
}
impl Input {
    fn request(&self, cursor: Cursor) -> SearchArtworksRequest {
        SearchArtworksRequest {
            word: self.word.clone(),
            target: self.target.clone(),
            sort: self.sort.clone(),
            duration: self.duration.clone(),
            start_date: self.start_date.clone(),
            end_date: self.end_date.clone(),
            content_type: self.content_type.clone(),
            ai_mode: self.ai_mode.clone(),
            aspect_ratio: self.aspect_ratio.clone(),
            resolution: self.resolution.clone(),
            tool: self.tool.clone(),
            bookmark_min: self.bookmark_min,
            bookmark_max: self.bookmark_max,
            cursor_context: self.cursor_context.clone(),
            cursor,
        }
    }
}
#[derive(Deserialize)]
struct Step {
    action: String,
    consumed: i64,
    mode: String,
    word: String,
    context: String,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    input: Input,
    user_id: i64,
    bodies: Vec<Value>,
    steps: Vec<Step>,
    results: Vec<Value>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
}
type Queries = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
struct Fixture {
    bodies: Vec<Value>,
    user_id: i64,
    seen: Queries,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> Result<Response> {
        let body = if request.url == "https://oauth.secure.pixiv.net/auth/token" {
            assert_eq!(request.method, reqwest::Method::POST);
            json!({"access_token":"fixture-access", "refresh_token":"fixture-rotated", "expires_in":3600, "user":{"id":self.user_id,"name":"fixture"}})
        } else {
            assert_eq!(request.method, reqwest::Method::GET);
            assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/illust");
            assert_eq!(request.operation, "SearchArtworks");
            let mut query = BTreeMap::<String, Vec<String>>::new();
            for (key, value) in request.parameters {
                query.entry(key).or_default().push(value);
            }
            let index = match query
                .get("offset")
                .and_then(|v| v.first())
                .map(String::as_str)
            {
                Some("30") => 1,
                Some("60") => 2,
                _ => 0,
            };
            self.seen.lock().unwrap().push(query);
            self.bodies.get(index).unwrap_or(&self.bodies[0]).clone()
        };
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}
async fn client(case: &Case, user_id: i64, seen: &Queries) -> Client<Fixture> {
    let transport = Fixture {
        bodies: case.bodies.clone(),
        user_id,
        seen: seen.clone(),
    };
    if user_id > 0 {
        let credentials = oauth::refresh(&transport, "fixture-refresh").await.unwrap();
        Client::from_credentials(&credentials, transport)
    } else {
        Client::with_transport("fixture-access", transport)
    }
}
fn envelope(cursor: &Cursor) -> Value {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor.as_str()).unwrap()).unwrap()
}
fn normalized(cursor: &Cursor) -> Value {
    if cursor.is_zero() {
        return Value::Null;
    }
    let mut value = envelope(cursor);
    if let Some(instance) = value.get_mut("i") {
        let raw = instance.as_str().unwrap();
        assert_eq!(raw.len(), 32);
        assert!(raw.bytes().all(|byte| byte.is_ascii_hexdigit()));
        *instance = json!("instance");
    }
    value
}
fn mutated(cursor: &Cursor, mode: &str) -> Cursor {
    let mut value = envelope(cursor);
    match mode {
        "legacy" => value["b"] = json!(1),
        "kind" => value["pl"] = json!(STANDARD.encode(br#"{"k":"max_bookmark_id","v":30}"#)),
        "identity" => {
            let map = value.as_object_mut().unwrap();
            map.remove("i");
            map.remove("e");
            map.insert("id".into(), json!("99"));
        }
        _ => unreachable!(),
    }
    Cursor::parse(&URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())).unwrap()
}

#[tokio::test]
async fn search_filters_pages_and_identity_bound_checkpoints_match_frozen_go() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-artworks.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 118);
    for case in cases {
        let seen = Arc::new(Mutex::new(vec![]));
        let original = client(&case, case.user_id, &seen).await;
        let mut current = Cursor::default();
        let mut next = Cursor::default();
        for (index, step) in case.steps.iter().enumerate() {
            let mut request = case.input.request(current.clone());
            if !step.word.is_empty() {
                request.word.clone_from(&step.word);
            }
            if !step.context.is_empty() {
                request.cursor_context.clone_from(&step.context);
            }
            let other = match step.mode.as_str() {
                "same" => Some(client(&case, case.user_id, &seen).await),
                "foreign" => Some(
                    client(
                        &case,
                        if case.user_id > 0 {
                            case.user_id + 1
                        } else {
                            0
                        },
                        &seen,
                    )
                    .await,
                ),
                "next" => {
                    current = next.clone();
                    request.cursor = next.clone();
                    None
                }
                "legacy" | "kind" | "identity" => {
                    request.cursor = mutated(&current, &step.mode);
                    None
                }
                _ => None,
            };
            let target = other.as_ref().unwrap_or(&original);
            let mut actual = json!({"items":[], "cursor":null, "reason":"", "message":""});
            let result = if step.action == "checkpoint" {
                target
                    .checkpoint_search_artworks(request, step.consumed)
                    .map(|cursor| {
                        actual["cursor"] = normalized(&cursor);
                        current = cursor;
                    })
            } else {
                target.search_artworks(request).await.map(|page| {
                    actual["items"] = serde_json::to_value(
                        page.items.iter().map(ArtworkDto::from).collect::<Vec<_>>(),
                    )
                    .unwrap();
                    actual["cursor"] = normalized(&page.next);
                    next = page.next;
                })
            };
            if let Err(error) = result {
                actual["reason"] = json!(pixiv_sdk::error::reason_of(&error).unwrap().as_str());
                actual["message"] = json!(error.to_string());
            }
            assert_eq!(actual, case.results[index], "{} step {index}", case.name);
        }
        assert_eq!(*seen.lock().unwrap(), case.queries, "{} queries", case.name);
    }
}
