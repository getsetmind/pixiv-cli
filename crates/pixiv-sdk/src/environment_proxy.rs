use std::{collections::BTreeMap, fmt, net::IpAddr, sync::OnceLock};

#[derive(Clone, Default)]
pub struct ProxyEnvironment {
    pub http_proxy: String,
    pub https_proxy: String,
    pub no_proxy: String,
    pub cgi: bool,
}
#[derive(Debug, Clone, Copy)]
pub struct ProxyError;
impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "refusing to use HTTP_PROXY value in CGI environment; see golang.org/s/cgihttpproxy",
        )
    }
}
impl std::error::Error for ProxyError {}
impl ProxyEnvironment {
    pub fn cached() -> &'static Self {
        static ENVIRONMENT: OnceLock<ProxyEnvironment> = OnceLock::new();
        ENVIRONMENT.get_or_init(|| {
            Self::from_values(
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
                    std::env::var_os(name)
                        .map(|value| (name.to_owned(), value.to_string_lossy().into_owned()))
                })
                .collect(),
            )
        })
    }

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
    pub fn proxy_for_url(&self, endpoint: &str) -> Result<Option<String>, ProxyError> {
        let Some(url) = crate::oauth::LoginUrl::parse(endpoint) else {
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
            return Err(ProxyError);
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
            let entry = go_trim(entry).to_ascii_lowercase();
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
    let direct = crate::oauth::LoginUrl::parse(raw)
        .filter(|url| !url.scheme().is_empty() && !url.host().is_empty());
    if let Some(url) = direct {
        return Some(format!(
            "{}{}",
            url.scheme().to_ascii_lowercase(),
            &raw[url.scheme().len()..]
        ));
    }
    let prefixed = format!("http://{raw}");
    if crate::oauth::LoginUrl::parse(&prefixed).is_some() {
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

fn go_trim(value: &str) -> &str {
    value.trim_matches(|c| matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{0085}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'))
}

pub fn reqwest_proxy(proxy: String) -> Option<String> {
    let (scheme, suffix) = proxy.split_once(':')?;
    match scheme {
        "http" | "https" | "socks5h" => Some(proxy),
        "socks5" => Some(format!("socks5h:{suffix}")),
        _ => {
            let parsed = crate::oauth::LoginUrl::parse(&proxy)?;
            let (_, port) = split_host_port(parsed.host());
            let mut proxy = url::Url::parse(&format!("http:{suffix}")).ok()?;
            if port.is_none_or(str::is_empty) {
                proxy.set_port(Some(0)).ok()?;
            }
            Some(proxy.into())
        }
    }
}
