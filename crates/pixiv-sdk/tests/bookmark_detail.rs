use pixiv_sdk::{
    Client, Error, Reason, Result,
    dto::*,
    models::*,
    pixiv::*,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Deserialize)]
struct Case {
    novel: bool,
    id: i64,
    status: u16,
    body: String,
    readback: bool,
    dtos: Vec<Value>,
    error: Option<Value>,
    requests: Vec<String>,
}
struct Fixture {
    status: u16,
    body: String,
    readback: bool,
    seen: Arc<Mutex<(Vec<String>, bool)>>,
}
impl Transport for Fixture {
    async fn send(&self, mut request: Request) -> Result<Response> {
        let mut seen = self.seen.lock().unwrap();
        request.parameters.sort_by(|a, b| a.0.cmp(&b.0));
        let parameters = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&request.parameters)
            .finish();
        let path = url::Url::parse(&request.url).unwrap().path().to_owned();
        if request.method == reqwest::Method::POST {
            seen.0.push(format!("POST {path} {parameters}"));
            seen.1 = path.ends_with("/add");
            return Ok(Response {
                status: self.status,
                retry_after: None,
                body: Value::Null,
            });
        }
        seen.0.push(format!("GET {path}?{parameters}"));
        let body = if self.readback {
            json!({"bookmark_detail":{"is_bookmarked":seen.1,"restrict":if seen.1{"private"}else{"public"},"tags":[{"name":if seen.1{"favorite"}else{"work-tag"},"is_registered":true}]}})
        } else if (200..300).contains(&self.status) {
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
async fn bookmark_states_dtos_and_add_read_remove_read_sequence_match_go() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/bookmark-detail.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 60);
    for case in cases {
        let seen = Arc::new(Mutex::new((vec![], false)));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                status: case.status,
                body: case.body,
                readback: case.readback,
                seen: seen.clone(),
            },
        );
        let mut dtos = vec![];
        if case.readback {
            if case.novel {
                client
                    .add_novel_bookmark(AddNovelBookmarkRequest {
                        novel_id: case.id,
                        restrict: "private".into(),
                        tags: vec!["favorite".into()],
                    })
                    .await
                    .unwrap();
                dtos.push(
                    serde_json::to_value(NovelBookmarkDetailDto::from(
                        &client
                            .novel_bookmark(NovelBookmarkRequest { novel_id: case.id })
                            .await
                            .unwrap(),
                    ))
                    .unwrap(),
                );
                client
                    .remove_novel_bookmark(RemoveNovelBookmarkRequest { novel_id: case.id })
                    .await
                    .unwrap();
            } else {
                client
                    .add_artwork_bookmark(AddArtworkBookmarkRequest {
                        artwork_id: case.id,
                        restrict: "private".into(),
                        tags: vec!["favorite".into()],
                    })
                    .await
                    .unwrap();
                dtos.push(
                    serde_json::to_value(ArtworkBookmarkDetailDto::from(
                        &client
                            .artwork_bookmark(ArtworkBookmarkRequest {
                                artwork_id: case.id,
                            })
                            .await
                            .unwrap(),
                    ))
                    .unwrap(),
                );
                client
                    .remove_artwork_bookmark(RemoveArtworkBookmarkRequest {
                        artwork_id: case.id,
                    })
                    .await
                    .unwrap();
            }
        }
        let result = if case.novel {
            client
                .novel_bookmark(NovelBookmarkRequest { novel_id: case.id })
                .await
                .map(|detail| serde_json::to_value(NovelBookmarkDetailDto::from(&detail)).unwrap())
        } else {
            client
                .artwork_bookmark(ArtworkBookmarkRequest {
                    artwork_id: case.id,
                })
                .await
                .map(|detail| {
                    serde_json::to_value(ArtworkBookmarkDetailDto::from(&detail)).unwrap()
                })
        };
        let error = match result {
            Ok(dto) => {
                dtos.push(dto);
                None
            }
            Err(error) => Some(
                json!({"Reason":error.code.as_str(),"Product":error.product,"Operation":error.operation,"Detail":error.detail.unwrap_or_default(),"HTTPStatus":error.http_status.unwrap_or_default(),"Transport":error.transport.map(|kind|serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()).unwrap_or_default(),"Retry":{"Safe":error.retry.safe,"After":error.retry.after.map(|date|date.to_rfc3339()).unwrap_or_else(||"0001-01-01T00:00:00Z".into()),"HasAfter":error.retry.after.is_some()}}),
            ),
        };
        assert_eq!(
            dtos, case.dtos,
            "novel={} status={}",
            case.novel, case.status
        );
        assert_eq!(error, case.error);
        assert_eq!(seen.lock().unwrap().0, case.requests);
    }
}
#[test]
fn bookmark_detail_dto_copies_tags_and_serializes_empty_tags_as_null() {
    let detail = ArtworkBookmarkDetail {
        restrict: "future".into(),
        tags: vec!["favorite".into()],
    };
    let mut dto = ArtworkBookmarkDetailDto::from(&detail);
    dto.tags.as_mut().unwrap()[0] = "changed".into();
    assert_eq!(detail.tags, vec!["favorite"]);
    assert_eq!(
        serde_json::to_value(ArtworkBookmarkDetailDto::from(
            &ArtworkBookmarkDetail::default()
        ))
        .unwrap(),
        json!({"restrict":"","tags":null})
    );
}
