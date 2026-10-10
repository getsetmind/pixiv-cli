#[allow(dead_code)]
#[path = "download_ugoira.rs"]
mod existing_media;
pub use existing_media::{decode_hex, files, normalize};

use pixiv_app::{
    database::Database,
    download::{ArtworkFuture, DownloadSaveClient, SaveFuture, UgoiraMetadataFuture},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    Client, Error, Reason,
    error::{Cause, TransportKind},
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions},
    transport::{
        Request, ResourceBody, ResourceReadRequest, ResourceTransport, Response, Transport,
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context as PollContext, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

#[derive(Default)]
pub struct Observed {
    pub requests: Mutex<Vec<Value>>,
    pub responses: Mutex<Vec<Value>>,
    pub opens: Mutex<Vec<i64>>,
    pub proxies: Mutex<Vec<Option<String>>>,
    pub factories: AtomicUsize,
    pub active: AtomicUsize,
    pub drops: AtomicUsize,
    pub active_reads: Arc<AtomicUsize>,
    pub request_context: Mutex<Option<Context>>,
    pub recommendation_index: AtomicUsize,
}
#[derive(Clone)]
pub struct PendingBody {
    pub ready: Arc<tokio::sync::Notify>,
    pub kind: String,
}
pub struct FixtureTransport {
    pub observed: Arc<Observed>,
    pub fixture: Arc<Value>,
    pub row: Value,
    pub database: Arc<Mutex<Database>>,
    pub responses: Mutex<BTreeMap<String, VecDeque<Value>>>,
    pub pending: Option<PendingBody>,
}
impl Drop for FixtureTransport {
    fn drop(&mut self) {
        self.observed.active.fetch_sub(1, Ordering::SeqCst);
        self.observed.drops.fetch_add(1, Ordering::SeqCst);
    }
}
pub fn scripts(row: &Value) -> BTreeMap<String, VecDeque<Value>> {
    let mut scripts = BTreeMap::<String, VecDeque<Value>>::new();
    for response in row["responses"].as_array().unwrap() {
        scripts
            .entry(response["url"].as_str().unwrap().into())
            .or_default()
            .push_back(response.clone());
    }
    scripts
}
fn header(headers: &[(String, String)], key: &str) -> String {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}
fn canceled(operation: &'static str) -> Error {
    Error::new(Reason::UpstreamUnavailable, operation)
        .with_transport(TransportKind::Http)
        .with_cause(Cause::TransportFailure(Box::new(Cause::Canceled)))
}
impl FixtureTransport {
    fn response(&self, url: &str) -> Value {
        let response = self
            .responses
            .lock()
            .unwrap()
            .get_mut(url)
            .unwrap_or_else(|| panic!("unfrozen response URL {url}"))
            .pop_front()
            .unwrap_or_else(|| panic!("response replay exceeded frozen count {url}"));
        self.observed
            .responses
            .lock()
            .unwrap()
            .push(response.clone());
        response
    }
    fn context(&self) -> Context {
        self.observed
            .request_context
            .lock()
            .unwrap()
            .clone()
            .unwrap()
    }
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
        self.observed.requests.lock().unwrap().push(json!({"url":url,"method":request.method.as_str(),"referer":header(&request.headers,"Referer"),"authorization":header(&request.headers,"Authorization"),"cookie":header(&request.headers,"Cookie")}));
        let response = self.response(&url);
        if !response["transport_error"].as_str().unwrap().is_empty() {
            return Err(canceled(request.operation));
        }
        let status = response["status"].as_u64().unwrap() as u16;
        let retry_after = response["retry_after"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .ok()
            .map(chrono::TimeDelta::seconds);
        let body = if request.operation == "Open" {
            let token = &request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1;
            let id = token.rsplit('-').next().unwrap().parse::<i64>().unwrap();
            self.observed.opens.lock().unwrap().push(id);
            let failure = self.row["refresh_failure"].as_str().unwrap();
            if failure == "cas" {
                self.database
                    .lock()
                    .unwrap()
                    .rotate_pixiv_credentials(id, 1, format!("fixture-competing-{id}").as_bytes())
                    .unwrap()
            }
            if failure == "persist" {
                let path = self.database.lock().unwrap().path().to_path_buf();
                rusqlite::Connection::open(path).unwrap().execute_batch("CREATE TRIGGER fixture_rotation_failure BEFORE UPDATE OF refresh_token ON pixiv_account BEGIN SELECT RAISE(FAIL, 'owned fixture persist failure'); END").unwrap();
            }
            if failure == "http" {
                json!({"error":{"message":"owned fixture refresh failed"}})
            } else {
                json!({"access_token":format!("fixture-access-{id}"),"refresh_token":format!("fixture-rotated-{id}"),"expires_in":3600,"user":{"id":if failure=="identity" {id+1} else {id}}})
            }
        } else if request.url.ends_with("/v1/illust/recommended") {
            assert!(
                request.parameters.is_empty(),
                "random recommendation must remain first page without query"
            );
            let index = self
                .observed
                .recommendation_index
                .fetch_add(1, Ordering::SeqCst);
            let body = self.row["recommendation_responses"][index]["body"].clone();
            if let Some(pending) = &self.pending
                && pending.kind == "recommendation"
                && index == 0
            {
                let mut reader = PendingRead::new(
                    self.context(),
                    pending.ready.clone(),
                    self.observed.active_reads.clone(),
                );
                let mut bytes = vec![];
                reader
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| canceled(request.operation))?;
                serde_json::from_slice(&bytes).unwrap()
            } else {
                body
            }
        } else {
            let id = request
                .parameters
                .iter()
                .find(|(key, _)| key == "illust_id")
                .unwrap()
                .1
                .parse::<i64>()
                .unwrap();
            if request.url.ends_with("/v1/illust/detail") {
                if status == 404 {
                    json!({"error":{"message":"fixture missing artwork"}})
                } else {
                    self.row["detail_overrides"]
                        .get(id.to_string())
                        .cloned()
                        .unwrap_or_else(|| self.fixture["artwork_metadata"][id.to_string()].clone())
                }
            } else {
                assert!(request.url.ends_with("/v1/ugoira/metadata"));
                self.fixture["ugoira_metadata"].clone()
            }
        };
        Ok(Response {
            status,
            retry_after,
            body,
        })
    }
}
impl ResourceTransport for FixtureTransport {
    type Body = ResourceBody;
    async fn open_resource(
        &self,
        request: ResourceReadRequest,
    ) -> pixiv_sdk::Result<ResourceResponse<Self::Body>> {
        request.validate.as_ref().unwrap()(&request.url)?;
        self.observed.requests.lock().unwrap().push(json!({"url":request.url,"method":request.method,"referer":request.headers.get("Referer").unwrap()[0],"authorization":request.headers.get("Authorization").map(|v|v[0].as_str()).unwrap_or(""),"cookie":request.headers.get("Cookie").map(|v|v[0].as_str()).unwrap_or("")}));
        let response = self.response(&request.url);
        if !response["transport_error"].as_str().unwrap().is_empty() {
            return Err(canceled(request.operation));
        }
        let archive = url::Url::parse(&request.url)
            .unwrap()
            .path()
            .ends_with(".zip");
        let body: ResourceBody = if let Some(pending) = &self.pending
            && pending.kind == "media"
            && request.url.contains("44_p1")
        {
            Box::pin(PendingRead::new(
                self.context(),
                pending.ready.clone(),
                self.observed.active_reads.clone(),
            ))
        } else {
            Box::pin(std::io::Cursor::new(decode_hex(
                self.fixture[if archive { "archive_hex" } else { "static_hex" }]
                    .as_str()
                    .unwrap(),
            )))
        };
        Ok(ResourceResponse::new(
            response["status"].as_i64().unwrap(),
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
struct PendingRead {
    wait: Pin<Box<dyn Future<Output = ()> + Send>>,
    ready: Arc<tokio::sync::Notify>,
    entered: bool,
    active: Arc<AtomicUsize>,
}
impl PendingRead {
    fn new(context: Context, ready: Arc<tokio::sync::Notify>, active: Arc<AtomicUsize>) -> Self {
        active.fetch_add(1, Ordering::SeqCst);
        Self {
            wait: Box::pin(async move {
                context.cancelled().await;
            }),
            ready,
            entered: false,
            active,
        }
    }
}
impl Drop for PendingRead {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}
impl AsyncRead for PendingRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut PollContext<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.entered {
            self.entered = true;
            self.ready.notify_one();
        }
        self.wait.as_mut().poll(context).map(|()| {
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "context canceled",
            ))
        })
    }
}
pub struct FixtureSaveClient {
    pub client: Arc<Client<FixtureTransport>>,
    pub remove_published: bool,
    pub remove_on_drop: bool,
    pub published: Mutex<Vec<PathBuf>>,
}
impl Drop for FixtureSaveClient {
    fn drop(&mut self) {
        if self.remove_on_drop {
            for path in self.published.lock().unwrap().iter() {
                std::fs::remove_file(path).unwrap();
            }
        }
    }
}
impl DownloadSaveClient for FixtureSaveClient {
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
        Box::pin(async move {
            let saved = tokio::select! {biased;error=context.cancelled()=>Err(SchedulerError::from(error)),result=self.client.save_resource(reference,SaveOptions{path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(SchedulerError::from)}?;
            if self.remove_published {
                std::fs::remove_file(&saved.path).unwrap();
            }
            if self.remove_on_drop {
                self.published
                    .lock()
                    .unwrap()
                    .push(PathBuf::from(&saved.path).with_extension("png"));
            }
            Ok(saved)
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased;error=context.cancelled()=>Err(error.into()),result=self.client.save_resource_url(&url,SaveOptions{path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(Into::into)}
        })
    }
}
pub fn normalize_order(values: &mut [Value], row: &Value) {
    let mut start = 0;
    let mut attempt = 0;
    while start < values.len() {
        let end = (start + 1..values.len())
            .find(|&i| values[i]["url"] == "https://oauth.secure.pixiv.net/auth/token")
            .unwrap_or(values.len());
        let media = (start..end)
            .find(|&i| {
                let url = values[i]["url"].as_str().unwrap();
                url != "https://oauth.secure.pixiv.net/auth/token"
                    && url != "https://app-api.pixiv.net/v1/illust/recommended"
            })
            .unwrap_or(end);
        assert!(
            values[media..end]
                .iter()
                .all(|v| v["url"] != "https://app-api.pixiv.net/v1/illust/recommended"),
            "recommendation began after media within RPC"
        );
        if let Some(request) = row["manager_requests"].get(attempt) {
            let ids = request["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap())
                .collect::<std::collections::BTreeSet<_>>();
            if ids.len() > 1 {
                values[media..end].sort_by(|a, b| a["url"].as_str().cmp(&b["url"].as_str()));
            }
        }
        start = end;
        attempt += 1;
    }
}
