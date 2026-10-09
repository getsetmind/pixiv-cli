use pixiv_app::{download::DownloadSaveClient, lifecycle::Context, scheduler::SchedulerError};
use pixiv_sdk::{
    Client,
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions, SavedResource},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use serde_json::Value;
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct FixtureTransport {
    pub context: Context,
    pub requests: Arc<Mutex<Vec<Value>>>,
}
impl Transport for FixtureTransport {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(
            request.operation, "Open",
            "direct sources must not fetch artwork metadata"
        );
        assert!(
            request
                .parameters
                .iter()
                .any(|(key, value)| key == "refresh_token" && value == "fixture-refresh")
        );
        Ok(Response {
            status: 200,
            retry_after: None,
            body: serde_json::json!({"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42}}),
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
        self.requests.lock().unwrap().push(serde_json::json!({
            "url":request.url,"method":request.method,
            "referer":request.headers.get("Referer").unwrap()[0],
            "authorization":request.headers.get("Authorization").map(|v|v[0].as_str()).unwrap_or(""),
            "cookie":request.headers.get("Cookie").map(|v|v[0].as_str()).unwrap_or("")
        }));
        let url = url::Url::parse(&request.url).unwrap();
        if url.path() == "/cancel.png" {
            self.context.cancel();
            return std::future::pending().await;
        }
        let body: Self::Body = if url.path() == "/read.png" {
            Box::pin(FailedBody)
        } else {
            Box::pin(std::io::Cursor::new(b"\x89PNG\r\n\x1a\nfixture".to_vec()))
        };
        Ok(ResourceResponse::new(
            if url.path() == "/status.png" {
                503
            } else {
                200
            },
            &ResourceHeaders::from([("Content-Type".into(), vec!["image/png".into()])]),
            body,
        ))
    }
}
struct FailedBody;
impl tokio::io::AsyncRead for FailedBody {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::Error::other(
            "read failed https://i.pximg.net/read.png?signature=private",
        )))
    }
}
pub struct FixtureSaveClient(pub Arc<Client<FixtureTransport>>);
impl DownloadSaveClient for FixtureSaveClient {
    fn save_ref(
        &self,
        _: Context,
        _: ResourceRef,
        _: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(async { panic!("direct fixture must not save opaque resource") })
    }
    fn save_url(
        &self,
        context: Context,
        url: String,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(async move {
            tokio::select! {
                error=context.cancelled()=>Err(error.into()),
                result=self.0.save_resource_url(&url,SaveOptions{path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(Into::into)
            }
        })
    }
}

#[derive(Clone)]
pub struct ManualCancelTransport {
    pub inner: FixtureTransport,
    pub started: Arc<tokio::sync::Notify>,
}
impl Transport for ManualCancelTransport {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        self.inner.send(request).await
    }
}
impl ResourceTransport for ManualCancelTransport {
    type Body = Pin<Box<dyn tokio::io::AsyncRead + Send>>;
    async fn open_resource(
        &self,
        request: ResourceReadRequest,
    ) -> pixiv_sdk::Result<ResourceResponse<Self::Body>> {
        if url::Url::parse(&request.url).unwrap().path() != "/cancel.png" {
            return self.inner.open_resource(request).await;
        }
        request.validate.as_ref().unwrap()(&request.url)?;
        self.inner.requests.lock().unwrap().push(serde_json::json!({
            "url":request.url,"method":request.method,
            "referer":request.headers.get("Referer").unwrap()[0],
            "authorization":request.headers.get("Authorization").map(|v|v[0].as_str()).unwrap_or(""),
            "cookie":request.headers.get("Cookie").map(|v|v[0].as_str()).unwrap_or("")
        }));
        self.started.notify_one();
        std::future::pending().await
    }
}
pub struct ManualFixtureSaveClient(pub Arc<Client<ManualCancelTransport>>);
impl DownloadSaveClient for ManualFixtureSaveClient {
    fn save_ref(
        &self,
        _: Context,
        _: ResourceRef,
        _: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(async { panic!("direct fixture must not save opaque resource") })
    }
    fn save_url(
        &self,
        context: Context,
        url: String,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(async move {
            tokio::select! {
                biased;
                error=context.cancelled()=>Err(error.into()),
                result=self.0.save_resource_url(&url,SaveOptions{path:path.to_string_lossy().into_owned(),..Default::default()})=>result.map_err(Into::into)
            }
        })
    }
}
