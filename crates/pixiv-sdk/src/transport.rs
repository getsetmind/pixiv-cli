use crate::error::{RetryAdvice, TransportKind};
use crate::{Error, Reason, Result};
use chrono::{
    DateTime, TimeDelta, Utc,
    format::{Parsed, StrftimeItems, parse},
};
use reqwest::{Method, redirect::Policy};
use serde_json::Value;
use std::{error::Error as StdError, fmt, time::Duration};

pub use crate::resource_transport::{
    ResourceBody, ResourceReadRequest, ResourceTransport, ResourceUrlValidator,
};

#[derive(Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub parameters: Vec<(String, String)>,
    pub operation: &'static str,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

pub struct Response {
    pub status: u16,
    pub retry_after: Option<TimeDelta>,
    pub body: Value,
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("retry_after", &self.retry_after)
            .finish_non_exhaustive()
    }
}

pub trait Transport: Send + Sync {
    fn send(&self, request: Request) -> impl std::future::Future<Output = Result<Response>> + Send;
    fn post_form(
        &self,
        request: Request,
    ) -> impl std::future::Future<Output = Result<Response>> + Send {
        async move {
            let response = self.send(request).await?;
            Ok(Response {
                body: Value::Null,
                retry_after: None,
                ..response
            })
        }
    }
}

#[derive(Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    pacing: crate::pacing::RequestPacing,
}

impl HttpTransport {
    pub fn new(proxy: Option<&str>) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .no_proxy();
        if let Some(proxy) = proxy.filter(|value| !value.is_empty()) {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy)
                    .map_err(|_| Error::new(Reason::InvalidArgument, "transport"))?,
            );
        }
        Ok(Self {
            pacing: crate::pacing::RequestPacing::new(Duration::ZERO),
            client: builder
                .build()
                .map_err(|_| Error::new(Reason::LocalStateError, "transport"))?,
        })
    }

    pub fn with_pacing(mut self, interval: Duration) -> Self {
        self.pacing = crate::pacing::RequestPacing::new(interval);
        self
    }
}

impl Transport for HttpTransport {
    async fn send(&self, request: Request) -> Result<Response> {
        self.send_request(request, true).await
    }
    async fn post_form(&self, request: Request) -> Result<Response> {
        self.send_request(request, false).await
    }
}

impl HttpTransport {
    async fn send_request(&self, request: Request, decode_json: bool) -> Result<Response> {
        let mut builder = self.client.request(request.method.clone(), &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        builder = if request.method == Method::GET {
            builder.query(&request.parameters)
        } else {
            builder.form(&request.parameters)
        };
        let original = builder
            .build()
            .map_err(|error| request_failure(&error, request.operation))?;
        let response = self.follow_redirects(original, request.operation).await?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_retry_after(value, Utc::now()));
        let bytes = response
            .bytes()
            .await
            .map_err(|error| request_failure(&error, request.operation))?;
        if !decode_json {
            return Ok(Response {
                status,
                retry_after: None,
                body: Value::Null,
            });
        }
        if !(200..300).contains(&status) {
            return Ok(Response {
                status,
                retry_after,
                body: Value::Null,
            });
        }
        let body = serde_json::from_slice(&bytes)
            .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, request.operation))?;
        Ok(Response {
            status,
            retry_after,
            body,
        })
    }

    async fn follow_redirects(
        &self,
        original: reqwest::Request,
        operation: &'static str,
    ) -> Result<reqwest::Response> {
        let failure = || classified_transport_failure("unknown", operation);
        let mut current = original.try_clone().ok_or_else(failure)?;
        let mut strip_sensitive = false;
        let mut include_body = true;
        for sent in 1..=10 {
            let previous_url = current.url().clone();
            let previous_method = current.method().clone();
            self.pacing.wait().await;
            let response = self
                .client
                .execute(current)
                .await
                .map_err(|error| request_failure(&error, operation))?;
            let status = response.status().as_u16();
            if !matches!(status, 301 | 302 | 303 | 307 | 308) {
                return Ok(response);
            }
            let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
                return Ok(response);
            };
            let location = location.to_str().map_err(|_| failure())?;
            if location.is_empty() {
                return Ok(response);
            }
            if sent == 10 || !valid_location_escapes(location) {
                return Err(failure());
            }
            let next_url = previous_url.join(location).map_err(|_| failure())?;
            strip_sensitive |= !crate::resource_transport::trusted_redirect_host(
                original.url().host_str().unwrap_or_default(),
                next_url.host_str().unwrap_or_default(),
            );
            let preserve_method = matches!(status, 307 | 308);
            include_body &= preserve_method;
            let method =
                if !preserve_method && !matches!(previous_method, Method::GET | Method::HEAD) {
                    Method::GET
                } else {
                    previous_method
                };
            let mut next = original.try_clone().ok_or_else(failure)?;
            *next.method_mut() = method;
            *next.url_mut() = next_url.clone();
            if !include_body {
                *next.body_mut() = None;
                for name in [
                    "content-encoding",
                    "content-language",
                    "content-location",
                    "content-type",
                ] {
                    next.headers_mut().remove(name);
                }
            }
            if strip_sensitive {
                for name in [
                    "authorization",
                    "www-authenticate",
                    "cookie",
                    "cookie2",
                    "proxy-authorization",
                    "proxy-authenticate",
                ] {
                    next.headers_mut().remove(name);
                }
            }
            if previous_url.scheme() == "https" && next_url.scheme() == "http" {
                next.headers_mut().remove(reqwest::header::REFERER);
            } else if !next.headers().contains_key(reqwest::header::REFERER) {
                let mut referer = previous_url;
                let _ = referer.set_username("");
                let _ = referer.set_password(None);
                next.headers_mut().insert(
                    reqwest::header::REFERER,
                    reqwest::header::HeaderValue::from_str(referer.as_str())
                        .map_err(|_| failure())?,
                );
            }
            current = next;
        }
        Err(failure())
    }
}

