use pixiv_app::{
    download::{ArtworkFuture, DownloadSaveClient, SaveFuture},
    lifecycle::Context,
};
use pixiv_sdk::{
    Client, Error, Reason,
    error::Cause,
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
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
pub struct FixtureTransport {
    pub observed: Arc<Observed>,
    pub metadata: Value,
    pub pending: Option<Arc<tokio::sync::Notify>>,
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
                body: json!({"access_token":format!("fixture-access-{id}"),"refresh_token":format!("fixture-rotated-{id}"),"expires_in":3600,"user":{"id":id}}),
            });
        }
        assert!(matches!(request.operation, "Artwork" | "SaveResource"));
        assert!(request.url.ends_with("/v1/illust/detail"));
        let id = &request
            .parameters
            .iter()
            .find(|(key, _)| key == "illust_id")
            .unwrap()
            .1;
        Ok(Response {
            status: if id == "45" { 404 } else { 200 },
            retry_after: None,
            body: if id == "45" {
                json!({"error":{"message":"fixture missing artwork"}})
            } else {
                self.metadata[id].clone()
            },
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
        self.observed.requests.lock().unwrap().push(json!({"url":request.url,"method":request.method,"referer":request.headers.get("Referer").unwrap()[0],"authorization":request.headers.get("Authorization").map(|v|v[0].as_str()).unwrap_or(""),"cookie":request.headers.get("Cookie").map(|v|v[0].as_str()).unwrap_or("")}));
        let path = url::Url::parse(&request.url).unwrap().path().to_owned();
        if path.contains("47_p1") {
            return Err(Error::new(Reason::UpstreamUnavailable, "OpenResource")
                .with_cause(Cause::TransportFailure(Box::new(Cause::Canceled))));
        }
        let body: Self::Body = if let (true, Some(ready)) = (path.contains("44_p1"), &self.pending)
        {
            Box::pin(PendingBody {
                ready: ready.clone(),
                notified: false,
            })
        } else {
            Box::pin(std::io::Cursor::new(b"\x89PNG\r\n\x1a\nfixture".to_vec()))
        };
        Ok(ResourceResponse::new(
            if path.contains("46_p1") { 503 } else { 200 },
            &ResourceHeaders::from([("Content-Type".into(), vec!["image/png".into()])]),
            body,
        ))
    }
}
struct PendingBody {
    ready: Arc<tokio::sync::Notify>,
    notified: bool,
}
impl tokio::io::AsyncRead for PendingBody {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut PollContext<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.notified {
            self.notified = true;
            self.ready.notify_one();
        }
        Poll::Pending
    }
}
pub struct FixtureSaveClient(pub Arc<Client<FixtureTransport>>);
impl DownloadSaveClient for FixtureSaveClient {
    fn artwork(&self, context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {biased;error=context.cancelled()=>Err(error.into()),result=self.0.artwork(id)=>result.map_err(Into::into)}
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased;error=context.cancelled()=>Err(error.into()),result=self.0.save_resource(reference,SaveOptions{path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(Into::into)}
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased;error=context.cancelled()=>Err(error.into()),result=self.0.save_resource_url(&url,SaveOptions{path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(Into::into)}
        })
    }
}
pub fn normalize(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace(root, "<ROOT>"),
        Value::Array(values) => values.iter_mut().for_each(|v| normalize(v, root)),
        Value::Object(values) => values.values_mut().for_each(|v| normalize(v, root)),
        _ => {}
    }
}
pub fn files(root: &std::path::Path) -> (Value, Value) {
    fn visit(
        root: &std::path::Path,
        path: &std::path::Path,
        files: &mut Vec<Value>,
        directories: &mut Vec<Value>,
    ) {
        let mut entries = std::fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().path())
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
                visit(root, &path, files, directories)
            } else {
                files.push(json!({"name":name,"hex":std::fs::read(path).unwrap().iter().map(|byte|format!("{byte:02x}")).collect::<String>()}));
            }
        }
    }
    let mut files = Vec::new();
    let mut directories = Vec::new();
    visit(root, root, &mut files, &mut directories);
    (json!(files), json!(directories))
}
