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

#[derive(Clone)]
pub struct Fixture {
    pub failure: String,
    pub context: Context,
    pub requests: Arc<Mutex<Vec<serde_json::Value>>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: serde_json::json!({"access_token":"fixture-token","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42}}),
            });
        }
        let mut url = url::Url::parse(&request.url).unwrap();
        if !request.parameters.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(request.parameters.iter().map(|(key, value)| (key, value)));
        }
        let headers: std::collections::BTreeMap<_, _> = request.headers.iter().cloned().collect();
        self.requests.lock().unwrap().push(serde_json::json!({"method":request.method.as_str(),"url":url.as_str(),"referer":headers.get("Referer").map(String::as_str).unwrap_or(""),"authorization":headers.get("Authorization").map(String::as_str).unwrap_or(""),"cookie":""}));
        Ok(Response {
            status: 200,
            retry_after: None,
            body: serde_json::json!({"ugoira_metadata":{"zip_urls":{"original":"https://i.pximg.net/archive.zip"},"frames":[{"file":"0.jpg","delay":10}]}}),
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
        self.requests.lock().unwrap().push(serde_json::json!({"method":request.method.as_str(),"url":request.url,"referer":request.headers.get("Referer").and_then(|v|v.first()).map(String::as_str).unwrap_or(""),"authorization":"","cookie":""}));
        let failed = request.url == "https://i.pximg.net/assets/two.png";
        let status = if failed && self.failure == "status" {
            404
        } else {
            200
        };
        Ok(ResourceResponse::new(
            status,
            &ResourceHeaders::from([
                ("Content-Type".into(), vec!["image/png".into()]),
                ("Content-Length".into(), vec!["7".into()]),
            ]),
            Body {
                bytes: b"payload".to_vec(),
                failure: if failed {
                    self.failure.clone()
                } else {
                    String::new()
                },
                context: self.context.clone(),
            },
        ))
    }
}
pub struct SaveClient(pub Arc<Client<Fixture>>);
impl DownloadSaveClient for SaveClient {
    fn save_ref(
        &self,
        _context: Context,
        reference: ResourceRef,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(async move {
            self.0
                .save_resource(
                    reference,
                    SaveOptions {
                        path: path.to_string_lossy().into_owned(),
                        progress: None,
                    },
                )
                .await
                .map_err(Into::into)
        })
    }
    fn save_url(
        &self,
        _context: Context,
        url: String,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(async move {
            self.0
                .save_resource_url(
                    &url,
                    SaveOptions {
                        path: path.to_string_lossy().into_owned(),
                        progress: None,
                    },
                )
                .await
                .map_err(Into::into)
        })
    }
}
pub struct FailWriter;
impl std::io::Write for FailWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("fixture writer failure"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct PublicationClient {
    pub inner: SaveClient,
    pub published: Arc<std::sync::atomic::AtomicBool>,
}
impl DownloadSaveClient for PublicationClient {
    fn save_ref(
        &self,
        context: Context,
        reference: ResourceRef,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(async move {
            let result = self.inner.save_ref(context, reference, path).await;
            if result.is_ok() {
                self.published
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            result
        })
    }
    fn save_url(
        &self,
        context: Context,
        url: String,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(async move {
            let result = self.inner.save_url(context, url, path).await;
            if result.is_ok() {
                self.published
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            result
        })
    }
}
pub struct ObservedWriter {
    pub bytes: Arc<Mutex<Vec<u8>>>,
    pub fail: bool,
}
impl std::io::Write for ObservedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
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
