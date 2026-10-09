use crate::{
    Error, Reason, Result,
    error::Cause,
    transport::{Request, Transport, checked_oauth},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use url::Url;

const CLIENT_ID: &str = "MOBrBDS8blbauoSck0ZfDbtuzpyT";
const CLIENT_SECRET: &str = "lsACyCD94FhDUtGTXi3QzcFE2uU1hqtDaKeqrdwj";
const REDIRECT_URI: &str = "https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback";

pub struct Credentials {
    access_token: String,
    refresh_token: String,
    pub user_id: i64,
    pub username: String,
    pub expires_at: DateTime<Utc>,
}

impl Credentials {
    pub fn access_token(&self) -> &str {
        &self.access_token
    }
    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("user_id", &self.user_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl Serialize for Credentials {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Metadata<'a> {
            user_id: i64,
            username: &'a str,
            expires_at: DateTime<Utc>,
        }
        Metadata {
            user_id: self.user_id,
            username: &self.username,
            expires_at: self.expires_at,
        }
        .serialize(serializer)
    }
}

#[derive(Clone, Default)]
pub struct LoginSession {
    state: Option<Arc<LoginState>>,
}

struct LoginState {
    verifier: String,
    state: String,
    authorization_url: String,
    used: AtomicBool,
}

impl fmt::Debug for LoginSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LoginSession { .. }")
    }
}

impl fmt::Display for LoginSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LoginSession { .. }")
    }
}

impl LoginSession {
    pub fn begin() -> Result<Self> {
        let random_token = |size| -> Result<String> {
            let mut random = vec![0_u8; size];
            getrandom::fill(&mut random).map_err(|_| {
                Error::new(Reason::UpstreamUnavailable, "BeginLogin")
                    .with_detail("cannot create oauth login session")
            })?;
            Ok(URL_SAFE_NO_PAD.encode(random))
        };
        let verifier = random_token(64)?;
        let state = random_token(32)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut authorization_url = Url::parse("https://app-api.pixiv.net/web/v1/login")
            .map_err(|_| Error::new(Reason::LocalStateError, "BeginLogin"))?;
        authorization_url
            .query_pairs_mut()
            .append_pair("client", "pixiv-android")
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state);
        Ok(Self {
            state: Some(Arc::new(LoginState {
                verifier,
                state,
                authorization_url: authorization_url.into(),
                used: AtomicBool::new(false),
            })),
        })
    }

    pub fn authorization_url(&self) -> &str {
        self.state
            .as_ref()
            .map_or("", |state| state.authorization_url.as_str())
    }

    pub fn accepts_callback_url(&self, callback: &str) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        LoginUrl::parse(callback.trim())
            .is_some_and(|url| !url.scheme.is_empty() && url.code(&state.state).is_some())
    }

    pub async fn complete<T: Transport>(
        &self,
        transport: &T,
        callback: &str,
    ) -> Result<Credentials> {
        let state = self.state.as_ref().ok_or_else(|| {
            Error::new(Reason::InvalidArgument, "Complete").with_detail("login session is nil")
        })?;
        let code = login_code(callback, &state.state).ok_or_else(|| {
            Error::new(Reason::InvalidArgument, "Complete").with_detail("login callback is invalid")
        })?;
        if state
            .used
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::new(Reason::InvalidArgument, "Complete")
                .with_detail("login session was already used"));
        }
        exchange(
            transport,
            "Complete",
            vec![
                ("grant_type", "authorization_code".to_owned()),
                ("code", code),
                ("code_verifier", state.verifier.clone()),
                ("redirect_uri", REDIRECT_URI.to_owned()),
            ],
        )
        .await
    }
}

fn login_code(input: &str, state: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    match LoginUrl::parse(input) {
        Some(url) if !url.scheme.is_empty() => url.code(state),
        _ if !input.contains(['?', '#', '&']) => Some(input.to_owned()),
        _ => None,
    }
}

/// Parsed address used by Pixiv OAuth callback and authorization-relay inputs.
///
/// This preserves the supported Go OAuth URL/query behavior rather than the
/// browser-oriented normalization of `url::Url`. It is not a general-purpose
/// Go URL parser; inputs and decoded query values must be representable as UTF-8.
pub struct LoginUrl<'a> {
    scheme: &'a str,
    host: String,
    path: String,
    query: &'a str,
    raw_path: &'a str,
    fragment: String,
    userinfo: bool,
    force_query: bool,
}