fn request_failure(error: &reqwest::Error, operation: &'static str) -> Error {
    let mut source: Option<&(dyn StdError + 'static)> = Some(error);
    let mut tls = false;
    let mut refused = false;
    let mut reset = false;
    let mut timeout = error.is_timeout();
    while let Some(error) = source {
        tls |= error.is::<rustls::Error>();
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            refused |= error.kind() == std::io::ErrorKind::ConnectionRefused;
            reset |= error.kind() == std::io::ErrorKind::ConnectionReset;
            timeout |= error.kind() == std::io::ErrorKind::TimedOut;
        }
        source = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .map(|inner| inner as &(dyn StdError + 'static))
            .or_else(|| error.source());
    }
    let kind = if tls {
        "tls"
    } else if refused {
        "connection_refused"
    } else if reset {
        "connection_reset"
    } else if timeout {
        "timeout"
    } else {
        "unknown"
    };
    classified_transport_failure(kind, operation)
}

fn classified_transport_failure(kind: &str, operation: &'static str) -> Error {
    Error::new(Reason::UpstreamUnavailable, operation)
        .with_transport(if kind == "tls" {
            TransportKind::Tls
        } else {
            TransportKind::Http
        })
        .with_detail(format!("transport: {kind}"))
}

fn valid_location_escapes(location: &str) -> bool {
    let bytes = location.as_bytes();
    bytes.iter().enumerate().all(|(index, byte)| {
        *byte != b'%'
            || bytes
                .get(index + 1..index + 3)
                .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
    })
}

impl ResourceTransport for HttpTransport {
    type Body = ResourceBody;

    async fn open_resource(
        &self,
        request: ResourceReadRequest,
    ) -> Result<crate::resource::ResourceResponse<Self::Body>> {
        crate::resource_transport::open(&self.client, &self.pacing, request).await
    }
}

pub(crate) fn checked(response: Response, operation: &'static str) -> Result<Value> {
    checked_status(response, operation, false)
}

pub(crate) fn checked_oauth(mut response: Response, operation: &'static str) -> Result<Value> {
    response.retry_after = None;
    checked_status(response, operation, true)
}

fn checked_status(response: Response, operation: &'static str, oauth: bool) -> Result<Value> {
    let code = match response.status {
        200..=299 => return Ok(response.body),
        400 if oauth => Reason::CredentialsExpired,
        400 => Reason::InvalidArgument,
        401 => Reason::CredentialsExpired,
        403 => Reason::Forbidden,
        404 if !oauth => Reason::NotFound,
        410 if !oauth => Reason::ContentUnavailable,
        429 => Reason::RateLimited,
        _ => Reason::UpstreamError,
    };
    let retry = matches!(response.status, 401 | 429) || (oauth && response.status == 400);
    let delay = if retry { response.retry_after } else { None };
    let after = delay.and_then(|delta| Utc::now().checked_add_signed(delta));
    Err(Error::new(code, operation)
        .with_http_status(response.status)
        .with_retry(RetryAdvice {
            safe: after.is_some(),
            after,
        }))
}

fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<TimeDelta> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return (0..=i64::MAX / 1_000_000_000)
            .contains(&seconds)
            .then(|| TimeDelta::seconds(seconds));
    }
    for format in [
        "%a, %d %b %Y %H:%M:%S GMT",
        "%A, %d-%b-%y %H:%M:%S GMT",
        "%a %b %e %H:%M:%S %Y",
    ] {
        let mut parsed = Parsed::new();
        if parse(&mut parsed, value, StrftimeItems::new(format)).is_err()
            || parsed.second == Some(60)
        {
            continue;
        }
        // Go accepts a weekday that disagrees with the calendar date.
        parsed.weekday = None;
        if let Ok(date) = parsed.to_naive_datetime_with_offset(0) {
            let duration = date.and_utc() - now;
            return Some(
                duration
                    .max(TimeDelta::zero())
                    .min(TimeDelta::nanoseconds(i64::MAX)),
            );
        }
    }
    None
}
