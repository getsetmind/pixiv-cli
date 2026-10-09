use crate::{
    Client, Error, Reason, Result,
    cursor::{Cursor, Page},
    models::{Artwork, Novel},
    ranking::{apply_offset, next_cursor},
    transport::Transport,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

pub type UserArtworkKind = String;
pub const USER_ARTWORK_KIND_ILLUSTRATION: &str = "illustration";
pub const USER_ARTWORK_KIND_ILLUST: &str = "illust";
pub const USER_ARTWORK_KIND_MANGA: &str = "manga";
pub const USER_ARTWORK_KIND_UGOIRA: &str = "ugoira";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserArtworksRequest {
    pub user_id: i64,
    pub kind: UserArtworkKind,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserNovelsRequest {
    pub user_id: i64,
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
fn validate_user_id(user_id: i64, operation: &'static str) -> Result<()> {
    if user_id <= 0 {
        return Err(
            Error::new(Reason::InvalidArgument, operation).with_detail("user ID must be positive")
        );
    }
    Ok(())
}
fn offset(
    next_url: Option<&str>,
    operation: &'static str,
    endpoint: &str,
    keys: &[&str],
) -> Result<Option<i64>> {
    next_url
        .map(|raw| {
            crate::continuation::next_offset(raw, endpoint, keys)
                .filter(|offset| *offset <= isize::MAX as i64)
                .ok_or_else(|| Error::new(Reason::MalformedUpstreamResponse, operation))
        })
        .transpose()
}
impl<T: Transport> Client<T> {
    pub async fn user_artworks(&self, request: UserArtworksRequest) -> Result<Page<Artwork>> {
        let operation = "UserArtworks";
        validate_user_id(request.user_id, operation)?;
        let kind = match request.kind.as_str() {
            "" | "illustration" | "illust" => "illust",
            "manga" => "manga",
            "ugoira" => "ugoira",
            _ => {
                return Err(Error::new(Reason::InvalidArgument, operation)
                    .with_detail("artwork kind is unsupported"));
            }
        };
        let mut query = BTreeMap::from([
            ("user_id".into(), request.user_id.to_string()),
            ("type".into(), kind.into()),
        ]);
        let digest = crate::continuation::query_digest(&query);
        apply_offset(&request.cursor, operation, &digest, &mut query)?;
        let body = self
            .get("/v1/user/illusts", query.into_iter().collect(), operation)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let envelope: ArtworksEnvelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let values = envelope.illusts.ok_or_else(malformed)?;
        crate::artwork::validate_list(&values, operation)?;
        let offset = offset(
            envelope.next_url.as_deref(),
            operation,
            "v1/user/illusts",
            &["user_id", "type", "offset"],
        )?;
        let mut items = Vec::with_capacity(values.len());
        for value in values {
            let artwork = crate::artwork::map(value, "Artwork", false, &self.resource_policy)?;
            self.remember_artwork(&artwork);
            items.push(artwork);
        }
        Ok(Page {
            items,
            next: next_cursor(operation, &digest, offset)?,
        })
    }
    pub async fn user_novels(&self, request: UserNovelsRequest) -> Result<Page<Novel>> {
        let operation = "UserNovels";
        validate_user_id(request.user_id, operation)?;
        let mut query = BTreeMap::from([("user_id".into(), request.user_id.to_string())]);
        let digest = crate::continuation::query_digest(&query);
        apply_offset(&request.cursor, operation, &digest, &mut query)?;
        query.insert("filter".into(), "for_android".into());
        let body = self
            .get("/v1/user/novels", query.into_iter().collect(), operation)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let envelope: NovelsEnvelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let wire = crate::novel::decode_list(envelope.novels.ok_or_else(malformed)?, operation)?;
        let offset = offset(
            envelope.next_url.as_deref(),
            operation,
            "v1/user/novels",
            &["user_id", "filter", "offset"],
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
            next: next_cursor(operation, &digest, offset)?,
        })
    }
}
