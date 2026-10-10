use super::{Failure, transport::RawTransport};
use crate::{Reason, reference::decode_url_component};
use std::{fmt, sync::Arc};
use url::Url;

pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:148.0) Gecko/20100101 Firefox/148.0";
#[derive(Clone, Default)]
pub struct Options {
    pub http_client: Option<Arc<dyn RawTransport>>,
    pub proxy_url: String,
    pub user_agent: String,
    pub flare_solverr: Option<FlareSolverrOptions>,
}
impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("fanbox.Options").finish_non_exhaustive()
    }
}
#[derive(Clone, Default)]
pub struct FlareSolverrOptions {
    pub url: String,
    pub proxy_url: String,
}
impl fmt::Debug for FlareSolverrOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("fanbox.FlareSolverrOptions")
            .finish_non_exhaustive()
    }
}
pub(crate) fn invalid_option(message: &str) -> Failure {
    Failure::new(
        Reason::InvalidArgument,
        format!("fanbox: invalid option: {message}"),
    )
}
pub(crate) fn validate_user_agent(raw: &str) -> std::result::Result<String, Failure> {
    if raw.is_empty() {
        return Ok(DEFAULT_USER_AGENT.into());
    }
    if raw.contains(['\r', '\n', '\0']) {
        return Err(invalid_option(
            "FANBOX User-Agent contains an invalid header character",
        ));
    }
    Ok(raw.into())
}
pub(crate) struct ParsedUrl {
    pub parsed: Url,
    pub wire: String,
    pub authority: String,
    pub path: String,
    pub query: Option<String>,
    pub fragment: String,
}
pub(crate) fn parse_url(raw: &str) -> Option<ParsedUrl> {
    if raw.bytes().any(|byte| byte < 32 || byte == 127) || raw.trim_start() != raw {
        return None;
    }
    let (scheme, rest) = raw.split_once("://")?;
    let boundary = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..boundary];
    if authority.is_empty() || authority.contains('%') {
        return None;
    }
    let remainder = &rest[boundary..];
    let (remainder, fragment) = remainder.split_once('#').unwrap_or((remainder, ""));
    let (path, query) = remainder
        .split_once('?')
        .map(|(path, query)| (path, Some(query)))
        .unwrap_or((remainder, None));
    decode_url_component(path, false)?;
    decode_url_component(fragment, false)?;
    let parsed = Url::parse(raw).ok().or_else(|| {
        // Go validates decimal port syntax without imposing a 16-bit range.
        let (host, port) = authority.rsplit_once(':')?;
        if !port.bytes().all(|byte| byte.is_ascii_digit())
            || (host.contains(':') && !(host.starts_with('[') && host.ends_with(']')))
        {
            return None;
        }
        Url::parse(&format!("{scheme}://{host}{}", &rest[boundary..])).ok()
    })?;
    parsed.host_str()?;
    let mut wire = format!("{}://{authority}{path}", scheme.to_ascii_lowercase());
    if let Some(query) = query {
        wire.push('?');
        wire.push_str(query);
    }
    if !fragment.is_empty() {
        wire.push('#');
        wire.push_str(fragment);
    }
    Some(ParsedUrl {
        parsed,
        wire,
        authority: authority.into(),
        path: path.into(),
        query: query.map(str::to_owned),
        fragment: fragment.into(),
    })
}
pub(crate) fn validate_proxy_url(raw: &str) -> std::result::Result<String, Failure> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    let parsed = parse_url(raw)
        .filter(|target| {
            matches!(target.parsed.scheme(), "http" | "https") && !target.authority.contains('@')
        })
        .ok_or_else(|| invalid_option("FANBOX proxy URL is invalid"))?;
    Ok(parsed.wire)
}
pub(crate) fn normalize_solver(
    options: Option<FlareSolverrOptions>,
) -> std::result::Result<Option<FlareSolverrOptions>, Failure> {
    let Some(mut options) = options else {
        return Ok(None);
    };
    let service = parse_url(&options.url)
        .filter(|target| {
            matches!(target.parsed.scheme(), "http" | "https")
                && !target.authority.contains('@')
                && target.query.is_none()
                && target.fragment.is_empty()
                && matches!(
                    decode_url_component(&target.path, false).as_deref(),
                    Some("" | "/")
                )
        })
        .ok_or_else(|| invalid_option("FlareSolverr service URL is invalid"))?;
    options.url = format!("{}://{}", service.parsed.scheme(), service.authority);
    if !options.proxy_url.is_empty() {
        parse_url(&options.proxy_url)
            .filter(|target| {
                matches!(target.parsed.scheme(), "http" | "socks4" | "socks5")
                    && !target.authority.contains('@')
                    && target.query.is_none()
                    && target.fragment.is_empty()
                    && matches!(
                        decode_url_component(&target.path, false).as_deref(),
                        Some("" | "/")
                    )
            })
            .ok_or_else(|| invalid_option("FlareSolverr upstream proxy URL is invalid"))?;
    }
    Ok(Some(options))
}
pub(crate) fn normalize_cookie(header: &str) -> std::result::Result<String, Failure> {
    normalize_cookie_bytes(header.as_bytes())
}
pub(crate) fn normalize_cookie_bytes(header: &[u8]) -> std::result::Result<String, Failure> {
    let fail = |message| Failure::new(Reason::CredentialsExpired, message);
    if trim_cookie_space(header).is_empty() {
        return Err(fail("FANBOX cookie header is required"));
    }
    if header.iter().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err(fail("FANBOX cookie header must not contain line breaks"));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut pairs = vec![];
    for pair in header.split(|byte| *byte == b';') {
        let pair = trim_cookie_space(pair);
        let (name, value) = pair
            .iter()
            .position(|byte| *byte == b'=')
            .map_or((&b""[..], &b""[..]), |index| {
                (&pair[..index], &pair[index + 1..])
            });
        let (name, value) = (trim_cookie_space(name), trim_cookie_space(value));
        if name.is_empty()
            || !name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(byte))
            || value.is_empty()
            || !value
                .iter()
                .all(|byte| (0x21..=0x7e).contains(byte) && !b"\",;\\".contains(byte))
        {
            return Err(fail(
                "FANBOX cookie header contains a malformed cookie pair",
            ));
        }
        if !seen.insert(name) {
            return Err(fail("FANBOX cookie header contains duplicate cookie names"));
        }
        let name = std::str::from_utf8(name)
            .map_err(|_| fail("FANBOX cookie header contains a malformed cookie pair"))?;
        let value = std::str::from_utf8(value)
            .map_err(|_| fail("FANBOX cookie header contains a malformed cookie pair"))?;
        pairs.push(format!("{name}={value}"));
    }
    if !seen.contains(&b"FANBOXSESSID"[..]) {
        return Err(fail("FANBOX cookie header must contain FANBOXSESSID"));
    }
    Ok(pairs.join("; "))
}
fn trim_cookie_space(mut bytes: &[u8]) -> &[u8] {
    while let Some(length) = edge_space(bytes, true) {
        bytes = &bytes[length..];
    }
    while let Some(length) = edge_space(bytes, false) {
        bytes = &bytes[..bytes.len() - length];
    }
    bytes
}
fn edge_space(bytes: &[u8], front: bool) -> Option<usize> {
    for length in 1..=bytes.len().min(4) {
        let edge = if front {
            &bytes[..length]
        } else {
            &bytes[bytes.len() - length..]
        };
        if let Ok(value) = std::str::from_utf8(edge) {
            return value
                .chars()
                .next()
                .filter(|value| value.is_whitespace())
                .map(|_| length);
        }
    }
    None
}
