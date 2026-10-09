use pixiv_sdk::oauth::LoginUrl;
use std::{collections::BTreeMap, error::Error, fmt};

pub const RELAY_RESULT_URL_HEADER: &str = "X-Pixiv-Relay-Result-URL";

#[derive(Clone, PartialEq, Eq)]
pub struct RemoteLoginStart {
    pub origin: String,
    pub session_id: String,
    pub proof: String,
}

impl fmt::Debug for RemoteLoginStart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RemoteLoginStart { .. }")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffProtocolError(&'static str);

impl fmt::Display for HandoffProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for HandoffProtocolError {}

type Result<T> = std::result::Result<T, HandoffProtocolError>;
const RELAY_ERROR: HandoffProtocolError = HandoffProtocolError("invalid remote login relay URL");
const LINK_ERROR: HandoffProtocolError = HandoffProtocolError("invalid remote login start link");
const AUTH_ERROR: HandoffProtocolError =
    HandoffProtocolError("remote Pixiv login relay returned an invalid sign-in address");
const RESULT_ERROR: HandoffProtocolError =
    HandoffProtocolError("invalid remote login relay result URL");

pub fn parse_remote_login_link(raw: &str) -> Result<RemoteLoginStart> {
    let parsed = LoginUrl::parse(raw.trim()).ok_or(LINK_ERROR)?;
    if !parsed.scheme().eq_ignore_ascii_case("pixiv")
        || !parsed.host().eq_ignore_ascii_case("account")
        || parsed.path() != "/remote-login"
        || parsed.has_userinfo()
        || !parsed.fragment().is_empty()
    {
        return Err(LINK_ERROR);
    }
    let values = parsed.query_values().ok_or(LINK_ERROR)?;
    if values.len() != 3 {
        return Err(LINK_ERROR);
    }
    let origin = single_nonblank(&values, "origin").ok_or(LINK_ERROR)?;
    let session_id = single_nonblank(&values, "session").ok_or(LINK_ERROR)?;
    let proof = single_nonblank(&values, "access").ok_or(LINK_ERROR)?;
    Ok(RemoteLoginStart {
        origin: canonical_relay_origin(origin).map_err(|_| LINK_ERROR)?,
        session_id: session_id.to_owned(),
        proof: proof.to_owned(),
    })
}

pub fn canonical_relay_origin(raw: &str) -> Result<String> {
    let parsed = relay_base(raw)?;
    let mut path = clean_path(&format!("/{}", parsed.path()));
    if path == "/" {
        path.clear();
    }
    let host: String = parsed
        .host()
        .chars()
        .filter_map(|c| c.to_lowercase().next())
        .collect();
    Ok(serialize_relay(&parsed, &host, &path))
}

pub fn relay_endpoint_url(base: &str, suffix: &str, session_id: &str) -> Result<String> {
    let parsed = relay_base(base)?;
    if session_id.trim().is_empty() {
        return Err(RELAY_ERROR);
    }
    let path = clean_path(&format!("/{}/{suffix}/{session_id}", parsed.path()));
    Ok(serialize_relay(&parsed, parsed.host(), &path))
}

pub fn validate_authorization_url(raw: &str) -> Result<()> {
    let parsed = LoginUrl::parse(raw.trim()).ok_or(AUTH_ERROR)?;
    let host = parsed.host().strip_suffix(':').unwrap_or(parsed.host());
    if !parsed.scheme().eq_ignore_ascii_case("https")
        || !host.eq_ignore_ascii_case("app-api.pixiv.net")
        || parsed.has_userinfo()
        || !parsed.fragment().is_empty()
        || parsed.path() != "/web/v1/login"
        || parsed.escaped_path() != "/web/v1/login"
    {
        return Err(AUTH_ERROR);
    }
    let values = parsed.query_values().ok_or(AUTH_ERROR)?;
    if values.len() != 4
        || single_nonblank(&values, "client") != Some("pixiv-android")
        || single_nonblank(&values, "code_challenge_method") != Some("S256")
        || single_nonblank(&values, "code_challenge").is_none()
        || single_nonblank(&values, "state").is_none()
    {
        return Err(AUTH_ERROR);
    }
    Ok(())
}

pub fn validate_relay_result_url(base: &str, result_url: &str) -> Result<()> {
    let expected = canonical_relay_origin(base)?;
    let expected = LoginUrl::parse(&expected).ok_or(RESULT_ERROR)?;
    let actual = LoginUrl::parse(result_url.trim()).ok_or(RESULT_ERROR)?;
    if !actual.scheme().eq_ignore_ascii_case(expected.scheme())
        || actual.host() != expected.host()
        || actual.has_userinfo()
        || !actual.raw_query().is_empty()
        || !actual.fragment().is_empty()
    {
        return Err(RESULT_ERROR);
    }
    let prefix = format!("{}/", clean_path(&format!("/{}/result", expected.path())));
    let result_id = actual.path().strip_prefix(&prefix).ok_or(RESULT_ERROR)?;
    if result_id.is_empty() || result_id.contains('/') || matches!(result_id, "." | "..") {
        return Err(RESULT_ERROR);
    }
    let mut count = 0;
    for byte in result_id.bytes() {
        if matches!(byte, b'\r' | b'\n') {
            continue;
        }
        if !byte.is_ascii_alphanumeric() && !b"-_".contains(&byte) {
            return Err(RESULT_ERROR);
        }
        count += 1;
    }
    if count % 4 == 1 {
        return Err(RESULT_ERROR);
    }
    Ok(())
}

pub fn is_allowed_pixiv_callback_url(raw: &str) -> bool {
    LoginUrl::parse(raw.trim()).is_some_and(|parsed| {
        parsed.scheme().eq_ignore_ascii_case("pixiv")
            && parsed.host().eq_ignore_ascii_case("account")
            && parsed.path() == "/login"
            && !parsed.query_value("code").trim().is_empty()
    })
}

fn relay_base(raw: &str) -> Result<LoginUrl<'_>> {
    let parsed = LoginUrl::parse(raw.trim()).ok_or(RELAY_ERROR)?;
    if !(parsed.scheme().eq_ignore_ascii_case("http")
        || parsed.scheme().eq_ignore_ascii_case("https"))
        || parsed.host().is_empty()
        || parsed.has_userinfo()
        || !parsed.raw_query().is_empty()
        || !parsed.fragment().is_empty()
    {
        return Err(RELAY_ERROR);
    }
    Ok(parsed)
}

fn single_nonblank<'a>(values: &'a BTreeMap<String, Vec<String>>, key: &str) -> Option<&'a str> {
    let values = values.get(key)?;
    (values.len() == 1 && !values[0].trim().is_empty()).then(|| values[0].as_str())
}

fn clean_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}

fn serialize_relay(parsed: &LoginUrl<'_>, host: &str, path: &str) -> String {
    let mut escaped_host = String::new();
    for byte in host.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:[]<>\"".contains(&byte) {
            escaped_host.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(escaped_host, "%{byte:02X}");
        }
    }
    format!(
        "{}://{}{}{}",
        parsed.scheme().to_ascii_lowercase(),
        escaped_host,
        parsed.escaped_replacement_path(path),
        if parsed.force_query() { "?" } else { "" }
    )
}
