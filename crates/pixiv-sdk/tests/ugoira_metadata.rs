use pixiv_sdk::{
    Client, Result,
    dto::UgoiraMetadataDto,
    resource::{OpenResourceRequest, ResourceHeaders, ResourceResponse},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::Cursor,
    sync::{Arc, Mutex},
};

#[derive(Deserialize)]
struct Case {
    name: String,
    id: i64,
    body: Value,
    dto: Option<Value>,
    error: Option<Value>,
    requests: Vec<String>,
    followup: bool,
    resource_requests: Vec<String>,
}
#[derive(Default)]
struct Seen {
    api: Vec<String>,
    resources: Vec<String>,
}
struct Fixture {
    body: Value,
    seen: Arc<Mutex<Seen>>,
}
impl Transport for Fixture {
    async fn send(&self, input: Request) -> Result<Response> {
        assert_eq!(input.method, reqwest::Method::GET);
        assert_eq!(input.url, "https://app-api.pixiv.net/v1/ugoira/metadata");
        assert_eq!(input.parameters, vec![("illust_id".into(), "42".into())]);
        self.seen
            .lock()
            .unwrap()
            .api
            .push("/v1/ugoira/metadata?illust_id=42".into());
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
        assert!(!input.headers.contains_key("Authorization"));
        self.seen.lock().unwrap().resources.push(input.url);
        Ok(ResourceResponse::new(
            200,
            &ResourceHeaders::new(),
            Cursor::new(vec![]),
        ))
    }
}

#[tokio::test]
async fn public_ugoira_metadata_preserves_archives_signed_delays_and_error_priorities() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/ugoira-metadata.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 45);
    for case in cases {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                body: case.body,
                seen: seen.clone(),
            },
        );
        let result = client.ugoira_metadata(case.id).await;
        match result {
            Ok(metadata) => {
                assert!(case.error.is_none(), "{}", case.name);
                let dto = serde_json::to_value(UgoiraMetadataDto::from(&metadata)).unwrap();
                assert_eq!(Some(dto.clone()), case.dto, "{}", case.name);
                let output = dto.to_string();
                assert!(!output.contains("https://"));
                assert!(!output.contains("fixture=secret"));
                assert!(!format!("{metadata:?}").contains("https://"));
            }
            Err(error) => {
                assert!(case.dto.is_none(), "{}", case.name);
                let actual = json!({"Reason":error.code.as_str(),"Product":error.product,
                    "Operation":error.operation,"Detail":error.detail.unwrap_or_default(),
                    "HTTPStatus":error.http_status.unwrap_or_default(),"Transport":error.transport.map(|kind| serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()).unwrap_or_default(),
                    "Retry":{"Safe":error.retry.safe,"HasAfter":error.retry.after.is_some(),
                        "After":error.retry.after.map(|date|date.to_rfc3339()).unwrap_or_else(||"0001-01-01T00:00:00Z".into())}});
                assert_eq!(Some(actual), case.error, "{}", case.name);
            }
        }
        if case.followup {
            let reference = pixiv_sdk::resource::ResourceRef::new(
                "pixiv",
                br#"{"k":"ugoira_archive","id":42,"p":-1,"v":"original"}"#,
            )
            .unwrap();
            client
                .open_resource(OpenResourceRequest {
                    reference,
                    ..Default::default()
                })
                .await
                .unwrap();
        }
        assert_eq!(seen.lock().unwrap().api, case.requests, "{}", case.name);
        assert_eq!(
            seen.lock().unwrap().resources,
            case.resource_requests,
            "{}",
            case.name
        );
    }
}

#[tokio::test]
async fn returned_archive_references_reuse_metadata_urls_without_another_api_read() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let client = Client::with_transport(
        "fixture-access",
        Fixture {
            seen: seen.clone(),
            body: json!({"ugoira_metadata":{
        "zip_urls":{"medium":"https://i.pximg.net/medium.zip","original":"https://i.pximg.net/original.zip"},
        "frames":[{"file":"0.jpg","delay":-1}]}}),
        },
    );
    let metadata = client.ugoira_metadata(42).await.unwrap();
    for archive in &metadata.archives {
        client
            .open_resource(OpenResourceRequest {
                reference: archive.resource.reference.clone(),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let seen = seen.lock().unwrap();
    assert_eq!(seen.api.len(), 1);
    assert_eq!(
        seen.resources,
        vec![
            "https://i.pximg.net/medium.zip",
            "https://i.pximg.net/original.zip"
        ]
    );
}

#[test]
fn explicit_metadata_dto_keeps_empty_arrays_unknown_qualities_and_signed_frame_values() {
    use pixiv_sdk::{
        models::{UgoiraArchive, UgoiraFrame, UgoiraMetadata},
        resource::Resource,
    };
    let mut metadata = UgoiraMetadata {
        artwork_id: 0,
        archives: vec![],
        frames: vec![],
    };
    assert_eq!(
        serde_json::to_value(UgoiraMetadataDto::from(&metadata)).unwrap(),
        json!({"artwork_id":0,"archives":[],"frames":[]})
    );
    metadata.archives.push(UgoiraArchive {
        quality: "future_quality".into(),
        resource: Resource {
            url: "fixture-locator-secret".into(),
            request_headers: [("Cookie".into(), "fixture-cookie-secret".into())].into(),
            ..Default::default()
        },
    });
    metadata.frames.push(UgoiraFrame {
        filename: "0.jpg".into(),
        delay_milliseconds: i64::MIN,
    });
    assert_eq!(
        serde_json::to_value(UgoiraMetadataDto::from(&metadata)).unwrap(),
        json!({
        "artwork_id":0,"archives":[{"quality":"future_quality","resource":null}],
        "frames":[{"filename":"0.jpg","delay_milliseconds":i64::MIN}]})
    );
}
