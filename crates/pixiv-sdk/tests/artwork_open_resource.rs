use pixiv_sdk::{
    Client, Result,
    pixiv::ResourcePolicy,
    resource::{OpenResourceRequest, ResourceHeaders, ResourceRef, ResourceResponse},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    io::Cursor,
    sync::{Arc, Mutex},
};

#[derive(Deserialize)]
struct Case {
    name: String,
    payload: String,
    product: String,
    zero: bool,
    method: String,
    preload: bool,
    body: Value,
    error: Option<Value>,
    api_requests: Vec<String>,
    urls: Vec<String>,
    headers: Vec<ResourceHeaders>,
}
#[derive(Default)]
struct Seen {
    api: Vec<String>,
    urls: Vec<String>,
    headers: Vec<ResourceHeaders>,
}
struct Fixture {
    body: Value,
    seen: Arc<Mutex<Seen>>,
}
impl Transport for Fixture {
    async fn send(&self, input: Request) -> Result<Response> {
        assert_eq!(input.method, reqwest::Method::GET);
        let mut url = url::Url::parse(&input.url).unwrap();
        assert_eq!(url.host_str(), Some("app-api.pixiv.net"));
        if !input.parameters.is_empty() {
            url.query_pairs_mut().extend_pairs(input.parameters);
        }
        let uri = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().into(),
        };
        self.seen.lock().unwrap().api.push(uri);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}
impl ResourceTransport for Fixture {
    type Body = Cursor<Vec<u8>>;
    async fn open_resource(
        &self,
        input: ResourceReadRequest,
    ) -> Result<ResourceResponse<Self::Body>> {
        input.validate.as_ref().unwrap()(&input.url)?;
        assert!(matches!(input.method.as_str(), "GET" | "HEAD"));
        let mut headers = input.headers;
        headers.insert(
            "User-Agent".into(),
            vec!["PixivAndroidApp/5.0.234 (Android 11; Pixel 5)".into()],
        );
        let mut seen = self.seen.lock().unwrap();
        seen.urls.push(input.url);
        seen.headers.push(headers);
        Ok(ResourceResponse::new(
            206,
            &ResourceHeaders::new(),
            Cursor::new(b"abc".to_vec()),
        ))
    }
}

#[tokio::test]
async fn resource_open_revalidates_identity_resolves_metadata_and_reuses_cached_urls() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/artwork-open-resource.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 124);
    for case in cases {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                body: case.body,
                seen: seen.clone(),
            },
        )
        .with_resource_policy(ResourcePolicy::default());
        let reference = if case.zero {
            ResourceRef::default()
        } else {
            ResourceRef::new(&case.product, case.payload.as_bytes()).unwrap()
        };
        if case.preload {
            client.artwork_pages(42).await.unwrap();
        }
        let mut error = None;
        for _ in 0..2 {
            let result = client
                .open_resource(OpenResourceRequest {
                    reference: reference.clone(),
                    method: case.method.clone(),
                    range: "bytes=0-2".into(),
                    if_none_match: "\"fixture\"".into(),
                    if_modified_since: "Thu, 08 Oct 2026 00:00:00 GMT".into(),
                    if_range: "\"range\"".into(),
                })
                .await;
            match result {
                Ok(response) => assert_eq!(response.status_code, 206),
                Err(err) => {
                    error = Some(serde_json::json!({
                        "Reason": err.code.as_str(), "Product": err.product,
                        "Operation": err.operation, "Detail": err.detail.unwrap_or_default(),
                        "HTTPStatus": err.http_status.unwrap_or_default(),
                        "Transport": err.transport.map(|kind| serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()).unwrap_or_default(),
                        "Retry": {"Safe": err.retry.safe, "HasAfter": err.retry.after.is_some(),
                            "After": err.retry.after.map(|date| date.to_rfc3339()).unwrap_or_else(|| "0001-01-01T00:00:00Z".into())},
                    }));
                    break;
                }
            }
        }
        assert_eq!(error, case.error, "{} {}", case.name, case.payload);
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.api, case.api_requests,
            "{} {}",
            case.name, case.payload
        );
        assert_eq!(seen.urls, case.urls, "{} {}", case.name, case.payload);
        assert_eq!(seen.headers, case.headers, "{} {}", case.name, case.payload);
    }
}
