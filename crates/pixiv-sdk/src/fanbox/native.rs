use super::transport::{
    BodyFuture, ExternalError, Headers, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
    TransportFuture,
};
use crate::context::RequestContext;
use futures_util::{Stream, StreamExt};
use std::{fmt, io, pin::Pin, sync::Arc};
use wreq::{
    header::{HeaderMap, HeaderName, HeaderValue, OrigHeaderMap},
    tls::trust::CertStore,
};

#[path = "profile.rs"]
mod profile;

pub use wreq::tls::{CertificateFailureKind, CertificateVerificationError};

#[derive(Clone)]
pub struct NativeTransport {
    client: wreq::Client,
}

impl fmt::Debug for NativeTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeTransport")
            .finish_non_exhaustive()
    }
}

impl NativeTransport {
    pub fn new(proxy_url: &str) -> Result<Self, ExternalError> {
        let roots = CertStore::builder().set_default_paths().build()?;
        let mut builder = wreq::Client::builder()
            .tls_options(profile::tls_options())
            .http2_options(profile::http2_options())
            .tls_cert_store(roots)
            .tls_cert_verification(true)
            .tls_verify_hostname(true)
            .pool_idle_timeout(None)
            .redirect(wreq::redirect::Policy::none())
            .retry(wreq::retry::Policy::never())
            .no_proxy();
        if !proxy_url.is_empty() {
            builder = builder.proxy(wreq::Proxy::all(proxy_url)?);
        }
        Ok(Self {
            client: builder.build()?,
        })
    }

    pub async fn close_idle_connections(&self) -> Result<(), ExternalError> {
        self.client
            .close_idle_connections()
            .await
            .map_err(Into::into)
    }

    async fn execute(&self, raw: RawRequest) -> Result<Option<RawResponse>, ExternalError> {
        let mut headers = HeaderMap::new();
        for (name, values) in raw.headers {
            let name = HeaderName::from_bytes(name.as_bytes())?;
            for value in values {
                headers.append(name.clone(), HeaderValue::from_str(&value)?);
            }
        }
        if let Some(host) = raw.logical_host {
            headers.insert(wreq::header::HOST, HeaderValue::from_str(&host)?);
        }
        if !headers.contains_key(wreq::header::ACCEPT_ENCODING) {
            headers.insert(
                wreq::header::ACCEPT_ENCODING,
                HeaderValue::from_static("gzip, deflate, br"),
            );
        }
        if !headers.contains_key(wreq::header::USER_AGENT) {
            headers.insert(
                wreq::header::USER_AGENT,
                HeaderValue::from_static("Go-http-client/2.0"),
            );
        }
        let mut order = OrigHeaderMap::new();
        let browser_headers = ["accept", "cookie", "origin", "referer"];
        if browser_headers
            .iter()
            .any(|name| headers.contains_key(*name))
        {
            for name in [
                "accept",
                "cookie",
                "origin",
                "referer",
                "user-agent",
                "accept-encoding",
            ] {
                order.insert(name);
            }
        } else {
            order.insert("accept-encoding");
            order.insert("user-agent");
        }
        for name in headers.keys() {
            order.insert(name.clone());
        }
        let mut request = self
            .client
            .request(wreq::Method::from_bytes(raw.method.as_bytes())?, &raw.url)
            .headers(headers)
            .orig_headers(order);
        if let Some(body) = raw.body {
            if raw.content_length < 0 {
                request = request.body(wreq::Body::wrap_stream(futures_util::stream::once(
                    async move { Ok::<_, io::Error>(body) },
                )));
            } else {
                request = request
                    .body(body)
                    .header(wreq::header::CONTENT_LENGTH, raw.content_length);
            }
        } else if raw.content_length > 0 {
            request = request.header(wreq::header::CONTENT_LENGTH, raw.content_length);
        }
        let context = raw.context;
        let mut request = request.build()?;
        request.extensions_mut().insert(context.clone());
        let response = tokio::select! {
            biased;
            error = context.cancelled() => return Err(Box::new(error)),
            response = self.client.execute(request) => response?,
        };
        let status = response.status().as_u16();
        let content_length = response
            .content_length()
            .and_then(|value| i64::try_from(value).ok())
            .unwrap_or(-1);
        let mut headers = Headers::new();
        for (name, value) in response.headers() {
            headers
                .entry(name.to_string())
                .or_default()
                .push(value.to_str()?.into());
        }
        let stream = response
            .bytes_stream()
            .map(|chunk| chunk.map(|data| data.to_vec()));
        Ok(Some(RawResponse {
            status,
            headers,
            content_length,
            body: Some(Box::new(NativeBody {
                stream: Some(Box::pin(stream)),
                buffer: vec![],
                offset: 0,
                context,
            })),
        }))
    }
}

impl RawTransport for NativeTransport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(self.execute(request))
    }
    fn close_idle_connections(&self) {
        drop(self.client.close_idle_connections());
    }
}

type BodyStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, wreq::Error>> + Send>>;
struct NativeBody {
    stream: Option<BodyStream>,
    buffer: Vec<u8>,
    offset: usize,
    context: Arc<dyn RequestContext>,
}
impl RawBody for NativeBody {
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
                let Some(stream) = &mut self.stream else {
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
        self.buffer.clear();
        self.offset = 0;
        Box::pin(async { Ok(()) })
    }
}