impl<'a> LoginUrl<'a> {
    pub fn scheme(&self) -> &str {
        self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn raw_query(&self) -> &str {
        self.query
    }

    pub fn parse(input: &'a str) -> Option<Self> {
        use crate::reference::decode_url_component;
        if input.bytes().any(|byte| byte < 32 || byte == 127) {
            return None;
        }
        let (input, fragment) = input.split_once('#').unwrap_or((input, ""));
        let fragment = decode_url_component(fragment, false)?;
        let colon = input.find(':');
        let (scheme, remainder) = match colon {
            Some(index) if !input[..index].contains(['/', '?']) => {
                let scheme = &input[..index];
                if scheme.is_empty()
                    || !scheme.as_bytes()[0].is_ascii_alphabetic()
                    || !scheme
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
                {
                    return None;
                }
                (scheme, &input[index + 1..])
            }
            _ => ("", input),
        };
        let force_query = remainder.ends_with('?') && remainder.matches('?').count() == 1;
        let (remainder, query) = remainder.split_once('?').unwrap_or((remainder, ""));
        let mut userinfo_present = false;
        let (host, path) = if let Some(authority) = remainder.strip_prefix("//") {
            let boundary = authority.find('/').unwrap_or(authority.len());
            let (authority, path) = authority.split_at(boundary);
            let host = if let Some((userinfo, host)) = authority.rsplit_once('@') {
                userinfo_present = true;
                if !userinfo
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&b))
                {
                    return None;
                }
                decode_url_component(userinfo, false)?;
                host
            } else {
                authority
            };
            let host = login_host(host, scheme)?;
            (host, path)
        } else {
            (String::new(), remainder)
        };
        let opaque = !scheme.is_empty() && !remainder.starts_with('/');
        let raw_path = if opaque { "" } else { path };
        let path = if opaque {
            String::new()
        } else {
            decode_url_component(path, false)?
        };
        Some(Self {
            scheme,
            host,
            path,
            query,
            raw_path,
            fragment,
            userinfo: userinfo_present,
            force_query,
        })
    }

    pub fn has_userinfo(&self) -> bool {
        self.userinfo
    }

    pub fn fragment(&self) -> &str {
        &self.fragment
    }

    pub fn force_query(&self) -> bool {
        self.force_query
    }

    /// Decodes every query entry, rejecting malformed escapes and unescaped semicolons.
    pub fn query_values(&self) -> Option<std::collections::BTreeMap<String, Vec<String>>> {
        use crate::reference::decode_url_component;
        let mut values = std::collections::BTreeMap::<String, Vec<String>>::new();
        for entry in self.query.split('&').filter(|entry| !entry.is_empty()) {
            if entry.contains(';') {
                return None;
            }
            let (key, value) = entry.split_once('=').unwrap_or((entry, ""));
            values
                .entry(decode_url_component(key, true)?)
                .or_default()
                .push(decode_url_component(value, true)?);
        }
        Some(values)
    }

    pub fn escaped_path(&self) -> String {
        self.escaped_replacement_path(&self.path)
    }

    /// Preserves the original escaped path only while it still represents the replacement path.
    pub fn escaped_replacement_path(&self, path: &str) -> String {
        if path == self.path
            && self
                .raw_path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/[]%".contains(&b))
        {
            self.raw_path.to_owned()
        } else {
            escape_login_path(path)
        }
    }

    pub fn query_value(&self, name: &str) -> String {
        use crate::reference::decode_url_component;
        self.query
            .split('&')
            .filter(|entry| !entry.contains(';'))
            .find_map(|entry| {
                let (key, value) = entry.split_once('=').unwrap_or((entry, ""));
                let key = decode_url_component(key, true)?;
                let value = decode_url_component(value, true)?;
                (key == name).then_some(value)
            })
            .unwrap_or_default()
    }

    fn official(&self, path: &str) -> bool {
        self.scheme.eq_ignore_ascii_case("https")
            && self.host.eq_ignore_ascii_case("app-api.pixiv.net")
            && self.path == path
    }

    fn code(&self, expected_state: &str) -> Option<String> {
        let code = self.query_value("code");
        let code = code.trim();
        if code.is_empty() {
            return None;
        }
        let state = self.query_value("state");
        let state = state.trim();
        let optional_state = (self.scheme.eq_ignore_ascii_case("pixiv")
            && self.host.eq_ignore_ascii_case("account")
            && self.path == "/login")
            || self.official("/web/v1/users/auth/pixiv/callback");
        if state != expected_state && (!state.is_empty() || !optional_state) {
            return None;
        }
        Some(code.to_owned())
    }
}

