pub use crate::artwork::{ResourcePolicy, artwork_variant_resource};
use crate::{
    Error, Reason, Result,
    models::{Artwork, ArtworkPage, UgoiraFrame, UgoiraMetadata},
    resource::{OpenResourceRequest, Resource, ResourceHeaders, ResourceResponse},
    transport::{
        HttpTransport, Request, ResourceReadRequest, ResourceTransport, Response, Transport,
        checked,
    },
};
use chrono::{DateTime, Utc};
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, fmt, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};

pub struct Client<T = HttpTransport> {
    transport: T,
    access_token: String,
    expires_at: Option<DateTime<Utc>>,
    interval: Duration,
    last_request: Mutex<Option<Instant>>,
    resource_policy: ResourcePolicy,
    resource_urls: std::sync::Mutex<BTreeMap<String, String>>,
}

impl<T> fmt::Debug for Client<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("pixiv::Client { .. }")
    }
}

impl Client<HttpTransport> {
    pub fn new(access_token: &str, proxy: Option<&str>) -> Result<Self> {
        Ok(Self::with_transport(
            access_token,
            HttpTransport::new(proxy)?,
        ))
    }
}

impl<T: Transport> Client<T> {
    pub fn with_transport(access_token: &str, transport: T) -> Self {
        Self {
            transport,
            access_token: access_token.trim().to_owned(),
            expires_at: None,
            interval: Duration::ZERO,
            last_request: Mutex::new(None),
            resource_policy: ResourcePolicy::default(),
            resource_urls: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    pub fn with_pacing(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
    pub fn with_resource_policy(mut self, policy: ResourcePolicy) -> Self {
        self.resource_policy = policy;
        self
    }
    pub fn with_expiry(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    async fn get(
        &self,
        path: &str,
        parameters: Vec<(String, String)>,
        operation: &'static str,
    ) -> Result<Value> {
        if self.access_token.is_empty() {
            return Err(Error::new(Reason::Unauthorized, operation));
        }
        if self.expires_at.is_some_and(|expiry| expiry <= Utc::now()) {
            return Err(Error::new(Reason::CredentialsExpired, operation));
        }
        let request = Request {
            method: Method::GET,
            url: format!("https://app-api.pixiv.net{path}"),
            headers: headers(Some(&self.access_token)),
            parameters,
            operation,
        };
        let response = self.send(request.clone()).await?;
        if response.status == 429
            && let Some(delay) = response.retry_after
        {
            tokio::time::sleep(delay.to_std().unwrap_or_default()).await;
            return checked(self.send(request).await?, operation);
        }
        checked(response, operation)
    }

    async fn send(&self, request: Request) -> Result<Response> {
        let mut last = self.last_request.lock().await;
        if let Some(previous) = *last {
            tokio::time::sleep_until(previous + self.interval).await;
        }
        *last = Some(Instant::now());
        drop(last);
        self.transport.send(request).await
    }

    pub async fn artwork(&self, id: i64) -> Result<Artwork> {
        if id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, "Artwork")
                .with_detail("artwork ID must be positive"));
        }
        let body = self
            .get(
                "/v1/illust/detail",
                vec![("illust_id".into(), id.to_string())],
                "Artwork",
            )
            .await?;
        let artwork = crate::artwork::map(
            body.get("illust")
                .ok_or_else(|| malformed("Artwork"))?
                .clone(),
            "Artwork",
            true,
            &self.resource_policy,
        )?;
        self.remember_artwork(&artwork);
        Ok(artwork)
    }

    pub async fn artwork_pages(&self, id: i64) -> Result<Vec<ArtworkPage>> {
        if id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, "ArtworkPages")
                .with_detail("artwork ID must be positive"));
        }
        let body = self
            .get(
                "/v1/illust/detail",
                vec![("illust_id".into(), id.to_string())],
                "ArtworkPages",
            )
            .await?;
        let pages = crate::artwork::pages(
            body.get("illust")
                .ok_or_else(|| malformed("ArtworkPages"))?
                .clone(),
            &self.resource_policy,
        )?;
        for page in &pages {
            self.remember_resource(&page.image.resource);
        }
        Ok(pages)
    }

    pub async fn search_artworks(&self, query: &str) -> Result<Vec<Artwork>> {
        if query.trim().is_empty() {
            return Err(Error::new(Reason::InvalidArgument, "search_artworks"));
        }
        let body = self
            .get(
                "/v1/search/illust",
                vec![
                    ("word".into(), query.into()),
                    ("search_target".into(), "partial_match_for_tags".into()),
                    ("sort".into(), "date_desc".into()),
                ],
                "search_artworks",
            )
            .await?;
        let list = body
            .get("illusts")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("search_artworks"))?;
        list.iter()
            .cloned()
            .map(|value| {
                let artwork =
                    crate::artwork::map(value, "search_artworks", false, &self.resource_policy)?;
                self.remember_artwork(&artwork);
                Ok(artwork)
            })
            .collect()
    }

    fn remember_resource(&self, resource: &Resource) {
        if !resource.reference.is_zero() && !resource.url.is_empty() {
            self.resource_urls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(resource.reference.to_string(), resource.url.clone());
        }
    }

    fn remember_artwork(&self, artwork: &Artwork) {
        self.remember_resource(&artwork.cover.resource);
        self.remember_resource(&artwork.user.profile_image.resource);
        for page in &artwork.pages {
            self.remember_resource(&page.image.resource);
        }
    }

    pub async fn ugoira_metadata(&self, id: i64) -> Result<UgoiraMetadata> {
        if id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, "ugoira_metadata"));
        }
        let body = self
            .get(
                "/v1/ugoira/metadata",
                vec![("illust_id".into(), id.to_string())],
                "ugoira_metadata",
            )
            .await?;
        #[derive(Deserialize)]
        struct Metadata {
            frames: Vec<Frame>,
            zip_urls: Value,
        }
        #[derive(Deserialize)]
        struct Frame {
            file: String,
            delay: u32,
        }
        let metadata: Metadata = serde_json::from_value(
            body.get("ugoira_metadata")
                .cloned()
                .ok_or_else(|| malformed("ugoira_metadata"))?,
        )
        .map_err(|_| malformed("ugoira_metadata"))?;
        if !metadata.zip_urls.is_object()
            || metadata.frames.is_empty()
            || metadata
                .frames
                .iter()
                .any(|frame| frame.file.is_empty() || frame.delay == 0)
        {
            return Err(malformed("ugoira_metadata"));
        }
        Ok(UgoiraMetadata {
            artwork_id: id,
            frames: metadata
                .frames
                .into_iter()
                .map(|frame| UgoiraFrame {
                    filename: frame.file,
                    delay_milliseconds: frame.delay,
                })
                .collect(),
        })
    }
}

