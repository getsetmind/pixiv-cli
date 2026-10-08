use crate::error::RetryAdvice;
use crate::{Error, Reason, Result};
use chrono::{
    DateTime, TimeDelta, Utc,
    format::{Parsed, StrftimeItems, parse},
};
use reqwest::{Method, redirect::Policy};
use serde_json::Value;
use std::{fmt, time::Duration};

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
        self.pacing.wait().await;
        let response = builder
            .send()
            .await
            .map_err(|_| Error::new(Reason::UpstreamUnavailable, request.operation))?;
        let status = response.status().as_u16();
        if !decode_json {
            response
                .bytes()
                .await
                .map_err(|_| Error::new(Reason::UpstreamUnavailable, request.operation))?;
            return Ok(Response {
                status,
                retry_after: None,
                body: Value::Null,
            });
        }
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_retry_after(value, Utc::now()));
        if !(200..300).contains(&status) {
            return Ok(Response {
                status,
                retry_after,
                body: Value::Null,
            });
        }
        let body = response
            .json()
            .await
            .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, request.operation))?;
        Ok(Response {
            status,
            retry_after,
            body,
        })
    }
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
