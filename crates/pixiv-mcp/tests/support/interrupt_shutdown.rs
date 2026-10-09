use pixiv_app::{download::DownloadSaveClient, lifecycle::Context, scheduler::SchedulerError};
use pixiv_sdk::{
    Client,
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions, SavedResource},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

#[derive(Default)]
pub struct State {
    pub request: Mutex<Option<Context>>,
    pub started: Arc<tokio::sync::Notify>,
    pub release: Arc<tokio::sync::Notify>,
    pub events: Mutex<Vec<&'static str>>,
    pub result: Mutex<Option<serde_json::Value>>,
}
#[derive(Clone)]
pub struct SyntheticTransport(pub Arc<State>);
impl Transport for SyntheticTransport {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.operation, "Open");
        Ok(Response {
            status: 200,
            retry_after: None,
            body: serde_json::json!({"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42}}),
        })
    }
}
struct HeldBody {
    state: Arc<State>,
    release: Pin<Box<dyn Future<Output = ()> + Send>>,
    entered: bool,
    released: bool,
    body: std::io::Cursor<Vec<u8>>,
}
impl AsyncRead for HeldBody {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.entered {
            self.entered = true;
            self.state.events.lock().unwrap().push("body_waiting");
            self.state.started.notify_one();
        }
        if !self.released {
            if self.release.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            self.released = true;
        }
        Pin::new(&mut self.body).poll_read(cx, buf)
    }
}
impl Drop for HeldBody {
    fn drop(&mut self) {
        self.state.events.lock().unwrap().push("body_closed");
    }
}
impl ResourceTransport for SyntheticTransport {
    type Body = Pin<Box<dyn AsyncRead + Send>>;
    async fn open_resource(
        &self,
        request: ResourceReadRequest,
    ) -> pixiv_sdk::Result<ResourceResponse<Self::Body>> {
        request.validate.as_ref().unwrap()(&request.url)?;
        assert!(!request.headers.contains_key("Authorization"));
        assert!(!request.headers.contains_key("Cookie"));
        let bytes = b"\x89PNG\r\n\x1a\nfixture".to_vec();
        let body: Self::Body = if url::Url::parse(&request.url).unwrap().path() == "/pending.png" {
            Box::pin(HeldBody {
                state: self.0.clone(),
                release: Box::pin(self.0.release.clone().notified_owned()),
                entered: false,
                released: false,
                body: std::io::Cursor::new(bytes),
            })
        } else {
            Box::pin(std::io::Cursor::new(bytes))
        };
        Ok(ResourceResponse::new(
            200,
            &ResourceHeaders::from([("Content-Type".into(), vec!["image/png".into()])]),
            body,
        ))
    }
}
pub struct SyntheticSaveClient(pub Arc<Client<SyntheticTransport>>);
impl DownloadSaveClient for SyntheticSaveClient {
    fn save_ref(
        &self,
        _: Context,
        _: ResourceRef,
        _: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(async { panic!("direct source must not save opaque reference") })
    }
    fn save_url(
        &self,
        context: Context,
        url: String,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()),result=self.0.save_resource_url(&url,SaveOptions{path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(Into::into)}
        })
    }
}
pub fn files(path: &std::path::Path) -> serde_json::Value {
    let mut entries = std::fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    serde_json::json!(entries.iter().map(|p|serde_json::json!({"name":p.file_name().unwrap().to_str().unwrap(),"hex":std::fs::read(p).unwrap().iter().map(|b|format!("{b:02x}")).collect::<String>()})).collect::<Vec<_>>())
}
pub fn counts(path: &std::path::Path) -> (usize, usize) {
    let mut counts = (0, 0);
    for entry in std::fs::read_dir(path).unwrap() {
        if entry
            .unwrap()
            .file_name()
            .to_str()
            .unwrap()
            .starts_with(".atomic-write-")
        {
            counts.1 += 1
        } else {
            counts.0 += 1
        }
    }
    counts
}
pub fn normalize(value: &mut serde_json::Value, root: &str) {
    match value {
        serde_json::Value::String(s) => *s = s.replace(root, "<ROOT>"),
        serde_json::Value::Array(v) => v.iter_mut().for_each(|v| normalize(v, root)),
        serde_json::Value::Object(v) => v.values_mut().for_each(|v| normalize(v, root)),
        _ => {}
    }
}
