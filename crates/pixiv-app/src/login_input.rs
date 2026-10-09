use pixiv_sdk::oauth::{LoginUrl, is_official_oauth_callback_url, is_official_oauth_start_url};
use std::{error::Error, fmt, net::IpAddr};

pub type CallbackUrlAccepter<'a> = &'a dyn Fn(&str) -> bool;
pub type RelayOpenError = Box<dyn Error + Send + Sync>;
pub type RelayOpener<'a> = &'a mut dyn FnMut(&str) -> Result<(), RelayOpenError>;

#[derive(Debug)]
pub enum LoginInputError {
    Message(String),
    OpenRelay(RelayOpenError),
}

impl fmt::Display for LoginInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => formatter.write_str(message),
            Self::OpenRelay(source) => write!(
                formatter,
                "could not open Pixiv authorization relay URL: {source}"
            ),
        }
    }
}

impl Error for LoginInputError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::OpenRelay(source) => Some(source.as_ref()),
        }
    }
}

#[derive(Debug, Default)]
pub struct LoginServerResult {
    pub code: String,
    pub error: Option<LoginInputError>,
}

#[derive(Debug, Default)]
pub struct LoginInputResult {
    pub result: LoginServerResult,
    pub relayed: bool,
    pub relay_url: String,
}

fn failure(message: &str) -> LoginServerResult {
    LoginServerResult {
        error: Some(LoginInputError::Message(message.to_owned())),
        ..LoginServerResult::default()
    }
}

pub fn login_code_from_input(
    input: &str,
    accepts_callback: Option<CallbackUrlAccepter<'_>>,
) -> LoginServerResult {
    let input = input.trim();
    if input.is_empty() {
        return failure("sign-in result cannot be empty");
    }
    if input.contains("://") || input.starts_with('/') {
        let Some(parsed) = LoginUrl::parse(input) else {
            return failure("invalid sign-in address");
        };
        if parsed.raw_query().is_empty() {
            return failure("sign-in address did not include required details");
        }
        if !accepts_callback.is_some_and(|accepts| accepts(input)) {
            return failure("sign-in address does not match this login session");
        }
    }
    LoginServerResult {
        code: input.to_owned(),
        error: None,
    }
}

pub(crate) fn equal_fold_ascii(input: &str, expected: &str) -> bool {
    let mut chars = input.chars();
    expected.chars().all(|expected| {
        chars.next().is_some_and(|actual| {
            actual.eq_ignore_ascii_case(&expected)
                || (expected == 's' && actual == 'ſ')
                || (expected == 'k' && actual == 'K')
        })
    }) && chars.next().is_none()
}

fn is_post_redirect(parsed: &LoginUrl<'_>) -> bool {
    parsed.scheme().eq_ignore_ascii_case("https")
        && equal_fold_ascii(parsed.host(), "accounts.pixiv.net")
        && parsed.path() == "/post-redirect"
}

pub fn pixiv_post_redirect_return_to(input: &str) -> Option<String> {
    let parsed = LoginUrl::parse(input.trim())?;
    if !is_post_redirect(&parsed) {
        return None;
    }
    let target = parsed.query_value("return_to");
    let target = target.trim();
    is_official_oauth_start_url(target).then(|| target.to_owned())
}

pub fn pixiv_auth_start_matches_challenge(input: &str, expected_challenge: &str) -> bool {
    expected_challenge.is_empty()
        || LoginUrl::parse(input)
            .is_some_and(|url| url.query_value("code_challenge") == expected_challenge)
}

pub fn pixiv_login_challenge(input: &str) -> String {
    LoginUrl::parse(input)
        .map(|url| url.query_value("code_challenge").trim().to_owned())
        .unwrap_or_default()
}

pub fn is_browser_callback_url(input: &str) -> bool {
    LoginUrl::parse(input).is_some_and(|url| {
        (url.scheme().eq_ignore_ascii_case("pixiv")
            && url.host().eq_ignore_ascii_case("account")
            && url.path() == "/login")
            || is_official_oauth_callback_url(input)
    })
}

pub fn classify_login_input(
    input: &str,
    accepts_callback: Option<CallbackUrlAccepter<'_>>,
    expected_challenge: &str,
) -> LoginInputResult {
    if LoginUrl::parse(input.trim()).is_some_and(|url| is_post_redirect(&url)) {
        let error = match pixiv_post_redirect_return_to(input) {
            None => Some("invalid Pixiv authorization relay URL"),
            Some(target) if !pixiv_auth_start_matches_challenge(&target, expected_challenge) => {
                Some("Pixiv authorization relay URL does not match this login attempt")
            }
            Some(_) => None,
        };
        return LoginInputResult {
            result: error.map(failure).unwrap_or_default(),
            relayed: true,
            relay_url: if error.is_none() {
                input.trim().to_owned()
            } else {
                String::new()
            },
        };
    }
    LoginInputResult {
        result: login_code_from_input(input, accepts_callback),
        ..LoginInputResult::default()
    }
}

