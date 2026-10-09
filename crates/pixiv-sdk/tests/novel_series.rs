use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    dto::NovelSeriesResultDto,
    pixiv::NovelSeriesRequest,
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
    series_id: i64,
    cursor: String,
    next_series_id: i64,
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
        assert_eq!(request.url, "https://app-api.pixiv.net/v2/novel/series");
        assert_eq!(request.operation, "NovelSeries");
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let index = usize::from(self.bodies.len() > 1 && query.contains_key("last_order"));
        self.queries.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}

#[tokio::test]
async fn series_matches_go_metadata_dtos_pages_and_global_cursor_binding() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/novel-series.json")).unwrap();
    let cases: Vec<Case> = serde_json::from_value(fixture["series"].clone()).unwrap();
    assert_eq!(cases.len(), 49);
    for case in cases {
        let queries = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                bodies: case.bodies,
                queries: queries.clone(),
            },
        );
        let mut request = NovelSeriesRequest {
            series_id: case.series_id,
            cursor: if case.cursor.is_empty() {
                Cursor::default()
            } else {
                Cursor::parse(&case.cursor).unwrap()
            },
        };
        for (index, expected) in case.results.iter().enumerate() {
            let mut actual = json!({"dto":null,"cursor":null,"reason":"","message":""});
            match client.novel_series(request.clone()).await {
                Ok(page) => {
                    let mut dto = serde_json::to_value(NovelSeriesResultDto::from(&page)).unwrap();
                    dto["novels"]["next"] = json!("");
                    actual["dto"] = dto;
                    if !page.novels.next.is_zero() {
                        actual["cursor"] = serde_json::from_slice(
                            &URL_SAFE_NO_PAD.decode(page.novels.next.as_str()).unwrap(),
                        )
                        .unwrap();
                    }
                    request.cursor = page.novels.next;
                }
                Err(error) => {
                    actual["reason"] = json!(pixiv_sdk::error::reason_of(&error).unwrap().as_str());
                    actual["message"] = json!(error.to_string());
                }
            }
            assert_eq!(&actual, expected, "{} step {index}", case.name);
            if case.next_series_id != 0 {
                request.series_id = case.next_series_id;
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

#[tokio::test]
async fn content_matches_go_validation_and_unavailable_without_http() {
    use pixiv_sdk::pixiv::NovelContentRequest;
    let fixture: Value = serde_json::from_str(include_str!("fixtures/novel-series.json")).unwrap();
    let queries = Arc::new(Mutex::new(vec![]));
    let client = Client::with_transport(
        "",
        Fixture {
            bodies: vec![],
            queries: queries.clone(),
        },
    );
    for case in fixture["content"].as_array().unwrap() {
        let error = client
            .novel_content(NovelContentRequest {
                novel_id: case["novel_id"].as_i64().unwrap(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), case["message"].as_str().unwrap());
        assert_eq!(
            pixiv_sdk::error::reason_of(&error).unwrap().as_str(),
            case["reason"].as_str().unwrap()
        );
    }
    assert!(queries.lock().unwrap().is_empty());
}

#[test]
fn content_dtos_match_go_empty_arrays_optional_variants_and_future_kinds() {
    use pixiv_sdk::{dto::NovelContentDto, models::*};
    let fixture: Value = serde_json::from_str(include_str!("fixtures/novel-series.json")).unwrap();
    let body = NovelContent {
        novel_id: 42,
        title: "title".into(),
        caption: "caption".into(),
        blocks: vec![
            NovelBlock {
                kind: NOVEL_BLOCK_PARAGRAPH.into(),
                text: "text".into(),
                marks: vec![
                    NovelMark {
                        kind: NOVEL_MARK_STRONG.into(),
                        text: "strong".into(),
                        ..Default::default()
                    },
                    NovelMark {
                        kind: NOVEL_MARK_RUBY.into(),
                        ruby: Some(NovelRuby {
                            text: "漢字".into(),
                            furigana: "かんじ".into(),
                        }),
                        ..Default::default()
                    },
                    NovelMark {
                        kind: NOVEL_MARK_LINK.into(),
                        href: "https://example.com".into(),
                        ..Default::default()
                    },
                    NovelMark {
                        kind: NOVEL_MARK_CUSTOM.into(),
                        class: "custom".into(),
                        ..Default::default()
                    },
                    NovelMark {
                        kind: "future".into(),
                        text: "future".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            NovelBlock {
                kind: NOVEL_BLOCK_HEADER.into(),
                text: "header".into(),
                ..Default::default()
            },
            NovelBlock {
                kind: NOVEL_BLOCK_IMAGE.into(),
                image: Some(NovelImageBlock {
                    caption: "image".into(),
                    width: 12,
                    height: 34,
                    ..Default::default()
                }),
                ..Default::default()
            },
            NovelBlock {
                kind: NOVEL_BLOCK_FILE.into(),
                file: Some(NovelFileBlock {
                    filename: "file".into(),
                    caption: "caption".into(),
                    size: 99,
                    ..Default::default()
                }),
                ..Default::default()
            },
            NovelBlock {
                kind: NOVEL_BLOCK_UNKNOWN.into(),
                unknown: Some(NovelUnknownBlock {
                    raw_type: "widget".into(),
                    payload: BTreeMap::from([("safe".into(), "value".into())]),
                }),
                ..Default::default()
            },
            NovelBlock {
                kind: "future".into(),
                unknown: Some(NovelUnknownBlock {
                    raw_type: "future".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ],
    };
    assert_eq!(
        serde_json::to_value(NovelContentDto::from(&NovelContent::default())).unwrap(),
        fixture["content_dto"][0]
    );
    assert_eq!(
        serde_json::to_value(NovelContentDto::from(&body)).unwrap(),
        fixture["content_dto"][1]
    );
    let mut dto = NovelContentDto::from(&body);
    dto.blocks[4]
        .unknown
        .as_mut()
        .unwrap()
        .payload
        .insert("safe".into(), "changed".into());
    assert_eq!(
        body.blocks[4].unknown.as_ref().unwrap().payload["safe"],
        "value"
    );
}

#[test]
fn content_resource_dtos_match_go_opaque_references_and_all_optional_variants() {
    use pixiv_sdk::{
        dto::NovelContentDto,
        models::*,
        resource::{Resource, ResourceRef},
    };
    let fixture: Value = serde_json::from_str(include_str!("fixtures/novel-series.json")).unwrap();
    let reference =
        ResourceRef::new("pixiv", br#"{"kind":"novel_image","id":42,"index":0}"#).unwrap();
    let image_resource = Resource {
        reference: reference.clone(),
        url: "https://i.pximg.net/runtime-image.jpg".into(),
        request_headers: BTreeMap::from([("Referer".into(), "https://www.pixiv.net/".into())]),
        expires_at: Some(
            chrono::DateTime::parse_from_rfc3339("2026-10-09T01:02:03Z")
                .unwrap()
                .to_utc(),
        ),
        requires_credentials: true,
    };
    let mut file_resource = image_resource.clone();
    file_resource.url = "https://i.pximg.net/runtime-file.bin".into();
    file_resource.requires_credentials = false;
    let body = NovelContent {
        novel_id: 42,
        blocks: vec![
            NovelBlock {
                kind: NOVEL_BLOCK_IMAGE.into(),
                text: "variants".into(),
                marks: vec![
                    NovelMark {
                        kind: NOVEL_MARK_EMPHASIS.into(),
                        text: "emphasis".into(),
                        ..Default::default()
                    },
                    NovelMark {
                        kind: NOVEL_MARK_DELETE.into(),
                        text: "delete".into(),
                        ..Default::default()
                    },
                    NovelMark {
                        kind: NOVEL_MARK_UNKNOWN.into(),
                        text: "unknown".into(),
                        ruby: Some(NovelRuby {
                            text: "base".into(),
                            furigana: "reading".into(),
                        }),
                        href: "https://example.com/optional".into(),
                        class: "optional".into(),
                    },
                ],
                image: Some(NovelImageBlock {
                    resource: image_resource,
                    caption: "resource image".into(),
                    width: 56,
                    height: 78,
                }),
                file: Some(NovelFileBlock {
                    resource: file_resource.clone(),
                    filename: "file.bin".into(),
                    caption: "resource file".into(),
                    size: 123,
                }),
                unknown: Some(NovelUnknownBlock {
                    raw_type: "mixed".into(),
                    payload: BTreeMap::from([("safe".into(), "copied".into())]),
                }),
            },
            NovelBlock {
                kind: NOVEL_BLOCK_FILE.into(),
                file: Some(NovelFileBlock {
                    resource: file_resource,
                    filename: "file.bin".into(),
                    size: 123,
                    ..Default::default()
                }),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut dto = NovelContentDto::from(&body);
    let actual = serde_json::to_value(&dto).unwrap();
    assert_eq!(actual, fixture["content_dto"][2]);
    assert_eq!(
        actual["blocks"][0]["image"]["resource"],
        json!({"ref":reference.as_str(),"requires_credentials":true})
    );
    assert_eq!(
        actual["blocks"][0]["file"]["resource"],
        json!({"ref":reference.as_str()})
    );
    let encoded = serde_json::to_string(&actual).unwrap();
    for private in [
        "runtime-image",
        "runtime-file",
        "Referer",
        "expires_at",
        "request_headers",
        "url",
    ] {
        assert!(!encoded.contains(private), "content DTO leaked {private}");
    }
    dto.blocks[0]
        .unknown
        .as_mut()
        .unwrap()
        .payload
        .insert("safe".into(), "changed".into());
    assert_eq!(
        body.blocks[0].unknown.as_ref().unwrap().payload["safe"],
        "copied"
    );
}
