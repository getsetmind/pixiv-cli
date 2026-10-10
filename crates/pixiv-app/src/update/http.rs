use super::{ExternalError, UpdateFuture, go_quote, message, wrap};
use crate::reverse_search::http::{HttpRequest, HttpTransport, ReqwestTransport};
use pixiv_sdk::fanbox::transport::{BodyFuture, Headers, RawBody, RawRead, RawResponse};
use std::sync::{Arc, Mutex};

pub struct ClientTransport {
    transport: Arc<dyn HttpTransport>,
}
impl ClientTransport {
    pub fn new(transport: Arc<dyn HttpTransport>) -> Self {
        Self { transport }
    }
    async fn execute(
        &self,
        mut request: HttpRequest,
    ) -> Result<Option<RawResponse>, ExternalError> {
        let original_headers = request.headers.clone();
        let original_url = request.url.clone();
        let original_method = request.method.clone();
        let operation = operation(&original_method);
        let original_has_body = request.body.is_some() || request.content_length > 0;
        let context = request.context.clone();
        let mut sensitive_stripped = false;
        let mut hops = 0usize;
        loop {
            let current_url = request.url.clone();
            let current_method = request.method.clone();
            let host = request.logical_host.clone();
            let sent = self.transport.send(request).await.map_err(|error| {
                wrap(
                    format!("{operation} {}", go_quote(&strip_password(&current_url))),
                    error,
                )
            })?;
            let Some(mut response) = sent else {
                return Err(wrap(
                    format!("{operation} {}", go_quote(&strip_password(&current_url))),
                    message(
                        "http: RoundTripper implementation (HttpTransport) returned a nil *Response with a nil error",
                    ),
                ));
            };
            if response.body.is_none() {
                if response.content_length > 0 && current_method != "HEAD" {
                    return Err(wrap(
                        format!("{operation} {}", go_quote(&strip_password(&current_url))),
                        message(format!(
                            "http: RoundTripper implementation (HttpTransport) returned a *Response with content length {} but a nil Body",
                            response.content_length
                        )),
                    ));
                }
                response.body = Some(Box::new(EmptyBody));
            }
            hops += 1;
            let redirect = matches!(response.status, 301 | 302 | 303 | 307 | 308);
            let location = header(&response.headers, "Location").to_owned();
            if !redirect
                || location.is_empty()
                || (matches!(response.status, 307 | 308) && original_has_body)
            {
                return Ok(Some(response));
            }
            let base = match url::Url::parse(&current_url) {
                Ok(base) => base,
                Err(error) => {
                    close_redirect_body(&mut response, false).await;
                    return Err(wrap(
                        format!("{operation} {}", go_quote(&current_url)),
                        Box::new(error),
                    ));
                }
            };
            let (before_fragment, fragment) = location.split_once('#').unwrap_or((&location, ""));
            let path = before_fragment
                .split_once('?')
                .map_or(before_fragment, |(path, _)| path);
            if let Err(error) = super::release::validate_escapes(path)
                .and_then(|()| super::release::validate_escapes(fragment))
            {
                close_redirect_body(&mut response, false).await;
                return Err(wrap(
                    format!("{operation} {}", go_quote(&current_url)),
                    message(format!(
                        "failed to parse Location header {}: parse {}: {error}",
                        go_quote(&location),
                        go_quote(&location)
                    )),
                ));
            }
            let target = match base.join(&location) {
                Ok(target) => target,
                Err(error) => {
                    close_redirect_body(&mut response, false).await;
                    return Err(wrap(
                        format!("{operation} {}", go_quote(&current_url)),
                        message(format!(
                            "failed to parse Location header {}: {error}",
                            go_quote(&location)
                        )),
                    ));
                }
            };
            close_redirect_body(&mut response, true).await;
            if hops >= 10 {
                return Err(wrap(
                    format!("{operation} {}", go_quote(&location)),
                    message("stopped after 10 redirects"),
                ));
            }
            if !sensitive_stripped && !same_or_subdomain(&original_url, target.as_str()) {
                sensitive_stripped = true;
            }
            let body_dropped = matches!(response.status, 301..=303);
            let mut headers = original_headers.clone();
            headers.retain(|name, _| {
                !(sensitive_stripped && sensitive_header(name) || body_dropped && body_header(name))
            });
            if !(base.scheme() == "https" && target.scheme() == "http")
                && header(&headers, "Referer").is_empty()
            {
                let mut referer = base.clone();
                referer.set_fragment(None);
                let _ = referer.set_username("");
                let _ = referer.set_password(None);
                headers.insert("Referer".into(), vec![referer.into()]);
            }
            let method = if body_dropped && current_method != "GET" && current_method != "HEAD" {
                "GET".into()
            } else {
                current_method
            };
            let logical_host = if !location.contains("://") && !location.starts_with("//") {
                host
            } else {
                None
            };
            request = HttpRequest {
                method,
                url: target.into(),
                logical_host,
                headers,
                body: None,
                content_length: 0,
                context: context.clone(),
            };
        }
    }
}
impl HttpTransport for ClientTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> UpdateFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(self.execute(request))
    }
    fn close_idle_connections(&self) {
        self.transport.close_idle_connections();
    }
}
fn operation(method: &str) -> String {
    if method.is_empty() {
        return "Get".into();
    }
    let mut bytes = method.as_bytes().to_vec();
    for byte in &mut bytes[1..] {
        byte.make_ascii_lowercase();
    }
    String::from_utf8(bytes).unwrap_or_else(|_| method.into())
}
fn header<'a>(headers: &'a Headers, name: &str) -> &'a str {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, values)| values.first())
        .map(String::as_str)
        .unwrap_or("")
}
fn sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "www-authenticate"
            | "cookie"
            | "cookie2"
            | "proxy-authorization"
            | "proxy-authenticate"
    )
}
fn body_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "content-encoding" | "content-language" | "content-location" | "content-type"
    )
}
fn same_or_subdomain(initial: &str, target: &str) -> bool {
    let (Ok(initial), Ok(target)) = (url::Url::parse(initial), url::Url::parse(target)) else {
        return false;
    };
    let (Some(initial), Some(target)) = (initial.host_str(), target.host_str()) else {
        return false;
    };
    initial.eq_ignore_ascii_case(target)
        || (!initial.contains(':')
            && !target.contains(':')
            && target
                .to_ascii_lowercase()
                .ends_with(&format!(".{}", initial.to_ascii_lowercase())))
}
fn strip_password(value: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(value) else {
        return value.into();
    };
    if parsed.password().is_some() {
        let _ = parsed.set_password(Some("***"));
        parsed.into()
    } else {
        value.into()
    }
}
async fn close_redirect_body(response: &mut RawResponse, slurp: bool) {
    if let Some(body) = response.body.as_mut() {
        if slurp && response.content_length <= 2048 {
            let mut remaining = 2048usize;
            let mut buffer = [0u8; 2048];
            while remaining > 0 {
                let read = body.read(&mut buffer[..remaining]).await;
                if read.count > remaining {
                    break;
                }
                remaining -= read.count;
                if read.eof || read.error.is_some() || read.count == 0 {
                    break;
                }
            }
        }
        let _ = body.close().await;
    }
}
struct EmptyBody;
impl RawBody for EmptyBody {
    fn read<'a>(&'a mut self, _buffer: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async {
            RawRead {
                count: 0,
                eof: true,
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async { Ok(()) })
    }
}
struct LazyTransport {
    proxy: String,
    client: Mutex<Option<Arc<ReqwestTransport>>>,
}
impl HttpTransport for LazyTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> UpdateFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        let client = {
            let mut client = self
                .client
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match client.as_ref() {
                Some(client) => Ok(client.clone()),
                None => ReqwestTransport::new(&self.proxy).map(|created| {
                    let created = Arc::new(created);
                    *client = Some(created.clone());
                    created
                }),
            }
        };
        Box::pin(async move { client?.send(request).await })
    }
    fn close_idle_connections(&self) {
        if let Some(client) = self
            .client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            client.close_idle_connections();
        }
    }
}
pub fn default_transport() -> Arc<dyn HttpTransport> {
    Arc::new(ClientTransport::new(Arc::new(LazyTransport {
        proxy: String::new(),
        client: Mutex::new(None),
    })))
}
pub fn ordinary_transport(proxy: &str) -> Result<Arc<dyn HttpTransport>, ExternalError> {
    if !proxy.is_empty() {
        let parsed = pixiv_sdk::oauth::LoginUrl::parse(proxy)
            .ok_or_else(|| wrap("parse proxy URL", Box::new(InvalidProxy)))?;
        if parsed.host().is_empty()
            || !matches!(
                parsed.scheme().to_ascii_lowercase().as_str(),
                "http" | "https" | "socks5" | "socks5h"
            )
        {
            return Err(wrap(
                "proxy URL must use http, https, socks5, or socks5h",
                Box::new(InvalidProxy),
            ));
        }
    }
    Ok(Arc::new(ClientTransport::new(Arc::new(LazyTransport {
        proxy: proxy.into(),
        client: Mutex::new(None),
    }))))
}
#[derive(Debug)]
struct InvalidProxy;
impl std::fmt::Display for InvalidProxy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid proxy configuration")
    }
}
impl std::error::Error for InvalidProxy {}
