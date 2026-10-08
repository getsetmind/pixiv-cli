use crate::{
    Error, Reason, Result,
    error::TransportKind,
    resource::{ResourceHeaders, ResourceResponse},
};
use futures_util::TryStreamExt;
use reqwest::{
    Method,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use std::{fmt, pin::Pin, sync::Arc};
use tokio::io::AsyncRead;
use tokio_util::io::StreamReader;

pub type ResourceBody = Pin<Box<dyn AsyncRead + Send>>;
pub type ResourceUrlValidator = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;

pub struct ResourceReadRequest {
    pub url: String,
    pub method: String,
    pub headers: ResourceHeaders,
    pub operation: &'static str,
    pub validate: Option<ResourceUrlValidator>,
}

impl fmt::Debug for ResourceReadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResourceReadRequest")
            .field("method", &self.method)
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

#[allow(async_fn_in_trait)]
pub trait ResourceTransport: Send + Sync {
    type Body: AsyncRead + Unpin + Send;

    async fn open_resource(
        &self,
        request: ResourceReadRequest,
    ) -> Result<ResourceResponse<Self::Body>>;
}

pub(crate) async fn open(
    client: &reqwest::Client,
    pacing: &crate::pacing::RequestPacing,
    request: ResourceReadRequest,
) -> Result<ResourceResponse<ResourceBody>> {
    let failure = || {
        Error::new(Reason::UpstreamUnavailable, request.operation)
            .with_transport(TransportKind::Http)
            .with_detail("transport: unknown")
    };
    if let Some(validate) = &request.validate {
        validate(&request.url)?;
    }
    let method = match request.method.as_str() {
        "" | "GET" => Method::GET,
        "HEAD" => Method::HEAD,
        _ => return Err(failure()),
    };
    let mut url = reqwest::Url::parse(&request.url).map_err(|_| failure())?;
    let initial_host = url.host_str().unwrap_or_default().to_owned();
    let mut headers = HeaderMap::new();
    for (name, values) in &request.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| failure())?;
        for value in values {
            headers.append(
                name.clone(),
                HeaderValue::from_str(value).map_err(|_| failure())?,
            );
        }
    }
    headers.insert(
        "referer",
        HeaderValue::from_static("https://app-api.pixiv.net/"),
    );
    headers.insert(
        "user-agent",
        HeaderValue::from_static("PixivAndroidApp/5.0.234 (Android 11; Pixel 5)"),
    );
    for sent in 1..=10 {
        pacing.wait().await;
        let response = client
            .request(method.clone(), url.clone())
            .headers(headers.clone())
            .send()
            .await
            .map_err(|_| failure())?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308)
            && let Some(location) = response.headers().get("location")
            && !location.as_bytes().is_empty()
        {
            if sent == 10 {
                return Err(failure());
            }
            let location = location.to_str().map_err(|_| failure())?;
            let next = url.join(location).map_err(|_| failure())?;
            if !trusted_redirect_host(&initial_host, next.host_str().unwrap_or_default()) {
                for name in ["authorization", "www-authenticate", "cookie", "cookie2"] {
                    headers.remove(name);
                }
            }
            if let Some(validate) = &request.validate {
                validate(next.as_str()).map_err(|_| failure())?;
            }
            url = next;
            continue;
        }
        let mut response_headers = ResourceHeaders::new();
        for name in [
            "Content-Type",
            "Content-Length",
            "Content-Range",
            "Accept-Ranges",
            "Etag",
            "Last-Modified",
            "Cache-Control",
        ] {
            let values: Vec<_> = response
                .headers()
                .get_all(name)
                .iter()
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
                .collect();
            if !values.is_empty() {
                response_headers.insert(name.to_owned(), values);
            }
        }
        let stream = response
            .bytes_stream()
            .map_err(|_| std::io::Error::other("cannot read resource"));
        let body: ResourceBody = Box::pin(StreamReader::new(stream));
        return Ok(ResourceResponse::new(
            i64::from(status),
            &response_headers,
            body,
        ));
    }
    Err(failure())
}

fn trusted_redirect_host(initial: &str, destination: &str) -> bool {
    let initial = initial.to_ascii_lowercase();
    let destination = destination.to_ascii_lowercase();
    destination == initial
        || (!destination.contains([':', '%'])
            && destination
                .strip_suffix(&initial)
                .is_some_and(|prefix| prefix.ends_with('.')))
}
