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
    io::{Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    task::{Context as TaskContext, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

#[derive(Default)]
pub struct Observed {
    pub requests: Vec<Value>,
    pub saves: Vec<Value>,
    pub writes: Vec<Value>,
    pub active_adapters: usize,
    pub active_bodies: usize,
    pub completed_adapters: usize,
}
#[derive(Clone)]
pub struct Fixture {
    pub row: Value,
    pub assets: Value,
    pub database: Arc<Mutex<Database>>,
    pub observed: Arc<Mutex<Observed>>,
    pub context: Arc<Mutex<Context>>,
    pub account: Arc<AtomicI64>,
    pub counts: Arc<Mutex<BTreeMap<String, usize>>>,
    pub disable: Arc<AtomicBool>,
}
impl Fixture {
    fn context(&self) -> Context {
        self.context.lock().unwrap().clone()
    }
    fn failure(&self) -> &str {
        if self.disable.load(Ordering::SeqCst) {
            ""
        } else {
            self.row["failure"].as_str().unwrap()
        }
    }
    fn record(&self, method: &str, url: &str, referer: &str, cookie: &str, authorization: &str) {
        self.observed.lock().unwrap().requests.push(json!({
            "account":self.account.load(Ordering::SeqCst), "method":method,"url":url,"referer":referer,"cookie":cookie,"authorization":authorization,
        }));
    }
    fn persisted(&self) {
        let id = self.account.load(Ordering::SeqCst);
        let db = self.database.lock().unwrap();
        let account = db.get_pixiv(id).unwrap();
        assert!(account.credential_revision >= 2);
        assert_eq!(
            account.refresh_token_copy(),
            format!("fixture-rotated-{id}").as_bytes()
        );
    }
    fn count(&self, key: String) -> usize {
        let mut counts = self.counts.lock().unwrap();
        let n = counts.entry(key).or_default();
        *n += 1;
        *n
    }
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
                .unwrap_or("")
        };
        let failure = self.failure();
        if request.operation == "Open" {
            let token = &request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1;
            let account: i64 = token.rsplit('-').next().unwrap().parse().unwrap();
            assert_eq!(
                self.database
                    .lock()
                    .unwrap()
                    .get_pixiv(account)
                    .unwrap()
                    .refresh_token_copy(),
                token.as_bytes()
            );
            self.account.store(account, Ordering::SeqCst);
            self.record(
                request.method.as_str(),
                &request.url,
                header("Referer"),
                header("Cookie"),
                header("Authorization"),
            );
            if failure == "refresh-cancel" {
                self.context().cancel();
                return Err(
                    pixiv_sdk::Error::new(pixiv_sdk::Reason::UpstreamError, "Open")
                        .with_cause(pixiv_sdk::error::Cause::Canceled),
                );
            }
            return Ok(Response {
                status: if account == 42 && failure == "refresh-rate" {
                    429
                } else if account == 42 && failure == "refresh-status" {
                    401
                } else {
                    200
                },
                retry_after: if account == 42 && failure == "refresh-rate" {
                    Some(chrono::TimeDelta::seconds(120))
                } else {
                    None
                },
                body: json!({"access_token":format!("fixture-access-{account}"),"refresh_token":format!("fixture-rotated-{account}"),"expires_in":3600,"user":{"id":account}}),
            });
        }
        self.persisted();
        let account = self.account.load(Ordering::SeqCst);
        assert_eq!(
            header("Authorization"),
            format!("Bearer fixture-access-{account}")
        );
        let mut url = url::Url::parse(&request.url).unwrap();
        url.query_pairs_mut()
            .extend_pairs(request.parameters.iter().map(|(k, v)| (k, v)));
        let id: i64 = url
            .query_pairs()
            .find(|(key, _)| key == "illust_id")
            .unwrap()
            .1
            .parse()
            .unwrap();
        self.record(
            request.method.as_str(),
            url.as_str(),
            header("Referer"),
            header("Cookie"),
            header("Authorization"),
        );
        let path = url.path();
        assert!(matches!(path, "/v1/illust/detail" | "/v1/ugoira/metadata"));
        let target = id == self.row["failure_id"].as_i64().unwrap();
        let failure_account = account == self.row["failure_account"].as_i64().unwrap();
        let variant_rate = target
            && failure == "variant-rate"
            && path == "/v1/illust/detail"
            && failure_account
            && self.count(format!("variant:{account}:{id}")) >= 3;
        let limited = target
            && ((failure == "metadata-rate" && path == "/v1/illust/detail"
                || failure == "ugoira-rate" && path == "/v1/ugoira/metadata")
                && failure_account
                || failure == "all-rate"
                || variant_rate);
        let retry_after = if limited {
            Some(chrono::TimeDelta::seconds(
                if self.count(format!("{account}:{path}:{id}")) % 2 == 1 {
                    0
                } else {
                    120
                },
            ))
        } else {
            None
        };
        let status = if limited {
            429
        } else if target
            && (failure == "metadata-status" && path == "/v1/illust/detail"
                || failure == "ugoira-status" && path == "/v1/ugoira/metadata" && failure_account)
        {
            404
        } else {
            200
        };
        let body = if path == "/v1/ugoira/metadata" {
            json!({"ugoira_metadata":{"zip_urls":{"medium":format!("https://i.pximg.net/ugoira/{id}_medium.zip"),"original":format!("https://i.pximg.net/ugoira/{id}_original.zip")},"frames":[{"file":"000.png","delay":70},{"file":"001.png","delay":130}]}})
        } else {
            artwork(id)
        };
        Ok(Response {
            status,
            retry_after,
            body,
        })
    }
}
fn artwork(id: i64) -> Value {
    let pages = if id == 43 { 2 } else { 1 };
    let kind = if id == 43 {
        "manga"
    } else if id == 44 {
        "ugoira"
    } else {
        "illust"
    };
    let original =
        |page| format!("https://i.pximg.net/img-original/img/2026/01/02/03/04/05/{id}_p{page}.jpg");
    let mut artwork = json!({"id":id,"type":kind,"title":"Title: slash/","user":{"id":7,"name":"Author?"},"create_date":"2026-01-02T03:04:05+00:00","page_count":pages,"tags":[{"name":"first"}],"image_urls":{"large":original(0)},"meta_single_page":{"original_image_url":original(0)}});
    if pages > 1 {
        artwork["meta_pages"] = json!(
            (0..pages)
                .map(|page| json!({"image_urls":{"original":original(page)}}))
                .collect::<Vec<_>>()
        );
    }
    json!({"illust":artwork})
}
pub struct Body {
    bytes: Vec<u8>,
    failure: &'static str,
    context: Context,
    observed: Arc<Mutex<Observed>>,
}
impl Drop for Body {
    fn drop(&mut self) {
        self.observed.lock().unwrap().active_bodies -= 1;
    }
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
        Poll::Ready(match self.failure {
            "read" => Err(std::io::Error::other("fixture body failure")),
            "cancel" => {
                self.context.cancel();
                Err(std::io::Error::other(pixiv_sdk::error::Cause::Canceled))
            }
            _ => Ok(()),
        })
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
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|v| v.first())
                .map(String::as_str)
                .unwrap_or("")
        };
        self.record(
            request.method.as_str(),
            &request.url,
            header("Referer"),
            header("Cookie"),
            "",
        );
        let url = url::Url::parse(&request.url).unwrap();
        let basename = url.path().rsplit('/').next().unwrap();
        let id: i64 = basename.split('_').next().unwrap().parse().unwrap();
        let archive = url.path().contains("ugoira");
        let failure = self.failure();
        let target = id == self.row["failure_id"].as_i64().unwrap();
        let mut bytes = if archive {
            let invalid = target
                && failure == "invalid-image"
                && self.account.load(Ordering::SeqCst)
                    == self.row["failure_account"].as_i64().unwrap();
            decode_hex(
                self.assets[if invalid {
                    "ugoira_invalid-image"
                } else {
                    "ugoira_valid"
                }]
                .as_str()
                .unwrap(),
            )
        } else {
            format!("fixture image {id} {basename}").into_bytes()
        };
        let body_failure = if target
            && (failure == "body-cancel" || failure == "page-cancel" && url.path().contains("_p1."))
        {
            "cancel"
        } else if target && failure == "body-read" && url.path().contains("_p1.") {
            "read"
        } else {
            ""
        };
        if !body_failure.is_empty() {
            bytes.truncate(bytes.len() / 2);
        }
        let limited = target && failure == "page-rate" && url.path().contains("_p1.");
        let mut headers = ResourceHeaders::from([(
            "Content-Type".into(),
            vec![
                if archive {
                    "application/zip"
                } else {
                    self.row["static_content_type"].as_str().unwrap()
                }
                .into(),
            ],
        )]);
        if limited {
            headers.insert("Retry-After".into(), vec!["120".into()]);
        }
        self.observed.lock().unwrap().active_bodies += 1;
        Ok(ResourceResponse::new(
            if limited { 429 } else { 200 },
            &headers,
            Body {
                bytes,
                failure: body_failure,
                context: self.context(),
                observed: self.observed.clone(),
            },
        ))
    }
}
pub struct SaveClient {
    pub client: Arc<Client<Fixture>>,
    pub fixture: Fixture,
    pub root: String,
}
impl Drop for SaveClient {
    fn drop(&mut self) {
        let mut observed = self.fixture.observed.lock().unwrap();
        observed.active_adapters -= 1;
        observed.completed_adapters += 1;
        let cancel = self.fixture.failure() == "after-first" && observed.completed_adapters == 1;
        drop(observed);
        if cancel {
            self.fixture.context().cancel();
        }
    }
}
impl DownloadSaveClient for SaveClient {
    fn artwork(&self, context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {biased;error=context.cancelled()=>Err(error.into()),result=self.client.artwork(id)=>result.map_err(Into::into)}
        }))
    }
    fn ugoira_metadata(&self, context: Context, id: i64) -> Option<UgoiraMetadataFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {biased;error=context.cancelled()=>Err(error.into()),result=self.client.ugoira_metadata(id)=>result.map_err(Into::into)}
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        self.fixture.observed.lock().unwrap().saves.push(json!({"ref":reference.to_string(),"destination":temporary_path(&path).replace(&self.root,"$ROOT")}));
        Box::pin(async move {
            tokio::select! {biased;error=context.cancelled()=>Err(error.into()),result=self.client.save_resource(reference,SaveOptions{path:path.to_string_lossy().into_owned(),progress:None})=>result.map_err(Into::into)}
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased;error=context.cancelled()=>Err(error.into()),result=self.client.save_resource_url(&url,SaveOptions{path:path.to_string_lossy().into_owned(),progress:None})=>result.map_err(Into::into)}
        })
    }
}
pub struct Writer {
    pub bytes: Arc<Mutex<Vec<u8>>>,
    pub observed: Arc<Mutex<Observed>>,
    pub failures: usize,
    pub persistent: bool,
    pub root: String,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let failed = self.persistent || self.failures > 0;
        self.failures = self.failures.saturating_sub(1);
        let mut observed = self.observed.lock().unwrap();
        observed.writes.push(json!({"text":String::from_utf8_lossy(bytes).replace(&self.root,"$ROOT"),"failed":failed}));
        drop(observed);
        if failed {
            return Err(std::io::Error::other("fixture writer failure"));
        }
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) -> std::io::Result<()> {
        self.write_all(args.to_string().as_bytes())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub struct Reader {
    pub bytes: Vec<u8>,
    pub position: usize,
    pub failure: String,
    pub context: Context,
    pub returned: Arc<Mutex<Vec<u8>>>,
}
impl Read for Reader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.failure == "classification" {
            return Err(std::io::Error::other("fixture input failure"));
        }
        if self.position == self.bytes.len() {
            return if matches!(self.failure.as_str(), "after-first" | "partial") {
                Err(std::io::Error::other("fixture input failure"))
            } else {
                Ok(0)
            };
        }
        let mut count = out.len().min(self.bytes.len() - self.position);
        if self.failure == "after-first"
            && let Some(end) = self.bytes[self.position..].iter().position(|b| *b == b'\n')
        {
            count = count.min(end + 1);
        }
        out[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
        self.position += count;
        self.returned
            .lock()
            .unwrap()
            .extend_from_slice(&out[..count]);
        if self.failure == "cancel-classified" && self.position == 1 {
            self.context.cancel();
        }
        Ok(count)
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
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if entry.file_type().unwrap().is_dir() {
            directories.push(relative);
            manifest(root, &path, files, directories);
        } else {
            assert!(
                !name.starts_with(".atomic-write-")
                    && !name.starts_with("ugoira-")
                    && !name.starts_with(".ugoira-"),
                "owned temporary file leaked: {relative}"
            );
            files.insert(relative, encode_hex(&std::fs::read(path).unwrap()));
        }
    }
}
fn temporary_path(path: &Path) -> String {
    if path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("ugoira-")
        && path.extension().is_some_and(|ext| ext == "zip")
    {
        path.with_file_name("ugoira-$TEMP.zip")
            .to_string_lossy()
            .into_owned()
    } else {
        path.to_string_lossy().into_owned()
    }
}
fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn encode_hex(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
