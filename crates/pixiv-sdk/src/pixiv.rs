pub use crate::artwork::{ResourcePolicy, artwork_variant_resource};
pub use crate::artwork_feeds::{RecommendedArtworksRequest, RelatedArtworksRequest};
pub use crate::artwork_series::ArtworkSeriesRequest;
pub use crate::bookmark::{
    ArtworkBookmarkRequest, NovelBookmarkRequest, UserArtworkBookmarkTagsRequest,
    UserNovelBookmarkTagsRequest,
};
pub use crate::bookmark_lists::{UserArtworkBookmarksRequest, UserNovelBookmarksRequest};
pub use crate::mutation::*;
pub use crate::novel::NovelRequest;
pub use crate::novel_content::NovelContentRequest;
pub use crate::novel_ranking::NovelRankingRequest;
pub use crate::novel_search::SearchNovelsRequest;
pub use crate::novel_series::NovelSeriesRequest;
pub use crate::ranking::*;
pub use crate::search::SearchArtworksRequest;
pub use crate::trending::TrendingArtworkTagsRequest;
use crate::{
    Error, Reason, Result,
    models::{Artwork, ArtworkPage, UgoiraMetadata},
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
    accept_language: String,
    expires_at: Option<DateTime<Utc>>,
    interval: Duration,
    last_request: Mutex<Option<Instant>>,
    pub(crate) resource_policy: ResourcePolicy,
    pub(crate) user_id: i64,
    username: String,
    pub(crate) cursor_instance: Option<String>,
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
        let mut random = [0_u8; 16];
        let cursor_instance = getrandom::fill(&mut random)
            .ok()
            .map(|()| random.iter().map(|byte| format!("{byte:02x}")).collect());
        Self {
            transport,
            access_token: access_token.trim().to_owned(),
            accept_language: String::new(),
            expires_at: None,
            interval: Duration::ZERO,
            last_request: Mutex::new(None),
            resource_policy: ResourcePolicy::default(),
            user_id: 0,
            username: String::new(),
            cursor_instance,
            resource_urls: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    pub fn from_credentials(credentials: &crate::oauth::Credentials, transport: T) -> Self {
        let mut client = Self::with_transport(credentials.access_token(), transport);
        client.user_id = credentials.user_id;
        client.username = credentials.username.clone();
        client
    }

    pub fn user_id(&self) -> i64 {
        self.user_id
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn with_accept_language(mut self, language: impl AsRef<str>) -> Self {
        self.accept_language = language.as_ref().trim().to_owned();
        self
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

    fn content_request(
        &self,
        path: &str,
        parameters: Vec<(String, String)>,
        operation: &'static str,
    ) -> Result<Request> {
        if self.access_token.is_empty() {
            return Err(Error::new(Reason::Unauthorized, operation));
        }
        if self.expires_at.is_some_and(|expiry| expiry <= Utc::now()) {
            return Err(Error::new(Reason::CredentialsExpired, operation));
        }
        let mut headers = headers(Some(&self.access_token));
        if !self.accept_language.is_empty() {
            headers.push(("Accept-Language".into(), self.accept_language.clone()));
        }
        if self.user_id > 0 {
            headers.push(("X-User-Id".into(), self.user_id.to_string()));
        }
        Ok(Request {
            method: Method::GET,
            url: format!("https://app-api.pixiv.net{path}"),
            headers,
            parameters,
            operation,
        })
    }

    pub(crate) async fn get(
        &self,
        path: &str,
        parameters: Vec<(String, String)>,
        operation: &'static str,
    ) -> Result<Value> {
        let request = self.content_request(path, parameters, operation)?;
        let response = self.send(request.clone()).await?;
        if response.status == 429
            && let Some(delay) = response.retry_after
        {
            tokio::time::sleep(delay.to_std().unwrap_or_default()).await;
            return checked(self.send(request).await?, operation);
        }
        checked(response, operation)
    }

    pub(crate) async fn get_json(
        &self,
        path: &str,
        parameters: Vec<(String, String)>,
        operation: &'static str,
    ) -> Result<Vec<u8>> {
        let request = self.content_request(path, parameters, operation)?;
        self.pace().await;
        let response = self.transport.send_json(request.clone()).await?;
        if response.status == 429
            && let Some(delay) = response.retry_after
        {
            tokio::time::sleep(delay.to_std().unwrap_or_default()).await;
            self.pace().await;
            return crate::transport::checked_json(
                self.transport.send_json(request).await?,
                operation,
            );
        }
        crate::transport::checked_json(response, operation)
    }

    async fn send(&self, request: Request) -> Result<Response> {
        self.pace().await;
        self.transport.send(request).await
    }

    async fn pace(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(previous) = *last {
            let due = previous + self.interval;
            if due > Instant::now() {
                tokio::time::sleep_until(due).await;
            }
        }
        *last = Some(Instant::now());
        drop(last);
    }

    pub(crate) async fn post_form(
        &self,
        path: &str,
        parameters: Vec<(String, String)>,
        operation: &'static str,
    ) -> Result<()> {
        let mut request = self.content_request(path, parameters, operation)?;
        request.method = Method::POST;
        self.pace().await;
        checked(self.transport.post_form(request).await?, operation)?;
        Ok(())
    }

    pub(crate) async fn post_form_json(
        &self,
        path: &str,
        parameters: Vec<(String, String)>,
        operation: &'static str,
    ) -> Result<Vec<u8>> {
        let mut request = self.content_request(path, parameters, operation)?;
        request.method = Method::POST;
        self.pace().await;
        crate::transport::checked_json(self.transport.send_json(request).await?, operation)
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

    pub(crate) fn remember_resource(&self, resource: &Resource) {
        if !resource.reference.is_zero() && !resource.url.is_empty() {
            self.resource_urls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(resource.reference.to_string(), resource.url.clone());
        }
    }

    pub(crate) fn remember_artwork(&self, artwork: &Artwork) {
        self.remember_resource(&artwork.cover.resource);
        self.remember_resource(&artwork.user.profile_image.resource);
        for page in &artwork.pages {
            self.remember_resource(&page.image.resource);
        }
    }

    pub async fn ugoira_metadata(&self, id: i64) -> Result<UgoiraMetadata> {
        if id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, "UgoiraMetadata")
                .with_detail("artwork ID must be positive"));
        }
        let body = self
            .get(
                "/v1/ugoira/metadata",
                vec![("illust_id".into(), id.to_string())],
                "UgoiraMetadata",
            )
            .await?;
        crate::ugoira::map(id, &body, &self.resource_policy, |resource| {
            self.remember_resource(resource);
        })
    }
}

impl<T: Transport + ResourceTransport> Client<T> {
    pub async fn save_resource(
        &self,
        reference: crate::resource::ResourceRef,
        options: crate::resource::SaveOptions,
    ) -> Result<crate::resource::SavedResource> {
        let operation = "SaveResource";
        crate::save::validate_path(&options, operation)?;
        let url = self.resolve_resource_url(&reference, operation).await?;
        self.resource_policy.validate(&url).map_err(|_| {
            Error::new(Reason::ResourceForbidden, operation)
                .with_detail("resource URL is not allowed")
        })?;
        let response = self
            .open_resource(OpenResourceRequest {
                reference,
                method: "GET".into(),
                ..Default::default()
            })
            .await?;
        crate::save::write(response, options, operation).await
    }

    pub async fn save_resource_url(
        &self,
        url: &str,
        options: crate::resource::SaveOptions,
    ) -> Result<crate::resource::SavedResource> {
        let operation = "SaveResourceURL";
        crate::save::validate_path(&options, operation)?;
        self.resource_policy.validate(url).map_err(|_| {
            Error::new(Reason::ResourceForbidden, operation)
                .with_detail("resource URL is not allowed")
        })?;
        let policy = self.resource_policy.clone();
        let response = self
            .transport
            .open_resource(ResourceReadRequest {
                url: url.into(),
                method: "GET".into(),
                headers: ResourceHeaders::from([(
                    "Referer".into(),
                    vec!["https://app-api.pixiv.net/".into()],
                )]),
                operation,
                validate: Some(Arc::new(move |url| policy.validate(url))),
            })
            .await?;
        crate::save::write(response, options, operation).await
    }

    async fn resolve_resource_url(
        &self,
        reference: &crate::resource::ResourceRef,
        operation: &'static str,
    ) -> Result<String> {
        let invalid = || {
            Error::new(Reason::InvalidArgument, operation).with_detail("invalid resource reference")
        };
        #[derive(Default, Deserialize)]
        struct Identity {
            k: Option<String>,
            id: Option<i64>,
            p: Option<i64>,
            v: Option<String>,
        }
        if reference.product().map_err(|_| invalid())? != "pixiv" {
            return Err(invalid());
        }
        let payload = reference.payload().map_err(|_| invalid())?;
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
            .get(reference.as_str())
            .cloned();
        let url = if let Some(url) = cached {
            url
        } else {
            let (endpoint, parameters) = match kind.as_str() {
                "artwork" => (
                    "/v1/illust/detail",
                    vec![("illust_id".into(), id.to_string())],
                ),
                "novel_cover" => (
                    "/v2/novel/detail",
                    vec![("novel_id".into(), id.to_string())],
                ),
                "user_profile" => ("/v1/user/detail", vec![("user_id".into(), id.to_string())]),
                "stamp" => ("/v1/stamps", vec![]),
                "ugoira_archive" => (
                    "/v1/ugoira/metadata",
                    vec![("illust_id".into(), id.to_string())],
                ),
                _ => {
                    return Err(Error::new(Reason::InvalidArgument, operation)
                        .with_detail("resource kind is unsupported"));
                }
            };
            let body = if kind == "stamp" {
                let raw = self.get_json(endpoint, parameters, operation).await?;
                crate::user_wire::decode_stamps(&raw, operation)?
            } else {
                self.get(endpoint, parameters, operation).await?
            };
            let url = if kind == "artwork" {
                crate::artwork::resource_url(
                    body.get("illust")
                        .cloned()
                        .ok_or_else(|| malformed(operation))?,
                    page,
                    &variant,
                    operation,
                )?
            } else {
                crate::resource_resolution::resolve(&kind, id, &variant, &body, operation)?
            };
            self.resource_policy.validate(&url).map_err(|_| {
                Error::new(Reason::ResourceForbidden, operation)
                    .with_detail("resolved resource URL is not allowed")
            })?;
            self.resource_urls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(reference.to_string(), url.clone());
            url
        };
        Ok(url)
    }

    pub async fn open_resource(
        &self,
        request: OpenResourceRequest,
    ) -> Result<ResourceResponse<T::Body>> {
        request.validate()?;
        let url = self
            .resolve_resource_url(&request.reference, "OpenResource")
            .await?;
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

pub use crate::user_detail::{CurrentUserRequest, UserRequest};

pub use crate::user_search::SearchUsersRequest;

pub use crate::recommendations::{RecommendedNovelsRequest, RecommendedUsersRequest};

pub use crate::user_works::{
    USER_ARTWORK_KIND_ILLUST, USER_ARTWORK_KIND_ILLUSTRATION, USER_ARTWORK_KIND_MANGA,
    USER_ARTWORK_KIND_UGOIRA, UserArtworkKind, UserArtworksRequest, UserNovelsRequest,
};

pub use crate::user_relationships::{
    MyPixivUsersRequest, RelatedUsersRequest, UserBlockedUsersRequest, UserFollowersRequest,
    UserFollowingRequest,
};

pub use crate::timeline::{
    FollowingArtworksRequest, FollowingNovelsRequest, LatestArtworksRequest, LatestNovelsRequest,
    MyPixivArtworksRequest, MyPixivNovelsRequest,
};

pub use crate::comments::{ArtworkCommentsRequest, NovelCommentsRequest};
pub use crate::stamps::StampsRequest;

pub use crate::comment_mutations::*;
