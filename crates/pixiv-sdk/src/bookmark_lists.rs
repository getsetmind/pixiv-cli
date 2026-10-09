use crate::{
    Client, Error, Reason, Result,
    cursor::{Cursor, Page},
    models::{Artwork, Novel},
    ranking::{apply_value, value_cursor},
    transport::Transport,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserArtworkBookmarksRequest {
    pub user_id: i64,
    pub restrict: String,
    pub tag: String,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserNovelBookmarksRequest {
    pub user_id: i64,
    pub restrict: String,
    pub tag: String,
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
fn query(
    user_id: i64,
    restrict: String,
    tag: String,
    operation: &'static str,
) -> Result<BTreeMap<String, String>> {
    if user_id <= 0 {
        return Err(
            Error::new(Reason::InvalidArgument, operation).with_detail("user ID must be positive")
        );
    }
    if !matches!(restrict.as_str(), "" | "public" | "private") {
        return Err(
            Error::new(Reason::InvalidArgument, operation).with_detail("restrict is unsupported")
        );
    }
    let mut query = BTreeMap::from([
        ("user_id".into(), user_id.to_string()),
        ("restrict".into(), restrict),
    ]);
    if !tag.is_empty() {
        query.insert("tag".into(), tag);
    }
    Ok(query)
}
fn next_value(
    next_url: Option<&str>,
    operation: &'static str,
    endpoint: &str,
) -> Result<Option<i64>> {
    next_url
        .map(|raw| {
            crate::continuation::next_value(
                raw,
                endpoint,
                &["user_id", "restrict", "tag", "max_bookmark_id"],
                "max_bookmark_id",
            )
            .ok_or_else(|| Error::new(Reason::MalformedUpstreamResponse, operation))
        })
        .transpose()
}
impl<T: Transport> Client<T> {
    pub async fn user_artwork_bookmarks(
        &self,
        request: UserArtworkBookmarksRequest,
    ) -> Result<Page<Artwork>> {
        let operation = "UserArtworkBookmarks";
        let mut query = query(request.user_id, request.restrict, request.tag, operation)?;
        let digest = crate::continuation::query_digest(&query);
        apply_value(
            &request.cursor,
            operation,
            &digest,
            &mut query,
            "max_bookmark_id",
            "cursor continuation value must be positive",
        )?;
        let body = self
            .get(
                "/v1/user/bookmarks/illust",
                query.into_iter().collect(),
                operation,
            )
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let envelope: ArtworksEnvelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let values = envelope.illusts.ok_or_else(malformed)?;
        crate::artwork::validate_list(&values, operation)?;
        let next = next_value(
            envelope.next_url.as_deref(),
            operation,
            "v1/user/bookmarks/illust",
        )?;
        let mut items = Vec::with_capacity(values.len());
        for value in values {
            let artwork = crate::artwork::map(value, "Artwork", false, &self.resource_policy)?;
            self.remember_artwork(&artwork);
            items.push(artwork);
        }
        Ok(Page {
            items,
            next: value_cursor(operation, &digest, "max_bookmark_id", next)?,
        })
    }
    pub async fn user_novel_bookmarks(
        &self,
        request: UserNovelBookmarksRequest,
    ) -> Result<Page<Novel>> {
        let operation = "UserNovelBookmarks";
        let mut query = query(request.user_id, request.restrict, request.tag, operation)?;
        let digest = crate::continuation::query_digest(&query);
        apply_value(
            &request.cursor,
            operation,
            &digest,
            &mut query,
            "max_bookmark_id",
            "cursor continuation value must be positive",
        )?;
        let body = self
            .get(
                "/v1/user/bookmarks/novel",
                query.into_iter().collect(),
                operation,
            )
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let envelope: NovelsEnvelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let wire = crate::novel::decode_list(envelope.novels.ok_or_else(malformed)?, operation)?;
        let next = next_value(
            envelope.next_url.as_deref(),
            operation,
            "v1/user/bookmarks/novel",
        )?;
        let mut items = Vec::with_capacity(wire.len());
        for item in wire {
            let novel = item.map(&self.resource_policy)?;
            self.remember_resource(&novel.cover.resource);
            self.remember_resource(&novel.user.profile_image.resource);
            items.push(novel);
        }
        Ok(Page {
            items,
            next: value_cursor(operation, &digest, "max_bookmark_id", next)?,
        })
    }
}
