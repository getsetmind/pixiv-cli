use pixiv_app::{
    download::{ArtworkFuture, DownloadSaveClient, SaveFuture, UgoiraMetadataFuture},
    lifecycle::Context,
};
use pixiv_sdk::{
    Client, Error, Reason,
    error::{Cause, RetryAdvice},
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use serde_json::{Value, json};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context as PollContext, Poll},
};

#[derive(Default)]
pub struct Observed {
    pub requests: Mutex<Vec<Value>>,
    pub opens: Mutex<Vec<i64>>,
    pub closes: AtomicUsize,
}

#[derive(Clone)]
pub struct PendingArchive {
    pub ready: Arc<tokio::sync::Notify>,
    pub root: PathBuf,
}

pub struct FixtureTransport {
    pub observed: Arc<Observed>,
    pub fixture: Arc<Value>,
    pub pending: Option<PendingArchive>,
    pub rate_metadata: bool,
    pub rate_archive: bool,
}
impl Drop for FixtureTransport {
    fn drop(&mut self) {
        self.observed.closes.fetch_add(1, Ordering::SeqCst);
    }
}
fn header(headers: &[(String, String)], key: &str) -> String {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}
fn rate_error(operation: &str) -> Error {
    Error::new(Reason::RateLimited, operation).with_retry(RetryAdvice {
        safe: true,
        after: Some(chrono::Utc::now() + chrono::TimeDelta::seconds(120)),
    })
}
impl Transport for FixtureTransport {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let url = if request.method.as_str() == "GET" && !request.parameters.is_empty() {
            format!(
                "{}?{}",
                request.url,
                pixiv_sdk::transport::encode_query_pairs(&request.parameters)
            )
        } else {
            request.url.clone()
        };
        self.observed.requests.lock().unwrap().push(json!({
            "url":url,"method":request.method.as_str(),
            "referer":header(&request.headers,"Referer"),
            "authorization":header(&request.headers,"Authorization"),
            "cookie":header(&request.headers,"Cookie")
        }));
        if request.operation == "Open" {
            let token = &request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1;
            let id = token.rsplit('-').next().unwrap().parse::<i64>().unwrap();
            self.observed.opens.lock().unwrap().push(id);
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"access_token":format!("fixture-access-{id}"),
                    "refresh_token":format!("fixture-rotated-{id}"),
                    "expires_in":3600,"user":{"id":id}}),
            });
        }
        let id = request
            .parameters
            .iter()
            .find(|(key, _)| key == "illust_id")
            .unwrap()
            .1
            .parse::<i64>()
            .unwrap();
        let body = if request.url.ends_with("/v1/illust/detail") {
            artwork(id)
        } else {
            assert!(request.url.ends_with("/v1/ugoira/metadata"));
            if self.rate_metadata {
                return Err(rate_error("UgoiraMetadata"));
            }
            if id == 83 {
                return Ok(Response {
                    status: 404,
                    retry_after: None,
                    body: json!({"error":{"message":"owned fixture metadata absent"}}),
                });
            }
            self.fixture["ugoira_metadata"][id.to_string()].clone()
        };
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}
impl ResourceTransport for FixtureTransport {
    type Body = Pin<Box<dyn tokio::io::AsyncRead + Send>>;
    async fn open_resource(
        &self,
        request: ResourceReadRequest,
    ) -> pixiv_sdk::Result<ResourceResponse<Self::Body>> {
        request.validate.as_ref().unwrap()(&request.url)?;
        self.observed.requests.lock().unwrap().push(json!({
            "url":request.url,"method":request.method,
            "referer":request.headers.get("Referer").unwrap()[0],
            "authorization":request.headers.get("Authorization").map(|v|v[0].as_str()).unwrap_or(""),
            "cookie":request.headers.get("Cookie").map(|v|v[0].as_str()).unwrap_or("")
        }));
        let path = url::Url::parse(&request.url).unwrap().path().to_owned();
        let archive = path.ends_with(".zip");
        let id = if archive {
            path.rsplit('/')
                .next()
                .unwrap()
                .trim_end_matches(".zip")
                .parse::<i64>()
                .unwrap()
        } else {
            0
        };
        if archive && self.rate_archive {
            return Err(rate_error("SaveResource"));
        }
        if id == 86 {
            return Err(Error::new(Reason::UpstreamUnavailable, "OpenResource")
                .with_cause(Cause::TransportFailure(Box::new(Cause::Canceled))));
        }
        let body: Self::Body = if archive {
            if let Some(pending) = &self.pending {
                let pending = pending.clone();
                Box::pin(PendingBody {
                    wait: Box::pin(async move {
                        loop {
                            if std::fs::read_dir(&pending.root).unwrap().any(|entry| {
                                let entry = entry.unwrap();
                                !entry.file_type().unwrap().is_dir()
                                    && entry.file_name().to_string_lossy().ends_with("_42.png")
                            }) {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                        }
                        pending.ready.notify_one();
                        std::future::pending::<()>().await;
                    }),
                })
            } else if id == 85 {
                Box::pin(FailedBody)
            } else {
                Box::pin(std::io::Cursor::new(decode_hex(
                    self.fixture["archive_hex"][id.to_string()]
                        .as_str()
                        .unwrap(),
                )))
            }
        } else {
            Box::pin(std::io::Cursor::new(decode_hex(
                self.fixture["stdio_cancellation"]["files"][0]["hex"]
                    .as_str()
                    .unwrap(),
            )))
        };
        Ok(ResourceResponse::new(
            if id == 84 { 503 } else { 200 },
            &ResourceHeaders::from([(
                "Content-Type".into(),
                vec![if archive {
                    "application/zip".into()
                } else {
                    "image/png".into()
                }],
            )]),
            body,
        ))
    }
}
struct PendingBody {
    wait: Pin<Box<dyn Future<Output = ()> + Send>>,
}
impl tokio::io::AsyncRead for PendingBody {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut PollContext<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.wait.as_mut().poll(context).map(|()| Ok(()))
    }
}
struct FailedBody;
impl tokio::io::AsyncRead for FailedBody {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut PollContext<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::Error::other(
            "synthetic resource body failure",
        )))
    }
}
pub struct FixtureSaveClient(pub Arc<Client<FixtureTransport>>);
impl DownloadSaveClient for FixtureSaveClient {
    fn artwork(&self, context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()),
            result=self.0.artwork(id)=>result.map_err(Into::into)}
        }))
    }
    fn ugoira_metadata(&self, context: Context, id: i64) -> Option<UgoiraMetadataFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()),
            result=self.0.ugoira_metadata(id)=>result.map_err(Into::into)}
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()),
            result=self.0.save_resource(reference,SaveOptions {
                path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(Into::into)}
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()),
            result=self.0.save_resource_url(&url,SaveOptions {
                path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(Into::into)}
        })
    }
}
fn artwork(id: i64) -> Value {
    if id == 42 {
        let metadata: Value =
            serde_json::from_str(include_str!("../fixtures/download_static.json")).unwrap();
        return metadata["metadata"]["42"].clone();
    }
    json!({"illust":{"id":id,"title":"Owned / ugoira","type":"ugoira","page_count":1,
        "create_date":if id == 88 {"2026-10-08T00:30:00+09:00"} else {"2026-10-08T12:34:56+09:00"},
        "user":{"id":7,"name":"Artist / name"},
        "tags":if id == 87 {json!([])} else {json!([{"name":" one "},{"name":"two"}])}}})
}
pub fn decode_hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
pub fn normalize(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace(root, "<ROOT>"),
        Value::Array(values) => values.iter_mut().for_each(|v| normalize(v, root)),
        Value::Object(values) => values.values_mut().for_each(|v| normalize(v, root)),
        _ => {}
    }
}
pub fn files(root: &Path) -> (Value, Value) {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<Value>, directories: &mut Vec<Value>) {
        let mut entries = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                directories.push(json!(name));
                visit(root, &path, files, directories);
            } else {
                files.push(json!({"name":name,"hex":std::fs::read(path).unwrap()
                    .iter().map(|byte|format!("{byte:02x}")).collect::<String>()}));
            }
        }
    }
    let mut files = vec![];
    let mut directories = vec![];
    visit(root, root, &mut files, &mut directories);
    (json!(files), json!(directories))
}
pub fn temporary_counts(root: &Path) -> (usize, usize, usize) {
    let (files, _) = files(root);
    let (mut published, mut archive, mut atomic) = (0, 0, 0);
    for file in files.as_array().unwrap() {
        let basename = file["name"].as_str().unwrap().rsplit('/').next().unwrap();
        if basename.starts_with(".atomic-write-") {
            atomic += 1;
        } else if basename.starts_with("ugoira-") {
            archive += 1;
        } else {
            published += 1;
        }
    }
    (published, archive, atomic)
}
