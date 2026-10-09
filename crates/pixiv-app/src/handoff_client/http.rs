use super::{
    HandoffBody, HandoffFuture, HandoffRead, HandoffRequest, HandoffResponse, HandoffTransport,
    HandoffTransportError,
};
use futures_util::{StreamExt, future::poll_fn};
use std::{collections::BTreeMap, io, pin::Pin, sync::Mutex};
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::{io::StreamReader, sync::CancellationToken};

pub use pixiv_sdk::environment_proxy::{
    ProxyEnvironment as HandoffProxyEnvironment, ProxyError as HandoffProxyError,
};

#[derive(Default)]
pub struct NativeHandoffTransport {
    proxy_environment: Option<HandoffProxyEnvironment>,
    clients: Mutex<BTreeMap<Option<String>, reqwest::Client>>,
}
impl NativeHandoffTransport {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_proxy_environment(environment: HandoffProxyEnvironment) -> Self {
        Self {
            proxy_environment: Some(environment),
            clients: Mutex::new(BTreeMap::new()),
        }
    }
}
impl HandoffTransport for NativeHandoffTransport {
    fn send<'a>(
        &'a self,
        request: HandoffRequest,
        cancellation: CancellationToken,
    ) -> HandoffFuture<'a, Result<HandoffResponse, HandoffTransportError>> {
        Box::pin(async move {
            let environment = self
                .proxy_environment
                .as_ref()
                .unwrap_or_else(|| HandoffProxyEnvironment::cached());
            let proxy = environment
                .proxy_for_url(&request.endpoint)
                .map_err(|_| HandoffTransportError)?;
            let client = {
                let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(client) = clients.get(&proxy) {
                    client.clone()
                } else {
                    let mut builder = reqwest::Client::builder()
                        .redirect(reqwest::redirect::Policy::none())
                        .no_proxy()
                        .user_agent("Go-http-client/1.1");
                    if let Some(proxy) = &proxy {
                        let proxy = pixiv_sdk::environment_proxy::reqwest_proxy(proxy.clone())
                            .ok_or(HandoffTransportError)?;
                        builder = builder
                            .proxy(reqwest::Proxy::all(proxy).map_err(|_| HandoffTransportError)?);
                    }
                    let client = builder.build().map_err(|_| HandoffTransportError)?;
                    clients.insert(proxy, client.clone());
                    client
                }
            };
            let method = reqwest::Method::from_bytes(request.method.as_bytes())
                .map_err(|_| HandoffTransportError)?;
            let mut request_builder = client.request(method, &request.endpoint).body(request.body);
            for (name, value) in request.headers {
                request_builder = request_builder.header(name, value);
            }
            let response = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(HandoffTransportError),
                result = request_builder.send() => result.map_err(|_| HandoffTransportError)?,
            };
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.to_string(),
                        crate::auth_bundle::go_utf8(value.as_bytes()),
                    )
                })
                .collect();
            let reader = StreamReader::new(
                response
                    .bytes_stream()
                    .map(|result| result.map_err(io::Error::other)),
            );
            Ok(HandoffResponse {
                status,
                headers,
                body: Box::new(NativeBody {
                    reader: Mutex::new(Some(Box::pin(reader))),
                    closed: CancellationToken::new(),
                    cancellation,
                }),
            })
        })
    }
}
struct NativeBody {
    reader: Mutex<Option<Pin<Box<dyn AsyncRead + Send>>>>,
    closed: CancellationToken,
    cancellation: CancellationToken,
}
impl HandoffBody for NativeBody {
    fn read<'a>(&'a self, buffer: &'a mut [u8]) -> HandoffFuture<'a, HandoffRead> {
        Box::pin(async move {
            let result = tokio::select! {
                biased;
                _ = self.closed.cancelled() => Err(io::Error::other("response body closed")),
                _ = self.cancellation.cancelled() => Err(io::Error::other("context canceled")),
                result = poll_fn(|cx| {
                    let mut reader = self.reader.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(reader) = reader.as_mut() else { return std::task::Poll::Ready(Err(io::Error::other("response body closed"))); };
                    let mut read_buffer = ReadBuf::new(buffer);
                    match reader.as_mut().poll_read(cx, &mut read_buffer) {
                        std::task::Poll::Ready(Ok(())) => std::task::Poll::Ready(Ok(read_buffer.filled().len())),
                        std::task::Poll::Ready(Err(error)) => std::task::Poll::Ready(Err(error)),
                        std::task::Poll::Pending => std::task::Poll::Pending,
                    }
                }) => result,
            };
            match result {
                Ok(count) => HandoffRead { count, error: None },
                Err(error) => HandoffRead {
                    count: 0,
                    error: Some(error),
                },
            }
        })
    }
    fn close(&self) {
        self.reader.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.closed.cancel();
    }
}
