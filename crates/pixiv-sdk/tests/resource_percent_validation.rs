use pixiv_sdk::{
    Client, Reason,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Deserialize)]
struct GoContract {
    source_commit: String,
    source_sha256: BTreeMap<String, String>,
    go_version: String,
    cases: Vec<GoCase>,
}

#[derive(Deserialize)]
struct GoCase {
    url: String,
    reason: String,
    operation: String,
    detail: String,
    message: String,
}

fn go_contract() -> GoContract {
    let contract: GoContract =
        serde_json::from_str(include_str!("fixtures/resource_percent_validation.json")).unwrap();
    assert_eq!(
        contract.source_commit,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(contract.go_version, "go1.27.1");
    assert_eq!(
        contract.source_sha256["sdk/pixiv/resource.go"],
        "e94cdf3b2d7f67e159bd2481a1c419e887d842c903107767c4004e4f6a529ed8"
    );
    assert_eq!(contract.cases.len(), 14);
    contract
}

struct ResourceUrlFixture<'a> {
    url: &'a str,
    requests: &'a AtomicUsize,
}

impl Transport for ResourceUrlFixture<'_> {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.operation, "Artwork");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/illust/detail");
        assert_eq!(request.parameters, [("illust_id".into(), "42".into())]);
        self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({"illust": {
                "id": 42,
                "create_date": "2026-01-02T03:04:05+09:00",
                "image_urls": {"original": self.url},
            }}),
        })
    }
}

#[tokio::test]
async fn artwork_resources_reject_go_invalid_percent_escapes_before_exposing_a_reference() {
    for case in go_contract()
        .cases
        .into_iter()
        .filter(|case| !case.reason.is_empty())
    {
        let url = case.url.as_str();
        let requests = AtomicUsize::new(0);
        let client = Client::with_transport(
            "fixture-access",
            ResourceUrlFixture {
                url,
                requests: &requests,
            },
        );
        let error = client.artwork(42).await.unwrap_err();
        assert_eq!(error.code, Reason::ResourceForbidden, "{url}");
        assert_eq!(error.code.as_str(), case.reason, "{url}");
        assert_eq!(error.operation, case.operation, "{url}");
        assert_eq!(error.detail.as_deref(), Some(case.detail.as_str()), "{url}");
        assert_eq!(error.to_string(), case.message, "{url}");
        assert_eq!(requests.load(Ordering::SeqCst), 1, "{url}");
    }
}

#[tokio::test]
async fn artwork_resources_keep_go_valid_escapes_and_unparsed_raw_query_escapes() {
    for case in go_contract()
        .cases
        .into_iter()
        .filter(|case| case.reason.is_empty())
    {
        let url = case.url.as_str();
        let requests = AtomicUsize::new(0);
        let client = Client::with_transport(
            "fixture-access",
            ResourceUrlFixture {
                url,
                requests: &requests,
            },
        );
        let artwork = client.artwork(42).await.unwrap();
        assert_eq!(artwork.cover.resource.url, url);
        assert!(!artwork.cover.resource.reference.is_zero(), "{url}");
        assert_eq!(requests.load(Ordering::SeqCst), 1, "{url}");
    }
}
