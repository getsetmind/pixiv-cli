use pixiv_sdk::{
    Client, Result,
    dto::ArtworkPageDto,
    pixiv::{ResourcePolicy, artwork_variant_resource},
    resource::{Resource, ResourceRef},
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
#[derive(Deserialize)]
struct PageCase {
    name: String,
    id: i64,
    body: Value,
    allowed_hosts: Option<Vec<String>>,
    dto: Value,
    reason: String,
    message: String,
    requests: usize,
}
#[derive(Deserialize)]
struct VariantCase {
    input: String,
    variant: String,
    r#ref: String,
    reason: String,
}
#[derive(Deserialize)]
struct Contract {
    pages: Vec<PageCase>,
    variants: Vec<VariantCase>,
}
struct Fixture<'a> {
    case: &'a PageCase,
    calls: &'a AtomicUsize,
}
impl Transport for Fixture<'_> {
    async fn send(&self, request: Request) -> Result<Response> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.operation, "ArtworkPages");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/illust/detail");
        assert_eq!(request.method, reqwest::Method::GET);
        assert_eq!(
            request.parameters,
            vec![("illust_id".to_owned(), self.case.id.to_string())]
        );
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.case.body.clone(),
        })
    }
}
fn contract() -> Contract {
    serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/artwork-pages.json"
    ))
    .unwrap()
}
#[tokio::test]
async fn artwork_pages_match_go_without_requiring_cover_or_publication_time() {
    let contract = contract();
    assert_eq!(contract.pages.len(), 27);
    for case in contract.pages {
        let calls = AtomicUsize::new(0);
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                case: &case,
                calls: &calls,
            },
        )
        .with_resource_policy(ResourcePolicy {
            allowed_hosts: case.allowed_hosts.clone().unwrap_or_default(),
        });
        let result = client.artwork_pages(case.id).await;
        if case.reason.is_empty() {
            let pages = result.unwrap_or_else(|error| panic!("{}: {error}", case.name));
            let dtos: Vec<_> = pages.iter().map(ArtworkPageDto::from).collect();
            assert_eq!(
                serde_json::to_value(dtos).unwrap(),
                case.dto,
                "{}",
                case.name
            );
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code.as_str(), case.reason, "{}", case.name);
            assert_eq!(error.to_string(), case.message, "{}", case.name);
        }
        assert_eq!(calls.load(Ordering::SeqCst), case.requests, "{}", case.name);
    }
}
#[test]
fn artwork_quality_variants_preserve_go_identity_and_zero_reference_rules() {
    let cases = contract().variants;
    assert_eq!(cases.len(), 27);
    for case in cases {
        let original = Resource {
            reference: if case.input.is_empty() {
                ResourceRef::default()
            } else {
                ResourceRef::parse(&case.input).unwrap()
            },
            ..Resource::default()
        };
        let result = artwork_variant_resource(&original, &case.variant);
        if case.reason.is_empty() {
            assert_eq!(result.unwrap().as_str(), case.r#ref);
        } else {
            assert_eq!(result.unwrap_err().code.as_str(), case.reason);
        }
    }
}
