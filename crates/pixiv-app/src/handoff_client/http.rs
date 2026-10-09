use super::{
    HandoffBody, HandoffFuture, HandoffRead, HandoffRequest, HandoffResponse, HandoffTransport,
    HandoffTransportError,
};
use futures_util::{StreamExt, future::poll_fn};
use std::{
    collections::BTreeMap,
    fmt, io,
    net::IpAddr,
    pin::Pin,
    sync::{Mutex, OnceLock},
};
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::{io::StreamReader, sync::CancellationToken};

#[derive(Clone, Default)]
pub struct HandoffProxyEnvironment {
    pub http_proxy: String,
    pub https_proxy: String,
    pub no_proxy: String,
    pub cgi: bool,
}
#[derive(Debug, Clone, Copy)]
pub struct HandoffProxyError;
impl fmt::Display for HandoffProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "refusing to use HTTP_PROXY value in CGI environment; see golang.org/s/cgihttpproxy",
        )
    }
}
impl std::error::Error for HandoffProxyError {}
impl HandoffProxyEnvironment {
    pub fn from_values(values: &BTreeMap<String, String>) -> Self {
        let get = |upper: &str, lower: &str| {
            values
                .get(upper)
                .filter(|v| !v.is_empty())
                .or_else(|| values.get(lower))
                .cloned()
                .unwrap_or_default()
        };
        Self {
            http_proxy: get("HTTP_PROXY", "http_proxy"),
            https_proxy: get("HTTPS_PROXY", "https_proxy"),
            no_proxy: get("NO_PROXY", "no_proxy"),
            cgi: values.get("REQUEST_METHOD").is_some_and(|v| !v.is_empty()),
        }
    }
    pub fn proxy_for_url(&self, endpoint: &str) -> Result<Option<String>, HandoffProxyError> {
        let Some(url) = pixiv_sdk::oauth::LoginUrl::parse(endpoint) else {
            return Ok(None);
        };
        let selected = match url.scheme() {
            "https" => &self.https_proxy,
            "http" => &self.http_proxy,
            _ => return Ok(None),
        };
        let Some(proxy) = parse_proxy(selected) else {
            return Ok(None);
        };
        if url.scheme() == "http" && self.cgi {
            return Err(HandoffProxyError);
        }
        let (host, explicit_port) = split_host_port(url.host());
        let host = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        let port = explicit_port
            .filter(|p| !p.is_empty())
            .unwrap_or(if url.scheme() == "https" { "443" } else { "80" });
        let ip = host.parse::<IpAddr>().ok().map(unmap);
        if host == "localhost" || ip.is_some_and(|ip| ip.is_loopback()) {
            return Ok(None);
        }
        let host = ascii_host(host).to_ascii_lowercase();
        for entry in self.no_proxy.split(',') {
            let entry = super::go_trim(entry).to_ascii_lowercase();
            if entry.is_empty() {
                continue;
            }
            if entry == "*" {
                return Ok(None);
            }
            if let Some((network, prefix)) = entry.split_once('/') {
                if let (Some(ip), Ok(network), Ok(prefix)) =
                    (ip, network.parse::<IpAddr>(), prefix.parse::<u32>())
                    && in_network(ip, network, prefix)
                {
                    return Ok(None);
                }
                continue;
            }
            let (excluded, excluded_port) = split_host_port(&entry);
            if excluded.is_empty() || excluded_port.is_some_and(|p| !p.is_empty() && p != port) {
                continue;
            }
            if let Ok(excluded_ip) = excluded.parse::<IpAddr>() {
                if ip == Some(unmap(excluded_ip)) {
                    return Ok(None);
                }
                continue;
            }
            if ip.is_some() {
                continue;
            }
            let excluded = ascii_host(
                excluded
                    .strip_prefix("*.")
                    .map(|s| format!(".{s}"))
                    .as_deref()
                    .unwrap_or(excluded),
            )
            .to_ascii_lowercase();
            let subdomains_only = excluded.starts_with('.');
            let root = excluded.strip_prefix('.').unwrap_or(&excluded);
            if (!subdomains_only && host == root) || host.ends_with(&format!(".{root}")) {
                return Ok(None);
            }
        }
        Ok(Some(proxy))
    }
}
fn parse_proxy(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let direct = pixiv_sdk::oauth::LoginUrl::parse(raw)
        .filter(|url| !url.scheme().is_empty() && !url.host().is_empty());
    if let Some(url) = direct {
        return Some(format!(
            "{}{}",
            url.scheme().to_ascii_lowercase(),
            &raw[url.scheme().len()..]
        ));
    }
    let prefixed = format!("http://{raw}");
    if pixiv_sdk::oauth::LoginUrl::parse(&prefixed).is_some() {
        Some(prefixed)
    } else {
        None
    }
}
fn split_host_port(host: &str) -> (&str, Option<&str>) {
    if let Some(bracketed) = host.strip_prefix('[')
        && let Some((ip, suffix)) = bracketed.split_once(']')
    {
        if let Some(port) = suffix.strip_prefix(':') {
            return (ip, Some(port));
        }
        if suffix.is_empty() {
            return (host, None);
        }
    }
    if host.matches(':').count() == 1
        && let Some((host, port)) = host.split_once(':')
    {
        return (host, Some(port));
    }
    (host, None)
}
fn ascii_host(host: &str) -> String {
    if host.is_ascii() {
        host.to_owned()
    } else {
        url::Host::parse(host)
            .map(|h| h.to_string())
            .unwrap_or_else(|_| host.to_owned())
    }
}
fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        ip => ip,
    }
}
fn in_network(ip: IpAddr, network: IpAddr, prefix: u32) -> bool {
    let (network, prefix) = match network {
        IpAddr::V6(network) if network.to_ipv4_mapped().is_some() && prefix >= 96 => (
            IpAddr::V4(network.to_ipv4_mapped().expect("mapped IPv4 address")),
            prefix.saturating_sub(96),
        ),
        network => (network, prefix),
    };
    match (ip, network) {
        (IpAddr::V4(ip), IpAddr::V4(net)) if prefix <= 32 => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            u32::from(ip) & mask == u32::from(net) & mask
        }
        (IpAddr::V6(ip), IpAddr::V6(net)) if prefix <= 128 => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            u128::from(ip) & mask == u128::from(net) & mask
        }
        _ => false,
    }
}

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
            static ENVIRONMENT: OnceLock<HandoffProxyEnvironment> = OnceLock::new();
            let environment = self.proxy_environment.as_ref().unwrap_or_else(|| {
                ENVIRONMENT.get_or_init(|| {
                    HandoffProxyEnvironment::from_values(
                        &[
                            "HTTP_PROXY",
                            "http_proxy",
                            "HTTPS_PROXY",
                            "https_proxy",
                            "NO_PROXY",
                            "no_proxy",
                            "REQUEST_METHOD",
                        ]
                        .into_iter()
                        .filter_map(|name| {
                            std::env::var_os(name).map(|value| {
                                (name.to_owned(), value.to_string_lossy().into_owned())
                            })
                        })
                        .collect(),
                    )
                })
            });
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
                        let proxy = proxy
                            .strip_prefix("socks5://")
                            .map(|p| format!("socks5h://{p}"))
                            .unwrap_or_else(|| proxy.clone());
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
