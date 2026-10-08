use pixiv_sdk::{
    Client, Result,
    error::Cause,
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions, SaveProgress},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

#[derive(Deserialize)]
struct Case {
    name: String,
    via_ref: bool,
    path: String,
    url: String,
    status: i64,
    length: String,
    body: String,
    failure: String,
    setup: String,
    error: Option<Value>,
    cause: String,
    size: i64,
    content_type: String,
    path_matches: bool,
    progress: Vec<Value>,
    calls: usize,
    closes: usize,
    #[serde(rename = "final")]
    final_: String,
    exists: bool,
    temporary_files: usize,
}
struct Body {
    bytes: Vec<u8>,
    failure: String,
    seen: Arc<Mutex<(usize, usize)>>,
}
impl AsyncRead for Body {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.bytes.is_empty() {
            let count = 3.min(self.bytes.len()).min(output.remaining());
            output.put_slice(&self.bytes[..count]);
            self.bytes.drain(..count);
            return Poll::Ready(Ok(()));
        }
        let result = match self.failure.as_str() {
            "read" => Err(io::Error::other("fixture private URL/path")),
            "cancel" => Err(io::Error::other(Cause::Canceled)),
            "deadline" => Err(io::Error::other(Cause::DeadlineExceeded)),
            "pending" => return Poll::Pending,
            _ => Ok(()),
        };
        Poll::Ready(result)
    }
}
impl Drop for Body {
    fn drop(&mut self) {
        self.seen.lock().unwrap().1 += 1;
    }
}
struct Fixture {
    url: String,
    status: i64,
    length: String,
    bytes: Vec<u8>,
    failure: String,
    seen: Arc<Mutex<(usize, usize)>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> Result<Response> {
        assert_eq!(request.method, reqwest::Method::GET);
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/ugoira/metadata");
        assert_eq!(request.parameters, vec![("illust_id".into(), "42".into())]);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({"ugoira_metadata":{"zip_urls":{"original":self.url},"frames":[{"file":"0.jpg"}]}}),
        })
    }
}
impl ResourceTransport for Fixture {
    type Body = Body;
    async fn open_resource(&self, request: ResourceReadRequest) -> Result<ResourceResponse<Body>> {
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.headers.get("Referer").unwrap(),
            &vec!["https://app-api.pixiv.net/"]
        );
        assert!(!request.headers.contains_key("Authorization"));
        assert!(!request.headers.contains_key("Cookie"));
        request.validate.as_ref().unwrap()(&request.url)?;
        self.seen.lock().unwrap().0 += 1;
        Ok(ResourceResponse::new(
            self.status,
            &ResourceHeaders::from([
                ("Content-Type".into(), vec!["image/png".into()]),
                ("Content-Length".into(), vec![self.length.clone()]),
            ]),
            Body {
                bytes: self.bytes.clone(),
                failure: self.failure.clone(),
                seen: self.seen.clone(),
            },
        ))
    }
}
fn root() -> std::path::PathBuf {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).unwrap();
    let path = std::env::temp_dir().join(format!(
        "pixiv-save-contract-{:x}",
        u128::from_ne_bytes(random)
    ));
    std::fs::create_dir(&path).unwrap();
    path
}
struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn temporary_files(path: &std::path::Path) -> usize {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                temporary_files(&entry.path())
            } else {
                usize::from(
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".atomic-write-"),
                )
            }
        })
        .sum()
}
#[tokio::test]
async fn saves_match_go_files_progress_error_priorities_and_body_ownership() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/resource-save.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 40);
    for case in cases {
        let dir = Directory(root());
        let path = if case.path.trim().is_empty() {
            case.path.clone()
        } else {
            dir.0.join(&case.path).to_string_lossy().into_owned()
        };
        match case.setup.as_str() {
            "file" => std::fs::write(&path, "previous").unwrap(),
            "directory" => std::fs::create_dir(&path).unwrap(),
            "parent-file" => {
                std::fs::write(std::path::Path::new(&path).parent().unwrap(), "previous").unwrap()
            }
            _ => (),
        }
        let seen = Arc::new(Mutex::new((0, 0)));
        let events = Arc::new(Mutex::new(Vec::<Value>::new()));
        let output = events.clone();
        let progress_path = path.clone();
        let existing = case.setup == "file";
        let options = SaveOptions {
            path: path.clone(),
            progress: Some(Arc::new(move |progress: SaveProgress| {
                output
                    .lock()
                    .unwrap()
                    .push(json!({"Total":progress.total,"Done":progress.done}));
                if existing {
                    assert_eq!(std::fs::read_to_string(&progress_path).unwrap(), "previous");
                }
            })),
        };
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                url: case.url.clone(),
                status: case.status,
                length: case.length,
                bytes: case.body.into_bytes(),
                failure: case.failure,
                seen: seen.clone(),
            },
        );
        let result = if case.via_ref {
            client
                .save_resource(
                    ResourceRef::new(
                        "pixiv",
                        br#"{"k":"ugoira_archive","id":42,"p":-1,"v":"original"}"#,
                    )
                    .unwrap(),
                    options,
                )
                .await
        } else {
            client.save_resource_url(&case.url, options).await
        };
        match result {
            Ok(saved) => {
                assert!(case.error.is_none(), "{}", case.name);
                assert_eq!(saved.size, case.size);
                assert_eq!(saved.content_type, case.content_type);
                assert_eq!(saved.path == path, case.path_matches);
            }
            Err(error) => {
                let actual = json!({"Reason":error.code.as_str(),"Product":error.product,"Operation":error.operation,"Detail":error.detail.clone().unwrap_or_default(),"HTTPStatus":error.http_status.unwrap_or_default(),"Transport":error.transport.map(|kind|serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()).unwrap_or_default(),"Retry":{"Safe":error.retry.safe,"After":error.retry.after.map(|date|date.to_rfc3339()).unwrap_or_else(||"0001-01-01T00:00:00Z".into()),"HasAfter":error.retry.after.is_some()}});
                assert_eq!(Some(actual), case.error, "{}", case.name);
                use std::error::Error;
                let cause = error
                    .source()
                    .and_then(|cause| cause.downcast_ref::<Cause>());
                let actual = match cause {
                    Some(Cause::Canceled) => "cancel",
                    Some(Cause::DeadlineExceeded) => "deadline",
                    _ => "",
                };
                assert_eq!(actual, case.cause, "{}", case.name);
                assert!(!error.to_string().contains("fixture private"));
                assert!(!format!("{error:?}").contains("fixture=secret"));
            }
        }
        assert_eq!(*events.lock().unwrap(), case.progress, "{}", case.name);
        assert_eq!(
            *seen.lock().unwrap(),
            (case.calls, case.closes),
            "{}",
            case.name
        );
        let destination = std::path::Path::new(&path);
        assert_eq!(destination.exists(), case.exists, "{}", case.name);
        let final_value = if destination.is_dir() {
            "<directory>".into()
        } else {
            std::fs::read_to_string(destination).unwrap_or_default()
        };
        assert_eq!(final_value, case.final_, "{}", case.name);
        assert_eq!(
            temporary_files(&dir.0),
            case.temporary_files,
            "{}",
            case.name
        );
    }
}
#[tokio::test]
async fn dropping_a_pending_save_removes_the_partial_file_and_releases_the_body() {
    let dir = Directory(root());
    let path = dir.0.join("asset.bin");
    std::fs::write(&path, "previous").unwrap();
    let seen = Arc::new(Mutex::new((0, 0)));
    let client = Client::with_transport(
        "fixture-access",
        Fixture {
            url: "https://i.pximg.net/asset.bin".into(),
            status: 200,
            length: "7".into(),
            bytes: b"payload".to_vec(),
            failure: "pending".into(),
            seen: seen.clone(),
        },
    );
    let mut save = Box::pin(client.save_resource_url(
        "https://i.pximg.net/asset.bin",
        SaveOptions {
            path: path.to_string_lossy().into_owned(),
            ..Default::default()
        },
    ));
    assert!(futures_util::poll!(&mut save).is_pending());
    assert_eq!(temporary_files(&dir.0), 1);
    drop(save);
    assert_eq!(temporary_files(&dir.0), 0);
    assert_eq!(*seen.lock().unwrap(), (1, 1));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "previous");
}
