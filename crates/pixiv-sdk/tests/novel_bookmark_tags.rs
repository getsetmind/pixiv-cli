use pixiv_sdk::{
    Client, Error, Reason, Result,
    cursor::Cursor,
    dto::BookmarkTagDto,
    pixiv::UserNovelBookmarkTagsRequest,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
struct Case {
    id: i64,
    restrict: String,
    cursor: bool,
    status: u16,
    body: String,
    items: Option<Value>,
    next: String,
    error: Option<Value>,
    requests: Vec<String>,
}
struct Fixture {
    status: u16,
    body: String,
    seen: Arc<Mutex<Vec<String>>>,
}
impl Transport for Fixture {
    async fn send(&self, mut request: Request) -> Result<Response> {
        request.parameters.sort_by(|a, b| a.0.cmp(&b.0));
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&request.parameters)
            .finish();
        let path = url::Url::parse(&request.url).unwrap().path().to_owned();
        self.seen
            .lock()
            .unwrap()
            .push(format!("{} {path}?{query}", request.method));
        let body = if (200..300).contains(&self.status) {
            serde_json::from_str(&self.body)
                .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, request.operation))?
        } else {
            Value::Null
        };
        Ok(Response {
            status: self.status,
            retry_after: None,
            body,
        })
    }
}
#[tokio::test]
async fn novel_bookmark_tags_preserve_required_arrays_counts_and_reject_continuation_before_io() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/novel-bookmark-tags.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 38);
    for case in cases {
        let seen = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                status: case.status,
                body: case.body.clone(),
                seen: seen.clone(),
            },
        );
        let cursor = if case.cursor {
            Cursor::new(
                "pixiv",
                "UserNovelBookmarkTags",
                1,
                "digest",
                b"payload",
                Default::default(),
            )
            .unwrap()
        } else {
            Cursor::default()
        };
        let result = client
            .user_novel_bookmark_tags(UserNovelBookmarkTagsRequest {
                user_id: case.id,
                restrict: case.restrict.clone(),
                cursor,
            })
            .await;
        let (items, next, error) = match result {
            Ok(page) => (
                Some(
                    serde_json::to_value(
                        page.items
                            .iter()
                            .map(BookmarkTagDto::from)
                            .collect::<Vec<_>>(),
                    )
                    .unwrap(),
                ),
                page.next.to_string(),
                None,
            ),
            Err(error) => (
                None,
                String::new(),
                Some(
                    json!({"Reason":error.code.as_str(),"Product":error.product,"Operation":error.operation,"Detail":error.detail.unwrap_or_default(),"HTTPStatus":error.http_status.unwrap_or_default(),"Transport":error.transport.map(|kind|serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()).unwrap_or_default(),"Retry":{"Safe":error.retry.safe,"After":error.retry.after.map(|date|date.to_rfc3339()).unwrap_or_else(||"0001-01-01T00:00:00Z".into()),"HasAfter":error.retry.after.is_some()}}),
                ),
            ),
        };
        assert_eq!(
            items, case.items,
            "body={} restrict={}",
            case.body, case.restrict
        );
        assert_eq!(next, case.next);
        assert_eq!(error, case.error);
        assert_eq!(*seen.lock().unwrap(), case.requests);
    }
}
