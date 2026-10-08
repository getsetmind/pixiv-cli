use crate::error::RetryAdvice;
use crate::{Error, Reason, Result};
use chrono::{TimeDelta, Utc};
use reqwest::{Method, redirect::Policy};
use serde_json::Value;
use std::{fmt, time::Duration};

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
    pub retry_after_seconds: Option<u64>,
    pub body: Value,
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("retry_after_seconds", &self.retry_after_seconds)
            .finish_non_exhaustive()
    }
}

#[allow(async_fn_in_trait)]
pub trait Transport: Send + Sync {
    async fn send(&self, request: Request) -> Result<Response>;
}

#[derive(Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
}

impl HttpTransport {
    pub fn new(proxy: Option<&str>) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(Policy::none())
            .no_proxy();
        if let Some(proxy) = proxy.filter(|value| !value.is_empty()) {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy)
                    .map_err(|_| Error::new(Reason::InvalidArgument, "transport"))?,
            );
        }
        Ok(Self {
            client: builder
                .build()
                .map_err(|_| Error::new(Reason::LocalStateError, "transport"))?,
        })
    }
}

impl Transport for HttpTransport {
    async fn send(&self, request: Request) -> Result<Response> {
        let mut builder = self.client.request(request.method.clone(), &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        builder = if request.method == Method::GET {
            builder.query(&request.parameters)
        } else {
            builder.form(&request.parameters)
        };
        let response = builder
            .send()
            .await
            .map_err(|_| Error::new(Reason::UpstreamUnavailable, request.operation))?;
        let status = response.status().as_u16();
        let retry_after_seconds = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        if !(200..300).contains(&status) {
            return Ok(Response {
                status,
                retry_after_seconds,
                body: Value::Null,
            });
        }
        let body = response
            .json()
            .await
            .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, request.operation))?;
        Ok(Response {
            status,
            retry_after_seconds,
            body,
        })
    }
}

pub(crate) fn checked(response: Response, operation: &'static str) -> Result<Value> {
    checked_status(response, operation, false)
}

pub(crate) fn checked_oauth(response: Response, operation: &'static str) -> Result<Value> {
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
    let seconds = if retry {
        response.retry_after_seconds
    } else {
        None
    };
    let after = seconds
        .and_then(|seconds| i64::try_from(seconds).ok())
        .and_then(TimeDelta::try_seconds)
        .and_then(|delta| Utc::now().checked_add_signed(delta));
    Err(Error::new(code, operation)
        .with_http_status(response.status)
        .with_retry(RetryAdvice {
            safe: after.is_some(),
            after,
        }))
}
