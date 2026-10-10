use super::{CallerContext, ReverseFuture};
use futures_util::{Stream, StreamExt};
use pixiv_sdk::fanbox::transport::{
    BodyFuture, ExternalError, Headers, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
    TransportFuture,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::{
    fmt,
    io::{self, Cursor, Read},
    pin::Pin,
    sync::{Arc, Mutex},
};

const READ_CHUNK_BYTES: usize = 32 * 1024;

/// A nonempty reader with length zero has Go's unknown-length body semantics.
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub logical_host: Option<String>,
    pub headers: Headers,
    pub body: Option<Box<dyn Read + Send>>,
    pub content_length: i64,
    pub context: CallerContext,
}
impl fmt::Debug for HttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpRequest")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}
pub trait HttpTransport: Send + Sync {
    fn send(
        &self,
        request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>>;
    fn close_idle_connections(&self) {}
}

/// Retiring the owned pool allows active client clones to finish on the old pool.
/// A blocking caller-provided `Read` cannot be interrupted while it is in progress.
pub struct ReqwestTransport {
    proxy: String,
    client: Mutex<Option<reqwest::Client>>,
}
impl fmt::Debug for ReqwestTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReqwestTransport")
            .finish_non_exhaustive()
    }
}
impl ReqwestTransport {
    pub fn new(proxy: &str) -> Result<Self, ExternalError> {
        let client = ordinary_client(proxy)?;
        Ok(Self {
            proxy: proxy.into(),
            client: Mutex::new(Some(client)),
        })
    }
    fn client(&self) -> Result<reqwest::Client, ExternalError> {
        let mut stored = self
            .client
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if stored.is_none() {
            *stored = Some(ordinary_client(&self.proxy)?);
        }
        Ok(stored.as_ref().expect("initialized HTTP client").clone())
    }
    fn retire_pool(&self) {
        let retired = self
            .client
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        drop(retired);
    }
    async fn execute(&self, raw: HttpRequest) -> Result<Option<RawResponse>, ExternalError> {
        let client = self.client()?;
        let mut headers = HeaderMap::new();
        for (name, values) in raw.headers {
            let name = HeaderName::from_bytes(name.as_bytes())?;
            for value in values {
                headers.append(name.clone(), HeaderValue::from_str(&value)?);
            }
        }
        if let Some(host) = raw.logical_host {
            headers.insert(reqwest::header::HOST, HeaderValue::from_str(&host)?);
        }
        let context = raw.context;
        let method = reqwest::Method::from_bytes(raw.method.as_bytes())?;
        let mut builder = client.request(method, &raw.url).headers(headers);
        if let Some(reader) = raw.body {
            let state = RequestBodyState {
                reader,
                context: context.clone(),
            };
            let stream = futures_util::stream::try_unfold(state, |state| async move {
                let (reader, data) = read_chunk(state.reader, state.context.clone()).await?;
                if data.is_empty() {
                    Ok(None)
                } else {
                    Ok::<_, io::Error>(Some((
                        data,
                        RequestBodyState {
                            reader,
                            context: state.context,
                        },
                    )))
                }
            });
            builder = builder.body(reqwest::Body::wrap_stream(stream));
            if raw.content_length > 0 {
                builder = builder.header(reqwest::header::CONTENT_LENGTH, raw.content_length);
            }
        } else if raw.content_length > 0 {
            builder = builder.header(reqwest::header::CONTENT_LENGTH, raw.content_length);
        }
        let request = builder.build()?;
        let mut request: hyper::http::Request<reqwest::Body> = request.try_into()?;
        request.extensions_mut().insert(context.clone());
        let request = reqwest::Request::try_from(request)?;
        let response = tokio::select! {
            biased;
            error = context.cancelled() => return Err(Box::new(error)),
            response = client.execute(request) => response?,
        };
        let status = response.status().as_u16();
        let content_length = response
            .content_length()
            .and_then(|length| i64::try_from(length).ok())
            .unwrap_or(-1);
        let mut headers = Headers::new();
        for (name, value) in response.headers() {
            headers
                .entry(canonical_header(name.as_str()))
                .or_default()
                .push(value.to_str()?.into());
        }
        Ok(Some(RawResponse {
            status,
            headers,
            content_length,
            body: Some(Box::new(ResponseBody {
                stream: Some(Box::pin(response.bytes_stream())),
                buffer: bytes::Bytes::new(),
                offset: 0,
                context,
            })),
        }))
    }
}
impl HttpTransport for ReqwestTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(self.execute(request))
    }
    fn close_idle_connections(&self) {
        self.retire_pool();
    }
}
impl RawTransport for ReqwestTransport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(
            self.execute(HttpRequest {
                method: request.method,
                url: request.url,
                logical_host: request.logical_host,
                headers: request.headers,
                body: request
                    .body
                    .map(|body| Box::new(Cursor::new(body)) as Box<dyn Read + Send>),
                content_length: request.content_length,
                context: request.context,
            }),
        )
    }
    fn close_idle_connections(&self) {
        self.retire_pool();
    }
}

