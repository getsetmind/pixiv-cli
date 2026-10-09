use crate::{
    Client, Error, Reason, Result,
    cursor::{Cursor, Page},
    models::{Artwork, Novel},
    transport::Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FollowingArtworksRequest {
    pub restrict: String,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FollowingNovelsRequest {
    pub restrict: String,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LatestArtworksRequest {
    pub content_type: String,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LatestNovelsRequest {
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MyPixivArtworksRequest {
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MyPixivNovelsRequest {
    pub cursor: Cursor,
}
#[derive(Deserialize)]
struct ArtworksEnvelope {
    illusts: Option<Vec<Value>>,
    next_url: Option<String>,
}
#[derive(Deserialize)]
struct NovelsEnvelope {
    novels: Option<Vec<Value>>,
    next_url: Option<String>,
}
#[derive(Serialize)]
struct Position<'a> {
    k: &'a str,
    v: i64,
}
fn following(operation: &str) -> bool {
    matches!(
        operation,
        "FollowingArtworks" | "FollowingNovels" | "MyPixivArtworks" | "MyPixivNovels"
    )
}
fn restrict(value: String, operation: &'static str) -> Result<String> {
    match value.as_str() {
        "" => Ok("public".into()),
        "public" | "private" => Ok(value),
        _ => {
            Err(Error::new(Reason::InvalidArgument, operation)
                .with_detail("restrict is unsupported"))
        }
    }
}
impl<T: Transport> Client<T> {
    fn timeline_query(
        &self,
        operation: &'static str,
        query: &mut BTreeMap<String, String>,
        cursor: &Cursor,
    ) -> Result<String> {
        let digest = crate::continuation::query_digest(query);
        let invalid = |detail| Error::new(Reason::InvalidCursor, operation).with_detail(detail);
        if following(operation) {
            self.validate_scoped_cursor(cursor, operation, 1, &digest)?;
        }
        if let Some((key, value)) = crate::ranking::cursor_position(cursor, operation, &digest)? {
            let keys: &[&str] = match operation {
                "LatestArtworks" => &["offset", "max_illust_id"],
                "LatestNovels" => &["max_novel_id"],
                _ => &["offset"],
            };
            if !keys.contains(&key.as_str()) {
                return Err(invalid("cursor continuation kind mismatch"));
            }
            if value <= 0 || (key == "offset" && value > isize::MAX as i64) {
                return Err(invalid(if key == "offset" {
                    "cursor continuation offset must be positive"
                } else {
                    "cursor continuation value must be positive"
                }));
            }
            if operation == "LatestArtworks" && key == "offset" {
                return Err(Error::new(Reason::UpstreamError, operation).with_cause(
                    crate::error::Cause::Redacted("pixiv upstream request failed".into()),
                ));
            }
            query.insert(key, value.to_string());
        }
        Ok(digest)
    }
    fn timeline_next(
        &self,
        operation: &'static str,
        endpoint: &str,
        digest: &str,
        next_url: Option<&str>,
    ) -> Result<Cursor> {
        let Some(raw) = next_url else {
            return Ok(Cursor::default());
        };
        let (allowed, keys): (&[&str], &[&str]) = match operation {
            "LatestArtworks" => (
                &["content_type", "filter", "offset", "max_illust_id"],
                &["max_illust_id", "offset"],
            ),
            "LatestNovels" => (&["filter", "max_novel_id"], &["max_novel_id"]),
            "MyPixivArtworks" | "MyPixivNovels" => (&["offset"], &["offset"]),
            _ => (&["restrict", "offset"], &["offset"]),
        };
        let (key, value) = crate::continuation::next_keyed_value(
            raw,
            endpoint.trim_start_matches('/'),
            allowed,
            keys,
        )
        .filter(|(key, value)| key != "offset" || *value <= isize::MAX as i64)
        .ok_or_else(|| Error::new(Reason::MalformedUpstreamResponse, operation))?;
        if !following(operation) {
            return crate::ranking::value_cursor(operation, digest, &key, Some(value));
        }
        let options = self.scoped_cursor_options(operation)?;
        let payload = serde_json::to_vec(&Position { k: &key, v: value }).map_err(|_| {
            Error::new(Reason::UpstreamError, operation).with_detail("cannot encode cursor")
        })?;
        Cursor::new("pixiv", operation, 1, digest, &payload, options)
    }
    pub async fn my_pixiv_artworks(
        &self,
        request: MyPixivArtworksRequest,
    ) -> Result<Page<Artwork>> {
        self.timeline_artworks(
            "MyPixivArtworks",
            "/v2/illust/mypixiv",
            BTreeMap::new(),
            request.cursor,
        )
        .await
    }
    pub async fn my_pixiv_novels(&self, request: MyPixivNovelsRequest) -> Result<Page<Novel>> {
        self.timeline_novels(
            "MyPixivNovels",
            "/v1/novel/mypixiv",
            BTreeMap::new(),
            request.cursor,
        )
        .await
    }
    pub async fn following_artworks(
        &self,
        request: FollowingArtworksRequest,
    ) -> Result<Page<Artwork>> {
        let operation = "FollowingArtworks";
        let query = BTreeMap::from([("restrict".into(), restrict(request.restrict, operation)?)]);
        self.timeline_artworks(operation, "/v2/illust/follow", query, request.cursor)
            .await
    }
    pub async fn latest_artworks(&self, request: LatestArtworksRequest) -> Result<Page<Artwork>> {
        let operation = "LatestArtworks";
        let content_type = match request.content_type.as_str() {
            "" | "illust" => "illust",
            "manga" => "manga",
            _ => {
                return Err(Error::new(Reason::InvalidArgument, operation)
                    .with_detail("content type is unsupported for latest artworks"));
            }
        };
        let query = BTreeMap::from([("content_type".into(), content_type.into())]);
        self.timeline_artworks(operation, "/v1/illust/new", query, request.cursor)
            .await
    }
    pub async fn following_novels(&self, request: FollowingNovelsRequest) -> Result<Page<Novel>> {
        let operation = "FollowingNovels";
        let query = BTreeMap::from([("restrict".into(), restrict(request.restrict, operation)?)]);
        self.timeline_novels(operation, "/v1/novel/follow", query, request.cursor)
            .await
    }
    pub async fn latest_novels(&self, request: LatestNovelsRequest) -> Result<Page<Novel>> {
        self.timeline_novels(
            "LatestNovels",
            "/v1/novel/new",
            BTreeMap::from([("filter".into(), "for_android".into())]),
            request.cursor,
        )
        .await
    }
    async fn timeline_artworks(
        &self,
        operation: &'static str,
        endpoint: &str,
        mut query: BTreeMap<String, String>,
        cursor: Cursor,
    ) -> Result<Page<Artwork>> {
        let digest = self.timeline_query(operation, &mut query, &cursor)?;
        if operation == "LatestArtworks" {
            query.insert("filter".into(), "for_android".into());
        }
        let body = self
            .get(endpoint, query.into_iter().collect(), operation)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let envelope: ArtworksEnvelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let values = envelope.illusts.ok_or_else(malformed)?;
        crate::artwork::validate_list(&values, operation)?;
        if operation == "MyPixivArtworks"
            && values.iter().any(|value| {
                value
                    .get("user")
                    .and_then(|user| user.get("id"))
                    .and_then(Value::as_i64)
                    .is_none_or(|id| id <= 0)
            })
        {
            return Err(malformed());
        }
        let next =
            self.timeline_next(operation, endpoint, &digest, envelope.next_url.as_deref())?;
        let mut items = Vec::with_capacity(values.len());
        for value in values {
            let item = crate::artwork::map(value, "Artwork", false, &self.resource_policy)?;
            self.remember_artwork(&item);
            items.push(item);
        }
        Ok(Page { items, next })
    }
    async fn timeline_novels(
        &self,
        operation: &'static str,
        endpoint: &str,
        mut query: BTreeMap<String, String>,
        cursor: Cursor,
    ) -> Result<Page<Novel>> {
        let digest = self.timeline_query(operation, &mut query, &cursor)?;
        let body = self
            .get(endpoint, query.into_iter().collect(), operation)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let envelope: NovelsEnvelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let values = crate::novel::decode_list(envelope.novels.ok_or_else(malformed)?, operation)?;
        let next =
            self.timeline_next(operation, endpoint, &digest, envelope.next_url.as_deref())?;
        let mut items = Vec::with_capacity(values.len());
        for value in values {
            let item = value.map(&self.resource_policy)?;
            self.remember_resource(&item.cover.resource);
            self.remember_resource(&item.user.profile_image.resource);
            items.push(item);
        }
        Ok(Page { items, next })
    }
}