pub fn login_input_from_text(
    input: &str,
    accepts_callback: Option<CallbackUrlAccepter<'_>>,
    expected_challenge: &str,
    open_relay: Option<RelayOpener<'_>>,
) -> LoginInputResult {
    let result = classify_login_input(input, accepts_callback, expected_challenge);
    if !result.relayed || result.result.error.is_some() {
        return result;
    }
    let error = if result.relay_url.is_empty() {
        Some(LoginInputError::Message(
            "Pixiv authorization relay URL is empty".to_owned(),
        ))
    } else if let Some(open) = open_relay {
        open(&result.relay_url)
            .err()
            .map(LoginInputError::OpenRelay)
    } else {
        Some(LoginInputError::Message(
            "browser opener is not configured".to_owned(),
        ))
    };
    match error {
        Some(error) => LoginInputResult {
            result: LoginServerResult {
                error: Some(error),
                ..LoginServerResult::default()
            },
            relayed: true,
            relay_url: String::new(),
        },
        None => result,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginAddressParseError {
    pub address: String,
    pub reason: &'static str,
}

impl fmt::Display for LoginAddressParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.address.is_empty() {
            formatter.write_str(self.reason)
        } else {
            write!(formatter, "address {}: {}", self.address, self.reason)
        }
    }
}

impl Error for LoginAddressParseError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoginAddressError {
    Message(String),
    Parse {
        message: String,
        source: LoginAddressParseError,
    },
}

impl fmt::Display for LoginAddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) | Self::Parse { message, .. } => formatter.write_str(message),
        }
    }
}

impl Error for LoginAddressError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::Parse { source, .. } => Some(source),
        }
    }
}

pub(crate) fn split_host_port(addr: &str) -> Result<(&str, &str), LoginAddressParseError> {
    let error = |reason| LoginAddressParseError {
        address: addr.to_owned(),
        reason,
    };
    let colon = addr
        .rfind(':')
        .ok_or_else(|| error("missing port in address"))?;
    let (host, left_start, right_start) = if addr.starts_with('[') {
        let end = addr
            .find(']')
            .ok_or_else(|| error("missing ']' in address"))?;
        if end + 1 != colon {
            return Err(error(if addr.as_bytes().get(end + 1) == Some(&b':') {
                "too many colons in address"
            } else {
                "missing port in address"
            }));
        }
        (&addr[1..end], 1, end + 1)
    } else {
        let host = &addr[..colon];
        if host.contains(':') {
            return Err(error("too many colons in address"));
        }
        (host, 0, 0)
    };
    if addr[left_start..].contains('[') {
        return Err(error("unexpected '[' in address"));
    }
    if addr[right_start..].contains(']') {
        return Err(error("unexpected ']' in address"));
    }
    Ok((host, &addr[colon + 1..]))
}

pub fn validate_login_addr(addr: &str) -> Result<(), LoginAddressError> {
    if addr.trim().is_empty() {
        return Err(LoginAddressError::Message(
            "--addr cannot be empty".to_owned(),
        ));
    }
    let quoted = crate::auth_bundle::go_quote(addr);
    let (host, _) = split_host_port(addr).map_err(|source| LoginAddressError::Parse {
        message: format!("invalid --addr {quoted}: {source}"),
        source,
    })?;
    let loopback = equal_fold_ascii(host, "localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| match ip {
            IpAddr::V4(ip) => ip.is_loopback(),
            IpAddr::V6(ip) => {
                ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
            }
        });
    if loopback {
        Ok(())
    } else {
        Err(LoginAddressError::Message(format!(
            "--addr must bind to a loopback address, got {quoted}"
        )))
    }
}

pub fn login_ssh_tunnel_command(addr: &str) -> Result<String, LoginAddressError> {
    let (host, port) = split_host_port(addr).map_err(|source| LoginAddressError::Parse {
        message: format!("parse login listener address: {source}"),
        source,
    })?;
    if host.is_empty() || port.is_empty() {
        return Err(LoginAddressError::Message(
            "login listener address is incomplete".to_owned(),
        ));
    }
    Ok(format!("ssh -N -L {port}:{host}:{port} USER@SERVER"))
}