fn login_host(input: &str, scheme: &str) -> Option<String> {
    use crate::reference::decode_url_component;
    let valid_port = |suffix: &str| {
        suffix.is_empty()
            || suffix
                .strip_prefix(':')
                .is_some_and(|port| port.bytes().all(|b| b.is_ascii_digit()))
    };
    let valid_host_byte =
        |b: u8| b >= 128 || b.is_ascii_alphanumeric() || b"-_.~!$&'()*+,;=:[]<>\"".contains(&b);
    let unescape_host = |input: &str, zone: bool| -> Option<String> {
        let mut index = 0;
        let bytes = input.as_bytes();
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'%' {
                let high = (*bytes.get(index + 1)? as char).to_digit(16)?;
                let low = (*bytes.get(index + 2)? as char).to_digit(16)?;
                let decoded = (high * 16 + low) as u8;
                if decoded != b'%'
                    && if zone {
                        decoded >= 128 || (decoded != b' ' && !valid_host_byte(decoded))
                    } else {
                        decoded < 128
                    }
                {
                    return None;
                }
                index += 3;
            } else {
                if !valid_host_byte(byte) {
                    return None;
                }
                index += 1;
            }
        }
        decode_url_component(input, false)
    };
    if let Some(bracketed) = input.strip_prefix('[') {
        if bracketed.contains('[') {
            return None;
        }
        let end = bracketed.rfind(']')?;
        let suffix = &bracketed[end + 1..];
        if !valid_port(suffix) {
            return None;
        }
        let raw_address = &bracketed[..end];
        let address = if let Some((host, zone)) = raw_address.split_once("%25") {
            format!(
                "{}%{}",
                unescape_host(host, false)?,
                unescape_host(zone, true)?
            )
        } else {
            unescape_host(raw_address, false)?
        };
        let ip = match address.split_once('%') {
            Some((_, "")) => return None,
            Some((ip, _)) => ip,
            None => address.as_str(),
        };
        ip.parse::<std::net::Ipv6Addr>().ok()?;
        return Some(format!("[{address}]{suffix}"));
    }
    if input.contains('[') {
        return None;
    }
    if let Some(index) = input.find(':') {
        let index = if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
            index
        } else {
            input.rfind(':')?
        };
        if !valid_port(&input[index..]) {
            return None;
        }
    }
    unescape_host(input, false)
}

pub fn is_official_oauth_callback_url(input: &str) -> bool {
    LoginUrl::parse(input.trim())
        .is_some_and(|url| url.official("/web/v1/users/auth/pixiv/callback"))
}

pub fn is_official_oauth_start_url(input: &str) -> bool {
    LoginUrl::parse(input.trim()).is_some_and(|url| url.official("/web/v1/users/auth/pixiv/start"))
}

pub async fn refresh<T: Transport>(transport: &T, refresh_token: &str) -> Result<Credentials> {
    let token = refresh_token.trim();
    if token.is_empty() {
        return Err(
            Error::new(Reason::CredentialsExpired, "Open").with_detail("refresh token is required")
        );
    }
    if looks_like_cookie(token) {
        return Err(Error::new(Reason::CredentialsExpired, "Open")
            .with_cause(Cause::Redacted("pixiv upstream request failed".to_owned())));
    }
    let body = request_exchange(
        transport,
        "Open",
        vec![
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", token.to_owned()),
        ],
    )
    .await?;
    let decoded: RefreshResponse = if body.is_null() {
        RefreshResponse::default()
    } else {
        serde_json::from_value(body)
            .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, "Open"))?
    };
    let payload = if !decoded.response.access_token.is_empty()
        || !decoded.response.refresh_token.is_empty()
        || decoded.response.user.id.0 != 0
    {
        decoded.response
    } else {
        decoded.root
    };
    if payload.access_token.is_empty() {
        return Err(Error::new(Reason::MalformedUpstreamResponse, "Open"));
    }
    if payload.user.id.0 <= 0 {
        return Err(Error::new(Reason::MalformedUpstreamResponse, "Open")
            .with_detail("oauth response did not include account identity"));
    }
    let expires_at = if payload.expires_in <= 0 {
        DateTime::from_timestamp(-62_135_596_800, 0)
            .ok_or_else(|| Error::new(Reason::LocalStateError, "Open"))?
    } else {
        Utc::now() + TimeDelta::nanoseconds(payload.expires_in.wrapping_mul(1_000_000_000))
    };
    Ok(Credentials {
        access_token: payload.access_token,
        refresh_token: if payload.refresh_token.is_empty() {
            token.to_owned()
        } else {
            payload.refresh_token
        },
        user_id: payload.user.id.0,
        username: payload.user.name,
        expires_at,
    })
}

