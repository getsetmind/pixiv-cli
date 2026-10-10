#[allow(dead_code)]
#[path = "download_ugoira.rs"]
mod existing_media;
pub use existing_media::{decode_hex, files, normalize};

use pixiv_app::{
    download::{
        ArtworkFuture, ArtworkPageFuture, DownloadSaveClient, SaveFuture, UgoiraMetadataFuture,
    },
    lifecycle::Context,
};
use pixiv_sdk::{
    Client, Error, Reason,
    error::{Cause, TransportKind},
    pixiv::{UserArtworkBookmarksRequest, UserArtworksRequest},
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
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
    pub factories: AtomicUsize,
    pub active: AtomicUsize,
    pub drops: AtomicUsize,
    pub active_reads: Arc<AtomicUsize>,
    pub request_context: Mutex<Option<Context>>,
}
#[derive(Clone)]
pub struct PendingList {
    pub ready: Arc<tokio::sync::Notify>,
}
pub struct FixtureTransport {
    pub observed: Arc<Observed>,
    pub fixture: Arc<Value>,
    pub responses: Mutex<BTreeMap<String, VecDeque<Value>>>,
    pub pending: Option<PendingList>,
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
            .unwrap_or_else(|| panic!("response replay exceeded frozen request count {url}"));
        self.observed
            .responses
            .lock()
            .unwrap()
            .push(response.clone());
        response
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
        self.observed.requests.lock().unwrap().push(json!({
            "url":url,"method":request.method.as_str(),
            "referer":header(&request.headers,"Referer"),
            "authorization":header(&request.headers,"Authorization"),
            "cookie":header(&request.headers,"Cookie")
        }));
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
            json!({"access_token":format!("fixture-access-{id}"),
                "refresh_token":format!("fixture-rotated-{id}"),
                "expires_in":3600,"user":{"id":id}})
        } else if request.url.ends_with("/v1/user/illusts")
            || request.url.ends_with("/v1/user/bookmarks/illust")
        {
            let parsed = url::Url::parse(&url).unwrap();
            let key = format!("{}?{}", parsed.path(), parsed.query().unwrap());
            let body = self.fixture["list_responses"][key]["body"].clone();
            if let Some(pending) = &self.pending
                && parsed
                    .query_pairs()
                    .any(|(key, value)| key == "user_id" && value == "14")
                && parsed.query_pairs().any(|(key, value)| {
                    key == "offset" && value == "3" || key == "max_bookmark_id" && value == "90"
                })
            {
                let context = self
                    .observed
                    .request_context
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap();
                let mut reader = PendingListBody::new(
                    context,
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
                self.fixture["artwork_metadata"][id.to_string()].clone()
            } else {
                assert!(request.url.ends_with("/v1/ugoira/metadata"));
                assert_eq!(id, 71);
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
    type Body = std::io::Cursor<Vec<u8>>;
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
        let response = self.response(&request.url);
        let archive = url::Url::parse(&request.url)
            .unwrap()
            .path()
            .ends_with(".zip");
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
            std::io::Cursor::new(decode_hex(
                self.fixture[if archive { "archive_hex" } else { "static_hex" }]
                    .as_str()
                    .unwrap(),
            )),
        ))
    }
}
struct PendingListBody {
    wait: Pin<Box<dyn Future<Output = ()> + Send>>,
    ready: Arc<tokio::sync::Notify>,
    entered: bool,
    active: Arc<AtomicUsize>,
}
impl PendingListBody {
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
impl Drop for PendingListBody {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}
impl AsyncRead for PendingListBody {
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
    pub observed: Arc<Observed>,
}
impl DownloadSaveClient for FixtureSaveClient {
    fn user_artworks(
        &self,
        context: Context,
        request: UserArtworksRequest,
    ) -> Option<ArtworkPageFuture<'_>> {
        Some(Box::pin(async move {
            *self.observed.request_context.lock().unwrap() = Some(context);
            self.client.user_artworks(request).await.map_err(Into::into)
        }))
    }
    fn user_artwork_bookmarks(
        &self,
        context: Context,
        request: UserArtworkBookmarksRequest,
    ) -> Option<ArtworkPageFuture<'_>> {
        Some(Box::pin(async move {
            *self.observed.request_context.lock().unwrap() = Some(context);
            self.client
                .user_artwork_bookmarks(request)
                .await
                .map_err(Into::into)
        }))
    }
    fn artwork(&self, context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()),
            result=self.client.artwork(id)=>result.map_err(Into::into)}
        }))
    }
    fn ugoira_metadata(&self, context: Context, id: i64) -> Option<UgoiraMetadataFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()),
            result=self.client.ugoira_metadata(id)=>result.map_err(Into::into)}
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()),
            result=self.client.save_resource(reference,SaveOptions {
                path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(Into::into)}
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()),
            result=self.client.save_resource_url(&url,SaveOptions {
                path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(Into::into)}
        })
    }
}
pub fn normalize_order(requests: &mut [Value]) {
    let first_media = requests
        .iter()
        .position(|request| {
            let url = request["url"].as_str().unwrap();
            url != "https://oauth.secure.pixiv.net/auth/token" && !url.contains("/v1/user/")
        })
        .unwrap_or(requests.len());
    assert!(
        requests[first_media..]
            .iter()
            .all(|request| { !request["url"].as_str().unwrap().contains("/v1/user/") }),
        "source list began after media acquisition"
    );
    requests[first_media..].sort_by(|a, b| a["url"].as_str().cmp(&b["url"].as_str()));
}
