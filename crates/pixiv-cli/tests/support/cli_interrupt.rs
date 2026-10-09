use pixiv_app::{
    download::{DownloadSaveClient, SaveFuture},
    lifecycle::Context,
};
use pixiv_sdk::{
    Client,
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use std::{
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};
#[derive(Clone)]
pub struct Fixture {
    pub stage: String,
    pub requests: Arc<Mutex<Vec<String>>>,
    pub dropped_pending: Arc<std::sync::atomic::AtomicUsize>,
}
fn ready() {
    println!("READY");
}
pub struct Pending(pub Arc<std::sync::atomic::AtomicUsize>);
impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.operation, "Open");
        self.requests.lock().unwrap().push("oauth".into());
        if self.stage == "oauth" {
            let _pending = Pending(self.dropped_pending.clone());
            ready();
            std::future::pending().await
        } else {
            Ok(Response {
                status: 200,
                retry_after: None,
                body: serde_json::json!({"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600,"user":{"id":42,"name":"synthetic"}}),
            })
        }
    }
}
pub struct Body {
    first: bool,
    _pending: Pending,
}
impl AsyncRead for Body {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.first {
            self.first = false;
            buffer.put_slice(b"part");
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}
impl ResourceTransport for Fixture {
    type Body = Body;
    async fn open_resource(
        &self,
        request: ResourceReadRequest,
    ) -> pixiv_sdk::Result<ResourceResponse<Body>> {
        assert_eq!(
            request.url,
            "https://i.pximg.net/assets/interrupt.png?signature=synthetic"
        );
        self.requests.lock().unwrap().push("resource".into());
        if self.stage == "headers" {
            let _pending = Pending(self.dropped_pending.clone());
            ready();
            std::future::pending().await
        } else {
            ready();
            Ok(ResourceResponse::new(
                200,
                &ResourceHeaders::from([
                    ("Content-Type".into(), vec!["image/png".into()]),
                    ("Content-Length".into(), vec!["100".into()]),
                ]),
                Body {
                    first: true,
                    _pending: Pending(self.dropped_pending.clone()),
                },
            ))
        }
    }
}
pub struct SaveClient(pub Arc<Client<Fixture>>);
impl DownloadSaveClient for SaveClient {
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()), result=self.0.save_resource(reference,SaveOptions{path:path.to_string_lossy().into_owned(),progress:None})=>result.map_err(Into::into)}
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {biased; error=context.cancelled()=>Err(error.into()), result=self.0.save_resource_url(&url,SaveOptions{path:path.to_string_lossy().into_owned(),progress:None})=>result.map_err(Into::into)}
        })
    }
}

pub struct OwnedChild(pub std::process::Child);
impl std::ops::Deref for OwnedChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