fn looks_like_cookie(value: &str) -> bool {
    if value.to_ascii_lowercase().starts_with("cookie:") {
        return true;
    }
    let mut pairs = 0;
    for part in value.split(';') {
        let Some((name, token)) = part.trim().split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || token.trim().is_empty() {
            continue;
        }
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "refresh_token"
                | "phpsessid"
                | "session"
                | "sessionid"
                | "session_id"
                | "csrftoken"
                | "csrf_token"
                | "device_token"
                | "yuid_b"
                | "p_ab_id"
                | "p_ab_id_2"
                | "privacy_policy_agreement"
                | "privacy_policy_notification"
        ) {
            return true;
        }
        pairs += 1;
    }
    pairs > 1
}

fn nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(
    deserializer: D,
) -> std::result::Result<T, D::Error> {
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Default, Deserialize)]
struct RefreshResponse {
    #[serde(flatten)]
    root: RefreshPayload,
    #[serde(default, deserialize_with = "nullable")]
    response: RefreshPayload,
}

#[derive(Default, Deserialize)]
struct RefreshPayload {
    #[serde(default, deserialize_with = "nullable")]
    access_token: String,
    #[serde(default, deserialize_with = "nullable")]
    refresh_token: String,
    #[serde(default, deserialize_with = "nullable")]
    expires_in: i64,
    #[serde(default, deserialize_with = "nullable")]
    user: RefreshUser,
}

#[derive(Default, Deserialize)]
struct RefreshUser {
    #[serde(default, deserialize_with = "nullable")]
    id: RefreshUserId,
    #[serde(default, deserialize_with = "nullable")]
    name: String,
}

#[derive(Default)]
struct RefreshUserId(i64);

impl<'de> Deserialize<'de> for RefreshUserId {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let id = match value {
            serde_json::Value::Number(value) => value.as_i64(),
            serde_json::Value::String(value) => value.parse().ok(),
            _ => None,
        }
        .ok_or_else(|| serde::de::Error::custom("invalid OAuth identity"))?;
        Ok(Self(id))
    }
}

async fn exchange<T: Transport>(
    transport: &T,
    operation: &'static str,
    extra: Vec<(&str, String)>,
) -> Result<Credentials> {
    let body = request_exchange(transport, operation, extra).await?;
    decode_login(body, operation)
}

async fn request_exchange<T: Transport>(
    transport: &T,
    operation: &'static str,
    extra: Vec<(&str, String)>,
) -> Result<serde_json::Value> {
    let mut parameters = vec![
        ("client_id".to_owned(), CLIENT_ID.to_owned()),
        ("client_secret".to_owned(), CLIENT_SECRET.to_owned()),
        ("include_policy".to_owned(), "true".to_owned()),
    ];
    parameters.extend(
        extra
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value)),
    );
    let mut headers = super::pixiv::headers(None);
    headers.push((
        "Content-Type".to_owned(),
        "application/x-www-form-urlencoded".to_owned(),
    ));
    checked_oauth(
        transport
            .send(Request {
                method: Method::POST,
                url: "https://oauth.secure.pixiv.net/auth/token".to_owned(),
                headers,
                parameters,
                operation,
            })
            .await?,
        operation,
    )
}

fn decode_login(body: serde_json::Value, operation: &'static str) -> Result<Credentials> {
    let decoded: RefreshResponse = if body.is_null() {
        RefreshResponse::default()
    } else {
        serde_json::from_value(body)
            .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, operation))?
    };
    let payload = if !decoded.response.access_token.is_empty()
        || !decoded.response.refresh_token.is_empty()
        || decoded.response.user.id.0 != 0
    {
        decoded.response
    } else {
        decoded.root
    };
    if payload.refresh_token.is_empty() {
        return Err(Error::new(Reason::MalformedUpstreamResponse, operation));
    }
    if payload.user.id.0 <= 0 || payload.refresh_token.trim().is_empty() {
        return Err(Error::new(Reason::MalformedUpstreamResponse, operation)
            .with_detail("oauth response did not include account identity"));
    }
    let expires_at = if payload.expires_in <= 0 {
        DateTime::from_timestamp(-62_135_596_800, 0)
            .ok_or_else(|| Error::new(Reason::LocalStateError, operation))?
    } else {
        Utc::now() + TimeDelta::nanoseconds(payload.expires_in.wrapping_mul(1_000_000_000))
    };
    Ok(Credentials {
        access_token: payload.access_token,
        refresh_token: payload.refresh_token,
        user_id: payload.user.id.0,
        username: payload.user.name,
        expires_at,
    })
}

fn escape_login_path(path: &str) -> String {
    let mut output = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~$&+,/:;=@".contains(&byte) {
            output.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}
