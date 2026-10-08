use pixiv_sdk::{
    Client, Error, Reason, Result,
    cursor::Cursor,
    dto::BookmarkTagDto,
    pixiv::UserArtworkBookmarkTagsRequest,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Deserialize)]
struct Case {
    id: i64,
    restrict: String,
    cursor: String,
    bodies: Option<Vec<String>>,
    items: Vec<Value>,
    next: Vec<String>,
    error: Option<Value>,
    requests: Vec<String>,
}
struct Fixture {
    bodies: Vec<String>,
    seen: Arc<Mutex<Vec<String>>>,
}
impl Transport for Fixture {
    async fn send(&self, mut request: Request) -> Result<Response> {
        request.parameters.sort_by(|a, b| a.0.cmp(&b.0));
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&request.parameters)
            .finish();
        let path = url::Url::parse(&request.url).unwrap().path().to_owned();
        let mut seen = self.seen.lock().unwrap();
        let index = seen.len();
        seen.push(format!("{} {path}?{query}", request.method));
        let body = serde_json::from_str(&self.bodies[index])
            .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, request.operation))?;
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}
#[tokio::test]
async fn artwork_bookmark_tag_pages_and_bound_cursors_match_go_without_replaying_next_urls() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/artwork-bookmark-tags.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 61);
    for case in cases {
        let seen = Arc::new(Mutex::new(vec![]));
        let bodies = case.bodies.unwrap_or_default();
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                bodies: bodies.clone(),
                seen: seen.clone(),
            },
        );
        let mut cursor = if case.cursor.is_empty() {
            Cursor::default()
        } else {
            Cursor::parse(&case.cursor).unwrap()
        };
        let mut items = vec![];
        let mut next = vec![];
        let mut error = None;
        for _ in 0..bodies.len().max(1) {
            match client
                .user_artwork_bookmark_tags(UserArtworkBookmarkTagsRequest {
                    user_id: case.id,
                    restrict: case.restrict.clone(),
                    cursor,
                })
                .await
            {
                Ok(page) => {
                    items.push(
                        serde_json::to_value(
                            page.items
                                .iter()
                                .map(BookmarkTagDto::from)
                                .collect::<Vec<_>>(),
                        )
                        .unwrap(),
                    );
                    next.push(page.next.to_string());
                    cursor = page.next;
                }
                Err(value) => {
                    error = Some(
                        json!({"Reason":value.code.as_str(),"Product":value.product,"Operation":value.operation,"Detail":value.detail.unwrap_or_default(),"HTTPStatus":value.http_status.unwrap_or_default(),"Transport":value.transport.map(|kind|serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()).unwrap_or_default(),"Retry":{"Safe":value.retry.safe,"After":value.retry.after.map(|date|date.to_rfc3339()).unwrap_or_else(||"0001-01-01T00:00:00Z".into()),"HasAfter":value.retry.after.is_some()}}),
                    );
                    break;
                }
            }
        }
        assert_eq!(items, case.items, "bodies={bodies:?}");
        assert_eq!(next, case.next, "bodies={bodies:?}");
        assert_eq!(
            error, case.error,
            "bodies={bodies:?} cursor={}",
            case.cursor
        );
        assert_eq!(*seen.lock().unwrap(), case.requests);
    }
}
