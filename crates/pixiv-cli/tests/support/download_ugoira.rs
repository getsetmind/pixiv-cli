use pixiv_app::{
    database::Database,
    download::{ArtworkFuture, DownloadSaveClient, SaveFuture, UgoiraMetadataFuture},
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
    fn record(&self, method: &str, url: &str, referer: &str, cookie: &str, authorization: &str) {
        self.observed.lock().unwrap().requests.push(json!({
            "account":self.account.load(Ordering::SeqCst),
            "method":method,"url":url,"referer":referer,"cookie":cookie,"authorization":authorization,
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
            self.record(request.method.as_str(), &request.url, referer, cookie, "");
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
        assert!(matches!(
            url.path(),
            "/v1/illust/detail" | "/v1/ugoira/metadata"
        ));
        assert_eq!(url.query(), Some("illust_id=42"));
        self.record(
            request.method.as_str(),
            url.as_str(),
            referer,
            cookie,
            &format!("Bearer fixture-access-{id}"),
        );
        let pages = self.row["metadata_pages"].as_u64().unwrap();
        let original = |page: u64| {
            format!("https://i.pximg.net/img-original/img/2026/01/02/03/04/05/42_p{page}.jpg")
        };
        let mut artwork = json!({
            "id":42,"type":self.row["metadata_kind"],"title":"Title: slash/","user":{"id":7,"name":"Author?"},"create_date":"2026-01-02T03:04:05+00:00","page_count":pages,"tags":[{"name":" first "},{"name":"second"}],"image_urls":{"large":original(0)},"meta_single_page":{"original_image_url":original(0)},
        });
        if self.row["failure"] == "empty-basename" {
            artwork["tags"] = json!([]);
        }
        if pages > 1 {
            artwork["meta_pages"] = Value::Array(
                (0..pages)
                    .map(|page| json!({"image_urls":{"original":original(page)}}))
                    .collect(),
            );
        }
        let id = self.account.load(Ordering::SeqCst);
        let failure = self.row["failure"].as_str().unwrap();
        if url.path() == "/v1/ugoira/metadata" {
            let mut urls = json!({"medium":"https://i.pximg.net/ugoira/42_medium.zip", "original":"https://i.pximg.net/ugoira/42_original.zip"});
            if failure == "medium-only" {
                urls.as_object_mut().unwrap().remove("original");
            }
            if failure == "no-archive" {
                urls = json!({});
            }
            let mut frames = json!([{"file":"000.png","delay":70},{"file":"001.png","delay":130}]);
            if failure == "declared-duplicate" {
                frames[1]["file"] = json!("000.png");
            }
            if failure == "declared-unsafe" {
                frames[1]["file"] = json!("..foo");
            }
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"ugoira_metadata":{"zip_urls":urls,"frames":frames}}),
            });
        }
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
        self.record(request.method.as_str(), &request.url, referer, cookie, "");
        let archive = request.url.contains("ugoira");
        let failed = archive;
        let failure = self.row["failure"].as_str().unwrap();
        let status =
            if (archive && failure == "archive-rate") || (!archive && failure == "direct-rate") {
                429
            } else {
                200
            };
        let mut headers = ResourceHeaders::from([(
            "Content-Type".into(),
            vec![
                if archive {
                    "application/zip"
                } else {
                    "image/png"
                }
                .into(),
            ],
        )]);
        if status == 429 {
            headers.insert("Retry-After".into(), vec!["120".into()]);
        }
        Ok(ResourceResponse::new(
            status,
            &headers,
            Body {
                bytes: if archive {
                    decode_hex(self.row["archive_hex"].as_str().unwrap())
                } else {
                    b"payload".to_vec()
                },
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
}
impl Drop for SaveClient {
    fn drop(&mut self) {
        let mut observed = self.observed.lock().unwrap();
        observed.active -= 1;
        observed.closes += 1;
        observed
            .commits
            .push(published(std::path::Path::new(&self.root)));
    }
}
impl DownloadSaveClient for SaveClient {
    fn ugoira_metadata(&self, context: Context, id: i64) -> Option<UgoiraMetadataFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! { biased;
                error=context.cancelled()=>Err(error.into()),
                result=self.client.ugoira_metadata(id)=>result.map_err(Into::into),
            }
        }))
    }
    fn artwork(&self, context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! { biased;
                error=context.cancelled()=>Err(error.into()),
                result=self.client.artwork(id)=>result.map_err(Into::into),
            }
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        self.observed.lock().unwrap().saves.push(json!({"ref":reference.to_string(),"destination":temporary_path(&path).replace(&self.root,"$ROOT")}));
        Box::pin(async move {
            tokio::select! {biased;
                error=context.cancelled()=>Err(error.into()),
                result=self.client.save_resource(reference,SaveOptions{path:path.to_string_lossy().into_owned(),progress:None})=>result.map_err(Into::into),
            }
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased;
                error=context.cancelled()=>Err(error.into()),
                result=self.client.save_resource_url(&url,SaveOptions{path:path.to_string_lossy().into_owned(),progress:None})=>result.map_err(Into::into),
            }
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
            files.insert(relative, encode_hex(&std::fs::read(path).unwrap()));
        }
    }
}

pub fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
pub fn encode_hex(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn temporary_path(path: &Path) -> String {
    if path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("ugoira-")
        && path.extension().is_some_and(|extension| extension == "zip")
    {
        path.with_file_name("ugoira-$TEMP.zip")
            .to_string_lossy()
            .into_owned()
    } else {
        path.to_string_lossy().into_owned()
    }
}
fn published(root: &Path) -> bool {
    std::fs::read_dir(root).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".quarantine"
                || name.starts_with("ugoira-")
                || name.starts_with(".ugoira-")
                || name.starts_with(".atomic-write-")
            {
                return false;
            }
            if path.is_dir() {
                return published(&path);
            }
            std::fs::read(path).is_ok_and(|bytes| bytes != b"old animation")
        })
    })
}
