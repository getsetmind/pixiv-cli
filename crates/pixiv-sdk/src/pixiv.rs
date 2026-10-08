use crate::{
    Error, Reason, Result,
    models::{Artwork, ArtworkKind, Tag, UgoiraFrame, UgoiraMetadata, User},
    transport::{HttpTransport, Request, Transport, checked},
};
use chrono::{DateTime, Utc};
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;
use std::{fmt, time::Duration};
use tokio::{sync::Mutex, time::Instant};

pub struct Client<T = HttpTransport> {
    transport: T,
    access_token: String,
    expires_at: Option<DateTime<Utc>>,
    interval: Duration,
    last_request: Mutex<Option<Instant>>,
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
        }
    }

    pub fn with_pacing(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
    pub fn with_expiry(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    async fn get(
        &self,
        path: &str,
        mut parameters: Vec<(String, String)>,
        operation: &'static str,
    ) -> Result<Value> {
        if self.access_token.is_empty() {
            return Err(Error::new(Reason::Unauthorized, operation));
        }
        if self.expires_at.is_some_and(|expiry| expiry <= Utc::now()) {
            return Err(Error::new(Reason::CredentialsExpired, operation));
        }
        let mut last = self.last_request.lock().await;
        if let Some(previous) = *last {
            tokio::time::sleep_until(previous + self.interval).await;
        }
        *last = Some(Instant::now());
        drop(last);
        parameters.push(("filter".to_owned(), "for_android".to_owned()));
        checked(
            self.transport
                .send(Request {
                    method: Method::GET,
                    url: format!("https://app-api.pixiv.net{path}"),
                    headers: headers(Some(&self.access_token)),
                    parameters,
                    operation,
                })
                .await?,
            operation,
        )
    }

    pub async fn artwork(&self, id: i64) -> Result<Artwork> {
        if id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, "artwork"));
        }
        let body = self
            .get(
                "/v1/illust/detail",
                vec![("illust_id".into(), id.to_string())],
                "artwork",
            )
            .await?;
        map_artwork(
            body.get("illust")
                .ok_or_else(|| malformed("artwork"))?
                .clone(),
            "artwork",
        )
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
            .map(|value| map_artwork(value, "search_artworks"))
            .collect()
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

fn map_artwork(value: Value, operation: &'static str) -> Result<Artwork> {
    #[derive(Deserialize)]
    struct WireArtwork {
        id: i64,
        title: String,
        caption: String,
        #[serde(rename = "type")]
        kind: String,
        tags: Vec<Tag>,
        user: User,
        create_date: DateTime<Utc>,
        total_bookmarks: u64,
        total_view: u64,
        width: u32,
        height: u32,
        page_count: u32,
        x_restrict: u32,
        #[serde(default)]
        illust_ai_type: u32,
    }
    let wire: WireArtwork = serde_json::from_value(value).map_err(|_| malformed(operation))?;
    if wire.id <= 0 || wire.user.id <= 0 || wire.page_count == 0 {
        return Err(malformed(operation));
    }
    let kind = match wire.kind.as_str() {
        "illust" => ArtworkKind::Illust,
        "manga" => ArtworkKind::Manga,
        "ugoira" => ArtworkKind::Ugoira,
        _ => ArtworkKind::Unknown,
    };
    Ok(Artwork {
        id: wire.id,
        title: wire.title,
        caption: wire.caption,
        kind,
        raw_kind: wire.kind,
        tags: wire.tags,
        user: wire.user,
        published_at: wire.create_date,
        total_bookmarks: wire.total_bookmarks,
        total_views: wire.total_view,
        width: wire.width,
        height: wire.height,
        page_count: wire.page_count,
        x_restrict: wire.x_restrict,
        ai_type: wire.illust_ai_type,
    })
}
