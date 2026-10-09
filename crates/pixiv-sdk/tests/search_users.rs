use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    dto::UserPreviewDto,
    pixiv::SearchUsersRequest,
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
    word: String,
    cursor: String,
    next_word: String,
    bodies: Vec<Value>,
    results: Vec<Value>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
}
type Queries = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
struct Fixture {
    bodies: Vec<Value>,
    queries: Queries,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/user");
        assert_eq!(request.operation, "SearchUsers");
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let index = usize::from(self.bodies.len() > 1 && query.contains_key("offset"));
        self.queries.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}

#[tokio::test]
async fn user_search_matches_go_conditions_dtos_pages_and_global_cursor_binding() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/user-search.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 64);
    for case in cases {
        let queries = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                bodies: case.bodies,
                queries: queries.clone(),
            },
        );
        let mut request = SearchUsersRequest {
            word: case.word,
            cursor: if case.cursor.is_empty() {
                Cursor::default()
            } else {
                Cursor::parse(&case.cursor).unwrap()
            },
        };
        for (index, expected) in case.results.iter().enumerate() {
            let mut actual = json!({"items":[],"cursor":null});
            match client.search_users(request.clone()).await {
                Ok(page) => {
                    actual["items"] = serde_json::to_value(
                        page.items
                            .iter()
                            .map(UserPreviewDto::from)
                            .collect::<Vec<_>>(),
                    )
                    .unwrap();
                    if !page.next.is_zero() {
                        actual["cursor"] = serde_json::from_slice(
                            &URL_SAFE_NO_PAD.decode(page.next.as_str()).unwrap(),
                        )
                        .unwrap();
                    }
                    request.cursor = page.next;
                }
                Err(error) => {
                    actual["reason"] = json!(pixiv_sdk::error::reason_of(&error).unwrap().as_str());
                    actual["message"] = json!(error.to_string());
                }
            }
            assert_eq!(&actual, expected, "{} step {index}", case.name);
            if !case.next_word.is_empty() {
                request.word = case.next_word.clone();
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
