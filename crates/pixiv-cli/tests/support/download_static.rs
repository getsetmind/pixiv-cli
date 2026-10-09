use pixiv_app::{
    database::Database,
    download::{ArtworkFuture, DownloadSaveClient, SaveFuture},
    lifecycle::Context,
};
use pixiv_sdk::{
    Client,
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    task::{Context as TaskContext, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

#[derive(Default)]
pub struct Observed {
    pub requests: Vec<Value>,
    pub saves: Vec<Value>,
    pub commits: Vec<bool>,
    pub active: usize,
    pub adapters: usize,
    pub closes: usize,
    pub writer_under_lease: bool,
}
#[derive(Clone)]
pub struct Fixture {
    pub row: Value,
    pub database: Arc<Mutex<Database>>,
    pub observed: Arc<Mutex<Observed>>,
    pub context: Context,
    pub account: Arc<AtomicI64>,
    pub counts: Arc<Mutex<BTreeMap<i64, usize>>>,
}
impl Fixture {
    fn record(&self, method: &str, url: &str, referer: &str, cookie: &str) {
        self.observed.lock().unwrap().requests.push(json!({
            "account":self.account.load(Ordering::SeqCst),
            "method":method,"url":url,"referer":referer,"cookie":cookie,
        }));
    }
    fn persisted(&self) {
        let id = self.account.load(Ordering::SeqCst);
        let account = self.database.lock().unwrap().get_pixiv(id).unwrap();
        assert_eq!(account.credential_revision, 2);
        assert_eq!(
            account.refresh_token_copy(),
            format!("fixture-rotated-{id}").as_bytes()
        );
    }
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let referer = request
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("referer"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        let cookie = request
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("cookie"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        if request.operation == "Open" {
            let token = &request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1;
            let id: i64 = token.rsplit('-').next().unwrap().parse().unwrap();
            let stored = self.database.lock().unwrap().get_pixiv(id).unwrap();
            assert_eq!(stored.refresh_token_copy(), token.as_bytes());
            self.account.store(id, Ordering::SeqCst);
            self.record(request.method.as_str(), &request.url, referer, cookie);
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({
                    "access_token":format!("fixture-access-{id}"),"refresh_token":format!("fixture-rotated-{id}"),"expires_in":3600,"user":{"id":id},
                }),
            });
        }
        self.persisted();
        let id = self.account.load(Ordering::SeqCst);
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                    && value == &format!("Bearer fixture-access-{id}"))
        );
        let mut url = url::Url::parse(&request.url).unwrap();
        url.query_pairs_mut()
            .extend_pairs(request.parameters.iter().map(|(k, v)| (k, v)));
        assert_eq!(url.path(), "/v1/illust/detail");
        assert_eq!(url.query(), Some("illust_id=42"));
        self.record(request.method.as_str(), url.as_str(), referer, cookie);
        let pages = self.row["metadata_pages"].as_u64().unwrap();
        let original = |page: u64| {
            format!("https://i.pximg.net/img-original/img/2026/01/02/03/04/05/42_p{page}.jpg")
        };
        let mut artwork = json!({
            "id":42,"type":self.row["metadata_kind"],"title":"Title: slash/","user":{"id":7,"name":"Author?"},"create_date":"2026-01-02T03:04:05+00:00","page_count":pages,"tags":[{"name":" first "},{"name":"second"}],"image_urls":{"large":original(0)},"meta_single_page":{"original_image_url":original(0)},
        });
        if pages > 1 {
            artwork["meta_pages"] = Value::Array(
                (0..pages)
                    .map(|page| json!({"image_urls":{"original":original(page)}}))
                    .collect(),
            );
        }
        let id = self.account.load(Ordering::SeqCst);
        let failure = self.row["failure"].as_str().unwrap();
        let limited = failure == "all-rate" || failure == "metadata-rate" && id == 42;
        let retry_after = if limited {
            let mut counts = self.counts.lock().unwrap();
            let count = counts.entry(id).or_default();
            *count += 1;
            Some(chrono::TimeDelta::seconds(if *count % 2 == 1 {
                0
            } else {
                120
            }))
        } else {
            None
        };
        Ok(Response {
            status: if limited { 429 } else { 200 },
            retry_after,
            body: json!({"illust":artwork}),
        })
    }
}
pub struct Body {
    bytes: Vec<u8>,
    failure: String,
    context: Context,
}
impl AsyncRead for Body {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.bytes.is_empty() {
            let count = buffer.remaining().min(self.bytes.len());
            buffer.put_slice(&self.bytes[..count]);
            self.bytes.drain(..count);
            return Poll::Ready(Ok(()));
        }
        match self.failure.as_str() {
            "read" => Poll::Ready(Err(std::io::Error::other("fixture body failure"))),
            "cancel" => {
                self.context.cancel();
                Poll::Ready(Err(std::io::Error::other(
                    pixiv_sdk::error::Cause::Canceled,
                )))
            }
            _ => Poll::Ready(Ok(())),
        }
    }
}
impl ResourceTransport for Fixture {
    type Body = Body;
    async fn open_resource(
        &self,
        request: ResourceReadRequest,
    ) -> pixiv_sdk::Result<ResourceResponse<Body>> {
        self.persisted();
        assert!(!request.headers.contains_key("Authorization"));
        let referer = request
            .headers
            .get("Referer")
            .and_then(|v| v.first())
            .map(String::as_str)
            .unwrap_or("");
        let cookie = request
            .headers
            .get("Cookie")
            .and_then(|v| v.first())
            .map(String::as_str)
            .unwrap_or("");
        self.record(request.method.as_str(), &request.url, referer, cookie);
        let failed = request.url.contains("42_p1");
        let failure = self.row["failure"].as_str().unwrap();
        let status = if failed && failure == "page-rate" {
            429
        } else {
            200
        };
        let mut headers =
            ResourceHeaders::from([("Content-Type".into(), vec!["image/png".into()])]);
        if status == 429 {
            headers.insert("Retry-After".into(), vec!["120".into()]);
        }
        Ok(ResourceResponse::new(
            status,
            &headers,
            Body {
                bytes: b"payload".to_vec(),
                failure: if failed && matches!(failure, "read" | "cancel") {
                    failure.into()
                } else {
                    String::new()
                },
                context: self.context.clone(),
            },
        ))
    }
}
pub struct SaveClient {
    pub client: Arc<Client<Fixture>>,
    pub observed: Arc<Mutex<Observed>>,
    pub root: String,
    pub committed: std::sync::atomic::AtomicBool,
}
impl Drop for SaveClient {
    fn drop(&mut self) {
        let mut observed = self.observed.lock().unwrap();
        observed.active -= 1;
        observed.closes += 1;
        observed.commits.push(self.committed.load(Ordering::SeqCst));
    }
}
impl DownloadSaveClient for SaveClient {
    fn artwork(&self, context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! { biased;
                error=context.cancelled()=>Err(error.into()),
                result=self.client.artwork(id)=>result.map_err(Into::into),
            }
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        self.observed.lock().unwrap().saves.push(json!({"ref":reference.to_string(),"destination":path.to_string_lossy().replace(&self.root,"$ROOT")}));
        Box::pin(async move {
            let result = tokio::select! {biased;
                error=context.cancelled()=>Err(error.into()),
                result=self.client.save_resource(reference,SaveOptions{path:path.to_string_lossy().into_owned(),progress:None})=>result.map_err(Into::into),
            };
            if result.is_ok() {
                self.committed.store(true, Ordering::SeqCst);
            }
            result
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            let result = tokio::select! {biased;
                error=context.cancelled()=>Err(error.into()),
                result=self.client.save_resource_url(&url,SaveOptions{path:path.to_string_lossy().into_owned(),progress:None})=>result.map_err(Into::into),
            };
            if result.is_ok() {
                self.committed.store(true, Ordering::SeqCst);
            }
            result
        })
    }
}
pub struct Writer {
    pub bytes: Arc<Mutex<Vec<u8>>>,
    pub observed: Arc<Mutex<Observed>>,
    pub fail: bool,
}
impl std::io::Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut observed = self.observed.lock().unwrap();
        if observed.active > 0 {
            observed.writer_under_lease = true;
        }
        if self.fail {
            return Err(std::io::Error::other("fixture writer failure"));
        }
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub fn manifest(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, String>,
    directories: &mut Vec<String>,
) {
    let mut entries = std::fs::read_dir(dir)
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if entry.file_type().unwrap().is_dir() {
            directories.push(relative);
            manifest(root, &path, files, directories);
        } else {
            files.insert(relative, std::fs::read_to_string(path).unwrap());
        }
    }
}
