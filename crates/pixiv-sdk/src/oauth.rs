use crate::{
    Error, Reason, Result,
    transport::{Request, Transport, checked},
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
    if token.is_empty()
        || token.chars().any(char::is_whitespace)
        || token.contains('=')
        || token.contains(';')
    {
        return Err(Error::new(Reason::InvalidArgument, "refresh"));
    }
    exchange(
        transport,
        vec![
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", token.to_owned()),
        ],
    )
    .await
}

async fn exchange<T: Transport>(transport: &T, extra: Vec<(&str, String)>) -> Result<Credentials> {
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
    let body = checked(
        transport
            .send(Request {
                method: Method::POST,
                url: "https://oauth.secure.pixiv.net/auth/token".to_owned(),
                headers: super::pixiv::headers(None),
                parameters,
                operation: "oauth",
            })
            .await?,
        "oauth",
    )?;
    #[derive(Deserialize)]
    struct Payload {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
        user: Identity,
    }
    #[derive(Deserialize)]
    struct Identity {
        id: String,
        name: String,
    }
    let payload: Payload = serde_json::from_value(body.get("response").cloned().unwrap_or(body))
        .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, "oauth"))?;
    let user_id = payload
        .user
        .id
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| Error::new(Reason::MalformedUpstreamResponse, "oauth"))?;
    if payload.access_token.is_empty()
        || payload.refresh_token.is_empty()
        || payload.expires_in <= 0
    {
        return Err(Error::new(Reason::MalformedUpstreamResponse, "oauth"));
    }
    let expires_at = TimeDelta::try_seconds(payload.expires_in)
        .and_then(|duration| Utc::now().checked_add_signed(duration))
        .ok_or_else(|| Error::new(Reason::MalformedUpstreamResponse, "oauth"))?;
    Ok(Credentials {
        access_token: payload.access_token,
        refresh_token: payload.refresh_token,
        user_id,
        username: payload.user.name,
        expires_at,
    })
}
