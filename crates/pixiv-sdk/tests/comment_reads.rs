use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    dto::{to_comment_access_control_dto, to_comment_dto, to_stamp_dto},
    pixiv::{ArtworkCommentsRequest, NovelCommentsRequest, StampsRequest},
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
    id: i64,
    operation: String,
    cursor: String,
    next_mode: String,
    bodies: Vec<Box<serde_json::value::RawValue>>,
    results: Vec<Value>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
}
type Queries = Arc<Mutex<Vec<Vec<(String, String)>>>>;
#[derive(Clone)]
struct Fixture {
    operation: String,
    bodies: Vec<Box<serde_json::value::RawValue>>,
    queries: Queries,
}
impl Transport for Fixture {
    async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
        panic!("comment reads use lossless JSON");
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.operation, self.operation);
        let endpoint = if self.operation == "Stamps" {
            "/v1/stamps"
        } else if self.operation == "ArtworkComments" {
            "/v3/illust/comments"
        } else {
            "/v2/novel/comments"
        };
        assert_eq!(request.url, format!("https://app-api.pixiv.net{endpoint}"));
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
async fn comment_reads_match_frozen_go_queries_dtos_errors_and_cursors() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/comment-reads.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 144);
    for case in cases {
        let queries = Arc::new(Mutex::new(vec![]));
        let fixture = Fixture {
            operation: case.operation.clone(),
            bodies: case.bodies,
            queries: queries.clone(),
        };
        let mut client = Client::with_transport("fixture-access", fixture.clone());
        let mut id = case.id;
        let mut cursor = if case.cursor.is_empty() {
            Cursor::default()
        } else {
            Cursor::parse(&case.cursor).unwrap()
        };
        for (index, expected) in case.results.iter().enumerate() {
            let mut operation = case.operation.as_str();
            if index > 0 {
                match case.next_mode.as_str() {
                    "other_client" => {
                        client = Client::with_transport("fixture-access", fixture.clone())
                    }
                    "changed_id" => id = 124,
                    "changed_operation" => {
                        operation = if operation == "ArtworkComments" {
                            "NovelComments"
                        } else {
                            "ArtworkComments"
                        }
                    }
                    _ => {}
                }
            }
            let mut actual = json!({"items":[],"total":null,"access_control":null,"cursor":null,"reason":"","message":""});
            let result = if operation == "Stamps" {
                client.stamps(StampsRequest {}).await.map(|items| {
                    (
                        serde_json::to_value(items.iter().map(to_stamp_dto).collect::<Vec<_>>())
                            .unwrap(),
                        None,
                        None,
                        Cursor::default(),
                    )
                })
            } else {
                let page = if operation == "ArtworkComments" {
                    client
                        .artwork_comments(ArtworkCommentsRequest {
                            artwork_id: id,
                            cursor: cursor.clone(),
                        })
                        .await
                } else {
                    client
                        .novel_comments(NovelCommentsRequest {
                            novel_id: id,
                            cursor: cursor.clone(),
                        })
                        .await
                };
                page.map(|page| {
                    (
                        serde_json::to_value(
                            page.items.iter().map(to_comment_dto).collect::<Vec<_>>(),
                        )
                        .unwrap(),
                        page.total,
                        page.access_control
                            .as_ref()
                            .map(to_comment_access_control_dto),
                        page.next,
                    )
                })
            };
            match result {
                Ok((items, total, access, next)) => {
                    actual["items"] = items;
                    actual["total"] = json!(total);
                    actual["access_control"] = json!(access);
                    actual["cursor"] = cursor_value(&next);
                    cursor = next;
                }
                Err(error) => {
                    actual["reason"] = json!(pixiv_sdk::error::reason_of(&error).unwrap().as_str());
                    actual["message"] = json!(error.to_string());
                }
            }

            assert_eq!(&actual, expected, "{} step {index}", case.name);
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
    }
}