fn ordinary_client(proxy: &str) -> Result<reqwest::Client, ExternalError> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .user_agent("Go-http-client/1.1");
    if !proxy.is_empty() {
        let url = url::Url::parse(proxy)?;
        if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
            || url.host_str().is_none_or(str::is_empty)
        {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "proxy URL must use http, https, socks5, or socks5h",
            )));
        }
        let proxy = pixiv_sdk::environment_proxy::reqwest_proxy(url.into()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "proxy URL scheme is invalid")
        })?;
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    Ok(builder.build()?)
}
struct RequestBodyState {
    reader: Box<dyn Read + Send>,
    context: CallerContext,
}
async fn read_chunk(
    mut reader: Box<dyn Read + Send>,
    context: CallerContext,
) -> io::Result<(Box<dyn Read + Send>, Vec<u8>)> {
    if let Some(error) = context.error() {
        return Err(io::Error::other(error));
    }
    let read = tokio::task::spawn_blocking(move || {
        let mut buffer = vec![0; READ_CHUNK_BYTES];
        let result = loop {
            match reader.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => break result,
            }
        };
        (reader, buffer, result)
    });
    let (reader, mut buffer, result) = tokio::select! {
        biased;
        error = context.cancelled() => return Err(io::Error::other(error)),
        result = read => result.map_err(io::Error::other)?,
    };
    buffer.truncate(result?);
    Ok((reader, buffer))
}

type ResponseStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>;
struct ResponseBody {
    stream: Option<ResponseStream>,
    buffer: bytes::Bytes,
    offset: usize,
    context: CallerContext,
}
impl RawBody for ResponseBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            if output.is_empty() {
                return RawRead {
                    count: 0,
                    eof: false,
                    error: None,
                };
            }
            loop {
                if self.offset < self.buffer.len() {
                    let count = output.len().min(self.buffer.len() - self.offset);
                    output[..count].copy_from_slice(&self.buffer[self.offset..self.offset + count]);
                    self.offset += count;
                    return RawRead {
                        count,
                        eof: false,
                        error: None,
                    };
                }
                let Some(stream) = self.stream.as_mut() else {
                    return RawRead {
                        count: 0,
                        eof: true,
                        error: None,
                    };
                };
                let next = tokio::select! {
                    biased;
                    error = self.context.cancelled() => return RawRead { count: 0, eof: false, error: Some(Box::new(error)) },
                    next = stream.next() => next,
                };
                match next {
                    Some(Ok(data)) => {
                        self.buffer = data;
                        self.offset = 0;
                    }
                    Some(Err(error)) => {
                        return RawRead {
                            count: 0,
                            eof: false,
                            error: Some(Box::new(error)),
                        };
                    }
                    None => {
                        self.stream = None;
                        return RawRead {
                            count: 0,
                            eof: true,
                            error: None,
                        };
                    }
                }
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        self.stream = None;
        self.buffer = bytes::Bytes::new();
        self.offset = 0;
        Box::pin(async { Ok(()) })
    }
}

/// The SDK Vec-backed native port buffers the complete multipart body.
/// ASCII2D's image limit does not cap authenticity tokens or multipart framing.
/// Go's zero length with a reader maps to the raw port's unknown length (-1).
pub struct NativeHttpTransport {
    transport: Arc<dyn RawTransport>,
}
impl NativeHttpTransport {
    pub fn new(transport: Arc<dyn RawTransport>) -> Self {
        Self { transport }
    }
    async fn execute(&self, request: HttpRequest) -> Result<Option<RawResponse>, ExternalError> {
        let body = if let Some(mut reader) = request.body {
            let mut body = Vec::new();
            loop {
                let (next_reader, chunk) = read_chunk(reader, request.context.clone()).await?;
                reader = next_reader;
                if chunk.is_empty() {
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            Some(body)
        } else {
            None
        };
        let content_length = if body.is_some() && request.content_length == 0 {
            -1
        } else {
            request.content_length
        };
        self.transport
            .send(RawRequest {
                method: request.method,
                url: request.url,
                logical_host: request.logical_host,
                headers: request.headers,
                body,
                content_length,
                context: request.context,
            })
            .await
    }
}
impl HttpTransport for NativeHttpTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(self.execute(request))
    }
    fn close_idle_connections(&self) {
        self.transport.close_idle_connections();
    }
}
fn canonical_header(name: &str) -> String {
    let mut capital = true;
    name.chars()
        .map(|character| {
            let result = if capital {
                character.to_ascii_uppercase()
            } else {
                character.to_ascii_lowercase()
            };
            capital = character == '-';
            result
        })
        .collect()
}
