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
use std::fmt;
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

pub struct LoginSession {
    verifier: String,
    authorization_url: Url,
}

impl fmt::Debug for LoginSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LoginSession { .. }")
    }
}

impl LoginSession {
    pub fn begin() -> Result<Self> {
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| Error::new(Reason::LocalStateError, "login"))?;
        let verifier = URL_SAFE_NO_PAD.encode(random);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut authorization_url = Url::parse("https://app-api.pixiv.net/web/v1/login")
            .map_err(|_| Error::new(Reason::LocalStateError, "login"))?;
        authorization_url
            .query_pairs_mut()
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("client", "pixiv-android");
        Ok(Self {
            verifier,
            authorization_url,
        })
    }

    pub fn authorization_url(&self) -> &str {
        self.authorization_url.as_str()
    }

    pub async fn complete<T: Transport>(
        self,
        transport: &T,
        callback: &str,
    ) -> Result<Credentials> {
        let url = Url::parse(callback).map_err(|_| Error::new(Reason::InvalidArgument, "login"))?;
        let valid = (url.scheme() == "pixiv"
            && url.host_str() == Some("account")
            && url.path() == "/login")
            || (url.scheme() == "https"
                && url.host_str() == Some("app-api.pixiv.net")
                && url.path() == "/web/v1/users/auth/pixiv/callback");
        if !valid || !url.username().is_empty() || url.password().is_some() || url.port().is_some()
        {
            return Err(Error::new(Reason::InvalidArgument, "login"));
        }
        let codes: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| key == "code")
            .map(|(_, value)| value.into_owned())
            .collect();
        if codes.len() != 1 || codes[0].is_empty() {
            return Err(Error::new(Reason::InvalidArgument, "login"));
        }
        exchange(
            transport,
            "Complete",
            vec![
                ("grant_type", "authorization_code".to_owned()),
                ("code", codes[0].clone()),
                ("code_verifier", self.verifier),
                ("redirect_uri", REDIRECT_URI.to_owned()),
            ],
        )
        .await
    }
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
    #[derive(Deserialize)]
    struct Payload {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
        user: Identity,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IdentityId {
        Text(String),
        Number(i64),
    }
    #[derive(Deserialize)]
    struct Identity {
        id: IdentityId,
        name: String,
    }
    let payload: Payload = serde_json::from_value(body.get("response").cloned().unwrap_or(body))
        .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, operation))?;
    let user_id = match payload.user.id {
        IdentityId::Text(value) => value.parse::<i64>().ok(),
        IdentityId::Number(value) => Some(value),
    }
    .filter(|id| *id > 0)
    .ok_or_else(|| Error::new(Reason::MalformedUpstreamResponse, operation))?;
    if payload.access_token.is_empty()
        || payload.refresh_token.is_empty()
        || payload.expires_in <= 0
    {
        return Err(Error::new(Reason::MalformedUpstreamResponse, operation));
    }
    let expires_at = TimeDelta::try_seconds(payload.expires_in)
        .and_then(|duration| Utc::now().checked_add_signed(duration))
        .ok_or_else(|| Error::new(Reason::MalformedUpstreamResponse, operation))?;
    Ok(Credentials {
        access_token: payload.access_token,
        refresh_token: payload.refresh_token,
        user_id,
        username: payload.user.name,
        expires_at,
    })
}