impl<T: Transport + ResourceTransport> Client<T> {
    pub async fn open_resource(
        &self,
        request: OpenResourceRequest,
    ) -> Result<ResourceResponse<T::Body>> {
        request.validate()?;
        let invalid = || {
            Error::new(Reason::InvalidArgument, "OpenResource")
                .with_detail("invalid resource reference")
        };
        #[derive(Default, Deserialize)]
        struct Identity {
            k: Option<String>,
            id: Option<i64>,
            p: Option<i64>,
            v: Option<String>,
        }
        if request.reference.product().map_err(|_| invalid())? != "pixiv" {
            return Err(invalid());
        }
        let payload = request.reference.payload().map_err(|_| invalid())?;
        let normalized = crate::codec::normalize_json(&payload).map_err(|_| invalid())?;
        let value: Value = serde_json::from_str(&normalized).map_err(|_| invalid())?;
        let identity = serde_json::from_value::<Option<Identity>>(value)
            .map_err(|_| invalid())?
            .unwrap_or_default();
        let kind = identity.k.unwrap_or_default();
        let id = identity.id.unwrap_or_default();
        let page = identity.p.unwrap_or_default();
        let variant = identity.v.unwrap_or_default();
        if kind.is_empty() || id <= 0 || page < -1 {
            return Err(invalid());
        }
        let cached = self
            .resource_urls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(request.reference.as_str())
            .cloned();
        let url = if let Some(url) = cached {
            url
        } else {
            if kind != "artwork" {
                return Err(Error::new(Reason::InvalidArgument, "OpenResource")
                    .with_detail("resource kind is unsupported"));
            }
            let body = self
                .get(
                    "/v1/illust/detail",
                    vec![("illust_id".into(), id.to_string())],
                    "OpenResource",
                )
                .await?;
            let url = crate::artwork::resource_url(
                body.get("illust")
                    .cloned()
                    .ok_or_else(|| malformed("OpenResource"))?,
                page,
                &variant,
            )?;
            self.resource_policy.validate(&url).map_err(|_| {
                Error::new(Reason::ResourceForbidden, "OpenResource")
                    .with_detail("resolved resource URL is not allowed")
            })?;
            self.resource_urls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(request.reference.to_string(), url.clone());
            url
        };
        self.resource_policy.validate(&url).map_err(|_| {
            Error::new(Reason::ResourceForbidden, "OpenResource")
                .with_detail("resource URL is not allowed")
        })?;
        let mut headers =
            ResourceHeaders::from([("Referer".into(), vec!["https://app-api.pixiv.net/".into()])]);
        for (name, value) in [
            ("Range", request.range),
            ("If-None-Match", request.if_none_match),
            ("If-Modified-Since", request.if_modified_since),
            ("If-Range", request.if_range),
        ] {
            if !value.is_empty() {
                headers.insert(name.into(), vec![value]);
            }
        }
        let policy = self.resource_policy.clone();
        self.transport
            .open_resource(ResourceReadRequest {
                url,
                method: if request.method.is_empty() {
                    "GET".into()
                } else {
                    request.method
                },
                headers,
                operation: "OpenResource",
                validate: Some(Arc::new(move |url| policy.validate(url))),
            })
            .await
    }
}

pub(crate) fn headers(token: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "User-Agent".into(),
            "PixivAndroidApp/5.0.234 (Android 11; Pixel 5)".into(),
        ),
        ("App-OS".into(), "android".into()),
        ("App-OS-Version".into(), "11".into()),
        ("App-Version".into(), "5.0.234".into()),
        ("Referer".into(), "https://app-api.pixiv.net/".into()),
    ];
    if let Some(token) = token {
        headers.push(("Authorization".into(), format!("Bearer {token}")));
    }
    headers
}

fn malformed(operation: &'static str) -> Error {
    Error::new(Reason::MalformedUpstreamResponse, operation)
}
