use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    dto::ArtworkDto,
    pixiv::ArtworkSeriesRequest,
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
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/illust/series");
        assert_eq!(request.operation, "ArtworkSeries");
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let index = usize::from(
            self.bodies.len() > 1
                && (query.contains_key("last_order") || query.contains_key("offset")),
        );
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
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/artwork-series.json")).unwrap();
    let cases: Vec<Case> = serde_json::from_value(fixture).unwrap();
    assert_eq!(cases.len(), 83);
    for case in cases {
        let queries = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                bodies: case.bodies,
                queries: queries.clone(),
            },
        );
        let mut request = ArtworkSeriesRequest {
            series_id: case.series_id,
            cursor: if case.cursor.is_empty() {
                Cursor::default()
            } else {
                Cursor::parse(&case.cursor).unwrap()
            },
        };
        for (index, expected) in case.results.iter().enumerate() {
            let mut actual = json!({"dto":null,"cursor":null,"reason":"","message":""});
            match client.artwork_series(request.clone()).await {
                Ok(page) => {
                    actual["dto"] = serde_json::to_value(
                        page.items.iter().map(ArtworkDto::from).collect::<Vec<_>>(),
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

struct ResourceFixture {
    api: Fixture,
    urls: Arc<Mutex<Vec<String>>>,
}
impl Transport for ResourceFixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        self.api.send(request).await
    }
}
impl pixiv_sdk::transport::ResourceTransport for ResourceFixture {
    type Body = std::io::Cursor<Vec<u8>>;
    async fn open_resource(
        &self,
        request: pixiv_sdk::transport::ResourceReadRequest,
    ) -> pixiv_sdk::Result<pixiv_sdk::resource::ResourceResponse<Self::Body>> {
        request.validate.as_ref().unwrap()(&request.url)?;
        assert_eq!(request.method, "GET");
        assert_eq!(request.headers["Referer"], ["https://app-api.pixiv.net/"]);
        self.urls.lock().unwrap().push(request.url);
        Ok(pixiv_sdk::resource::ResourceResponse::new(
            200,
            &BTreeMap::new(),
            std::io::Cursor::new(vec![]),
        ))
    }
}

#[tokio::test]
async fn series_remembers_cover_and_profile_without_expanding_list_pages() {
    use pixiv_sdk::resource::OpenResourceRequest;
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/artwork-series.json")).unwrap();
    let case = cases.into_iter().find(|case| case.name == "id:21").unwrap();
    let queries = Arc::new(Mutex::new(vec![]));
    let urls = Arc::new(Mutex::new(vec![]));
    let client = Client::with_transport(
        "fixture-access",
        ResourceFixture {
            api: Fixture {
                bodies: case.bodies,
                queries: queries.clone(),
            },
            urls: urls.clone(),
        },
    );
    let page = client
        .artwork_series(ArtworkSeriesRequest {
            series_id: 21,
            ..Default::default()
        })
        .await
        .unwrap();
    let item = &page.items[0];
    assert!(item.pages.is_empty());
    let resources = [&item.cover.resource, &item.user.profile_image.resource];
    for resource in resources {
        client
            .open_resource(OpenResourceRequest {
                reference: resource.reference.clone(),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    assert_eq!(queries.lock().unwrap().len(), 1);
    assert_eq!(
        *urls.lock().unwrap(),
        [
            "https://i.pximg.net/cover.jpg",
            "https://i.pximg.net/profile.jpg"
        ]
    );
}

struct TagsFixture(Value);
impl Transport for TagsFixture {
    async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.0.clone(),
        })
    }
}

#[tokio::test]
async fn shared_artwork_tags_preserve_null_elements_and_reject_invalid_fields_like_go() {
    use pixiv_sdk::pixiv::{ArtworkRankingRequest, SearchArtworksRequest};
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/artwork-null-tags.json")).unwrap();
    assert_eq!(cases.len(), 15);
    for case in cases {
        let client = Client::with_transport("fixture-access", TagsFixture(case["body"].clone()));
        let operation = case["operation"].as_str().unwrap();
        let result = match operation {
            "Artwork" => client.artwork(42).await.map(|item| vec![item]),
            "ArtworkRanking" => client
                .artwork_ranking(ArtworkRankingRequest::default())
                .await
                .map(|page| page.items),
            "SearchArtworks" => client
                .search_artworks(SearchArtworksRequest {
                    word: "tag".into(),
                    ..Default::default()
                })
                .await
                .map(|page| page.items),
            _ => panic!("unexpected operation"),
        };
        let mut actual = json!({"dto":[],"reason":"","message":""});
        match result {
            Ok(items) => {
                actual["dto"] =
                    serde_json::to_value(items.iter().map(ArtworkDto::from).collect::<Vec<_>>())
                        .unwrap()
            }
            Err(error) => {
                actual["reason"] = json!(pixiv_sdk::error::reason_of(&error).unwrap().as_str());
                actual["message"] = json!(error.to_string());
            }
        }
        assert_eq!(
            actual["dto"], case["dto"],
            "{operation} tags {}",
            case["body"]
        );
        assert_eq!(
            actual["reason"], case["reason"],
            "{operation} tags {}",
            case["body"]
        );
        assert_eq!(
            actual["message"], case["message"],
            "{operation} tags {}",
            case["body"]
        );
    }
}
