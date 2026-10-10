use pixiv_app::{
    database::Database,
    download::{
        ArtworkFuture, ArtworkPageFuture, DownloadSaveClient, SaveFuture, UgoiraMetadataFuture,
    },
    lifecycle::Context,
};
use pixiv_sdk::{
    Client,
    pixiv::{UserArtworkBookmarksRequest, UserArtworksRequest},
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions},
    transport::{
        JsonResponse, Request, ResourceReadRequest, ResourceTransport, Response, Transport,
    },
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
    pub active_adapters: usize,
    pub active_bodies: usize,
}
#[derive(Clone)]
pub struct Fixture {
    pub plans: Arc<BTreeMap<String, Vec<Value>>>,
    pub database: Arc<Mutex<Database>>,
    pub observed: Arc<Mutex<Observed>>,
    pub context: Arc<Mutex<Context>>,
    pub account: Arc<AtomicI64>,
    pub counts: Arc<Mutex<BTreeMap<String, usize>>>,
}
impl Fixture {
    pub fn new(row: &Value, database: Database) -> Self {
        let mut plans: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for (request, response) in row["requests"]
            .as_array()
            .unwrap()
            .iter()
            .zip(row["responses"].as_array().unwrap())
        {
            let key = format!(
                "{}:{}:{}",
                request["account"],
                request["method"].as_str().unwrap(),
                request["url"].as_str().unwrap()
            );
            plans.entry(key).or_default().push(response.clone());
        }
        Self {
            plans: Arc::new(plans),
            database: Arc::new(Mutex::new(database)),
            observed: Arc::new(Mutex::new(Observed::default())),
            context: Arc::new(Mutex::new(Context::new())),
            account: Arc::new(AtomicI64::new(0)),
            counts: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
    fn context(&self) -> Context {
        self.context.lock().unwrap().clone()
    }
    fn persisted(&self) {
        let id = self.account.load(Ordering::SeqCst);
        let database = self.database.lock().unwrap();
        let account = database.get_pixiv(id).unwrap();
        assert!(account.credential_revision >= 2);
        assert_eq!(
            account.refresh_token_copy(),
            format!("fixture-rotated-{id}").as_bytes()
        );
        assert!(self.observed.lock().unwrap().active_adapters > 0);
    }
    fn record(
        &self,
        method: &str,
        url: &str,
        referer: &str,
        cookie: &str,
        authorization: &str,
    ) -> pixiv_sdk::Result<Value> {
        let account = self.account.load(Ordering::SeqCst);
        self.observed.lock().unwrap().requests.push(json!({"account":account,"method":method,"url":url,"referer":referer,"cookie":cookie,"authorization":authorization}));
        let key = format!("{account}:{method}:{url}");
        let mut counts = self.counts.lock().unwrap();
        let ordinal = counts.entry(key.clone()).or_default();
        let Some(plan) = self.plans.get(&key).and_then(|plans| plans.get(*ordinal)) else {
            return Err(
                pixiv_sdk::Error::new(pixiv_sdk::Reason::UpstreamError, "fixture")
                    .with_detail(format!("unexpected owned SDK request {key}")),
            );
        };
        *ordinal += 1;
        Ok(plan.clone())
    }
    fn json_response(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
                .unwrap_or("")
        };
        if request.operation == "Open" {
            let token = &request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1;
            let id: i64 = token.rsplit('-').next().unwrap().parse().unwrap();
            assert_eq!(
                self.database
                    .lock()
                    .unwrap()
                    .get_pixiv(id)
                    .unwrap()
                    .refresh_token_copy(),
                token.as_bytes()
            );
            self.account.store(id, Ordering::SeqCst);
        } else {
            self.persisted();
            assert_eq!(
                header("Authorization"),
                format!(
                    "Bearer fixture-access-{}",
                    self.account.load(Ordering::SeqCst)
                )
            );
        }
        let mut url = url::Url::parse(&request.url).unwrap();
        assert!(matches!(
            url.host_str(),
            Some("app-api.pixiv.net" | "oauth.secure.pixiv.net")
        ));
        if request.operation != "Open" {
            url.query_pairs_mut()
                .extend_pairs(request.parameters.iter().map(|(key, value)| (key, value)));
        }
        let plan = self.record(
            request.method.as_str(),
            url.as_str(),
            header("Referer"),
            header("Cookie"),
            header("Authorization"),
        )?;
        let failure = plan["body_failure"].as_str().unwrap();
        if matches!(failure, "cancel" | "client-cancel") {
            if failure == "cancel" {
                self.context().cancel();
            }
            return Err(pixiv_sdk::Error::new(
                pixiv_sdk::Reason::UpstreamUnavailable,
                request.operation,
            )
            .with_cause(pixiv_sdk::error::Cause::TransportFailure(Box::new(
                pixiv_sdk::error::Cause::Canceled,
            ))));
        }
        if failure == "cancel-after" {
            self.context().cancel();
        }
        Ok(JsonResponse {
            status: plan["status"].as_u64().unwrap() as u16,
            retry_after: plan["retry_after"]
                .as_str()
                .unwrap()
                .parse::<i64>()
                .ok()
                .map(chrono::TimeDelta::seconds),
            body: decode_hex(plan["body_hex"].as_str().unwrap()),
        })
    }
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let operation = request.operation;
        let result = self.json_response(request);
        if result
            .as_ref()
            .is_err_and(|error| pixiv_sdk::error::is_canceled(error))
            && self.context().error().is_some()
        {
            tokio::task::yield_now().await;
        }
        let response = result?;
        Ok(Response {
            status: response.status,
            retry_after: response.retry_after,
            body: serde_json::from_slice(&response.body).map_err(|_| {
                pixiv_sdk::Error::new(pixiv_sdk::Reason::MalformedUpstreamResponse, operation)
            })?,
        })
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        let result = self.json_response(request);
        if result
            .as_ref()
            .is_err_and(|error| pixiv_sdk::error::is_canceled(error))
            && self.context().error().is_some()
        {
            tokio::task::yield_now().await;
        }
        result
    }
}
pub struct Body {
    bytes: Vec<u8>,
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
        let count = buffer.remaining().min(self.bytes.len());
        buffer.put_slice(&self.bytes[..count]);
        self.bytes.drain(..count);
        Poll::Ready(Ok(()))
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
        assert_eq!(
            url::Url::parse(&request.url).unwrap().host_str(),
            Some("i.pximg.net")
        );
        let header = |name| {
            request
                .headers
                .get(name)
                .and_then(|values| values.first())
                .map(String::as_str)
                .unwrap_or("")
        };
        let plan = self.record(
            request.method.as_str(),
            &request.url,
            header("Referer"),
            header("Cookie"),
            "",
        )?;
        let mut headers = ResourceHeaders::from([(
            "Content-Type".into(),
            vec![plan["content_type"].as_str().unwrap().into()],
        )]);
        if let Some(retry) = plan["retry_after"]
            .as_str()
            .filter(|retry| !retry.is_empty())
        {
            headers.insert("Retry-After".into(), vec![retry.into()]);
        }
        self.observed.lock().unwrap().active_bodies += 1;
        Ok(ResourceResponse::new(
            plan["status"].as_i64().unwrap(),
            &headers,
            Body {
                bytes: decode_hex(plan["body_hex"].as_str().unwrap()),
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
        self.fixture.observed.lock().unwrap().active_adapters -= 1;
    }
}
impl DownloadSaveClient for SaveClient {
    fn user_artworks(
        &self,
        context: Context,
        request: UserArtworksRequest,
    ) -> Option<ArtworkPageFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {biased; result=self.client.user_artworks(request)=>result.map_err(Into::into), error=context.cancelled()=>Err(error.into())}
        }))
    }
    fn user_artwork_bookmarks(
        &self,
        context: Context,
        request: UserArtworkBookmarksRequest,
    ) -> Option<ArtworkPageFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {biased; result=self.client.user_artwork_bookmarks(request)=>result.map_err(Into::into), error=context.cancelled()=>Err(error.into())}
        }))
    }
    fn artwork(&self, context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! { biased; error=context.cancelled()=>Err(error.into()), result=self.client.artwork(id)=>result.map_err(Into::into) }
        }))
    }
    fn ugoira_metadata(&self, context: Context, id: i64) -> Option<UgoiraMetadataFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! { biased; error=context.cancelled()=>Err(error.into()), result=self.client.ugoira_metadata(id)=>result.map_err(Into::into) }
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        // Direct opaque sources do not pass through the Go manager SaveResource observer.
        if path
            .file_name()
            .is_none_or(|name| !name.to_string_lossy().starts_with("resource-"))
        {
            self.fixture.observed.lock().unwrap().saves.push(json!({"ref":reference.to_string(),"destination":temporary_path(&path).replace(&self.root,"$ROOT")}));
        }
        Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()), result=self.client.save_resource(reference,SaveOptions{path:path.to_string_lossy().into_owned(),progress:None})=>result.map_err(Into::into)}
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()), result=self.client.save_resource_url(&url,SaveOptions{path:path.to_string_lossy().into_owned(),progress:None})=>result.map_err(Into::into)}
        })
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
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(
                !name.starts_with(".atomic-write-")
                    && !name.starts_with("ugoira-")
                    && !name.starts_with(".ugoira-"),
                "owned temp file leaked: {relative}"
            );
            files.insert(
                relative,
                std::fs::read(path)
                    .unwrap()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            );
        }
    }
}
fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[cfg(target_os = "linux")]
pub struct OneWorker(libc::cpu_set_t);
#[cfg(target_os = "linux")]
impl OneWorker {
    pub fn acquire() -> Self {
        let mut original: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe {
                libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut original)
            },
            0
        );
        let cpu = (0..libc::CPU_SETSIZE as usize)
            .find(|cpu| unsafe { libc::CPU_ISSET(*cpu, &original) })
            .unwrap();
        let mut selected: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        unsafe { libc::CPU_SET(cpu, &mut selected) };
        assert_eq!(
            unsafe {
                libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &selected)
            },
            0
        );
        assert_eq!(std::thread::available_parallelism().unwrap().get(), 1);
        Self(original)
    }
}
#[cfg(target_os = "linux")]
impl Drop for OneWorker {
    fn drop(&mut self) {
        assert_eq!(
            unsafe { libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &self.0) },
            0
        );
    }
}