#[derive(Clone)]
struct ResourceFixture {
    body: Value,
    raw: Option<String>,
    api_requests: Arc<Mutex<Vec<String>>>,
    resource_urls: Arc<Mutex<Vec<String>>>,
}
impl Transport for ResourceFixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert!(request.parameters.is_empty());
        self.api_requests.lock().unwrap().push(request.url);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        let response = self.send(request).await?;
        Ok(JsonResponse {
            status: response.status,
            retry_after: response.retry_after,
            body: self
                .raw
                .as_ref()
                .map(|raw| raw.as_bytes().to_vec())
                .unwrap_or_else(|| serde_json::to_vec(&response.body).unwrap()),
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
        assert!(!request.headers.contains_key("Authorization"));
        self.resource_urls.lock().unwrap().push(request.url);
        Ok(pixiv_sdk::resource::ResourceResponse::new(
            200,
            &Default::default(),
            std::io::Cursor::new(b"fixture".to_vec()),
        ))
    }
}
#[tokio::test]
async fn retained_stamp_reference_uses_cache_and_re_resolves_on_new_client() {
    let api_requests = Arc::new(Mutex::new(vec![]));
    let resource_urls = Arc::new(Mutex::new(vec![]));
    let fixture = ResourceFixture {
        raw: None,
        body: json!({"stamps":[{"stamp_id":9,"stamp_url":"https://s.pximg.net/stamp.png?token=sentinel"},{"stamp_id":9,"stamp_url":"https://s.pximg.net/duplicate.png"}]}),
        api_requests: api_requests.clone(),
        resource_urls: resource_urls.clone(),
    };
    let first = Client::with_transport("fixture-access", fixture.clone());
    let stamps = first.stamps(StampsRequest {}).await.unwrap();
    let dto = serde_json::to_string(&to_stamp_dto(&stamps[0])).unwrap();
    assert!(!dto.contains("sentinel"));
    assert!(!dto.contains("pximg.net"));
    let reference = stamps[0].image.resource.reference.clone();
    let request = || pixiv_sdk::resource::OpenResourceRequest {
        reference: reference.clone(),
        ..Default::default()
    };
    first.open_resource(request()).await.unwrap();
    assert_eq!(api_requests.lock().unwrap().len(), 1);
    let second = Client::with_transport("fixture-access", fixture);
    second.open_resource(request()).await.unwrap();
    assert_eq!(
        *api_requests.lock().unwrap(),
        vec!["https://app-api.pixiv.net/v1/stamps"; 2]
    );
    assert_eq!(
        *resource_urls.lock().unwrap(),
        vec![
            "https://s.pximg.net/duplicate.png",
            "https://s.pximg.net/stamp.png?token=sentinel"
        ]
    );
}

#[tokio::test]
async fn fresh_stamp_reference_preserves_go_ordered_raw_resolution() {
    let cases = [
        (
            "uppercase",
            r#"{"STAMPS":[{"STAMP_ID":9,"STAMP_URL":"https://s.pximg.net/stamp.png"}]}"#,
            false,
        ),
        (
            "null clearing",
            r#"{"stamps":[{"stamp_id":9,"stamp_id":null,"stamp_url":"https://s.pximg.net/stamp.png"}],"next_url":1,"NEXT_URL":null}"#,
            false,
        ),
        (
            "invalid first field",
            r#"{"stamps":[{"stamp_id":"invalid","STAMP_ID":9,"stamp_url":"https://s.pximg.net/stamp.png"}]}"#,
            true,
        ),
        (
            "unrelated invalid item",
            r#"{"stamps":[{"stamp_id":9,"stamp_url":"https://s.pximg.net/stamp.png"},{"stamp_id":10,"stamp_url":"https://example.com/bad.png"}]}"#,
            true,
        ),
    ];
    for (name, raw, malformed) in cases {
        let api_requests = Arc::new(Mutex::new(vec![]));
        let resource_urls = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            ResourceFixture {
                body: Value::Null,
                raw: Some(raw.to_owned()),
                api_requests: api_requests.clone(),
                resource_urls: resource_urls.clone(),
            },
        );
        let reference =
            pixiv_sdk::resource::ResourceRef::new("pixiv", br#"{"k":"stamp","id":9}"#).unwrap();
        let result = client
            .open_resource(pixiv_sdk::resource::OpenResourceRequest {
                reference,
                ..Default::default()
            })
            .await;
        if malformed {
            let error = result.unwrap_err();
            assert_eq!(
                pixiv_sdk::error::reason_of(&error),
                Some(pixiv_sdk::Reason::MalformedUpstreamResponse),
                "{name}"
            );
            assert!(
                error.to_string().starts_with("pixiv:OpenResource:"),
                "{name}: {error}"
            );
            assert!(resource_urls.lock().unwrap().is_empty());
        } else {
            result.unwrap();
            assert_eq!(
                *resource_urls.lock().unwrap(),
                vec!["https://s.pximg.net/stamp.png"],
                "{name}"
            );
        }
        assert_eq!(
            *api_requests.lock().unwrap(),
            vec!["https://app-api.pixiv.net/v1/stamps"],
            "{name}"
        );
    }
}
