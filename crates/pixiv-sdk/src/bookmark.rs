use crate::{Client, Error, Reason, Result, models::ArtworkBookmarkDetail, transport::Transport};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArtworkBookmarkRequest {
    pub artwork_id: i64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NovelBookmarkRequest {
    pub novel_id: i64,
}

#[derive(Default, Deserialize)]
struct Envelope {
    bookmark_detail: Option<Detail>,
}
#[derive(Default, Deserialize)]
struct Detail {
    is_bookmarked: Option<bool>,
    restrict: Option<String>,
    tags: Option<Vec<Option<Tag>>>,
}
#[derive(Default, Deserialize)]
struct Tag {
    name: Option<String>,
    is_registered: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserNovelBookmarkTagsRequest {
    pub user_id: i64,
    pub restrict: crate::mutation::Restrict,
    pub cursor: crate::cursor::Cursor,
}
pub type UserArtworkBookmarkTagsRequest = UserNovelBookmarkTagsRequest;

#[derive(Default, Deserialize)]
struct Continuation {
    k: Option<String>,
    v: Option<i64>,
    s: Option<i64>,
    p: Option<std::collections::BTreeMap<String, Option<Vec<String>>>>,
}

fn tag_query_digest(user_id: i64, restrict: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([("restrict", restrict), ("user_id", &user_id.to_string())])
        .finish();
    format!("{:x}", Sha256::digest(format!("{query}&").as_bytes()))
}

fn tag_offset(
    cursor: &crate::cursor::Cursor,
    operation: &'static str,
    digest: &str,
) -> Result<i64> {
    if cursor.is_zero() {
        return Ok(0);
    }
    let invalid = |detail| Error::new(Reason::InvalidCursor, operation).with_detail(detail);
    cursor
        .validate("pixiv", operation, 1, digest)
        .map_err(|_| invalid("cursor does not match this operation and query"))?;
    let payload = cursor
        .payload()
        .map_err(|_| invalid("cursor payload is unavailable"))?;
    let state: Option<Continuation> =
        serde_json::from_slice(&payload).map_err(|_| invalid("cursor payload is malformed"))?;
    let state = state.unwrap_or_default();
    let key = state.k.unwrap_or_default();
    let value = state.v.unwrap_or_default();
    let params = state.p.unwrap_or_default();
    if value < 0
        || state.s.unwrap_or_default() < 0
        || (key.is_empty() && params.is_empty())
        || (!key.is_empty() && !params.is_empty())
    {
        return Err(invalid("cursor payload is malformed"));
    }
    if key != "offset" {
        return Err(invalid("cursor continuation kind mismatch"));
    }
    if value <= 0 {
        return Err(invalid("cursor continuation offset must be positive"));
    }
    Ok(value)
}

fn next_tag_offset(raw: &str) -> Option<i64> {
    let (scheme, remainder) = raw.split_once("://")?;
    if scheme != "https" || raw.bytes().any(|byte| byte < 32 || byte == 127) {
        return None;
    }
    let (authority, path) = remainder.split_once('/')?;
    if !matches!(authority, "app-api.pixiv.net" | "app-api.pixiv.net:") {
        return None;
    }
    let (path, fragment) = path.split_once('#').map_or((path, ""), |(p, f)| (p, f));
    if !fragment.is_empty() {
        return None;
    }
    let (path, query) = path.split_once('?')?;
    if path != "v1/user/bookmark-tags/illust" || query.contains(';') {
        return None;
    }
    let bytes = query.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (bytes.get(index + 1).is_none_or(|b| !b.is_ascii_hexdigit())
                || bytes.get(index + 2).is_none_or(|b| !b.is_ascii_hexdigit()))
        {
            return None;
        }
    }
    let mut entries = std::collections::BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(bytes) {
        if !matches!(key.as_ref(), "offset" | "user_id" | "restrict")
            || entries
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return None;
        }
    }
    let value = entries.get("offset")?.parse::<i64>().ok()?;
    (value > 0).then_some(value)
}

#[derive(Deserialize)]
struct TagsEnvelope {
    bookmark_tags: Option<Vec<Option<CountedTag>>>,
    next_url: Option<String>,
}
#[derive(Default, Deserialize)]
struct CountedTag {
    name: Option<String>,
    count: Option<i64>,
}

impl<T: Transport> Client<T> {
    pub async fn user_artwork_bookmark_tags(
        &self,
        request: UserArtworkBookmarkTagsRequest,
    ) -> Result<crate::cursor::Page<crate::models::BookmarkTag>> {
        let operation = "UserArtworkBookmarkTags";
        if request.user_id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, operation)
                .with_detail("user ID must be positive"));
        }
        if !matches!(request.restrict.as_str(), "" | "public" | "private") {
            return Err(Error::new(Reason::InvalidArgument, operation)
                .with_detail("restrict is unsupported"));
        }
        let digest = tag_query_digest(request.user_id, &request.restrict);
        let offset = tag_offset(&request.cursor, operation, &digest)?;
        let mut query = vec![
            ("user_id".into(), request.user_id.to_string()),
            ("restrict".into(), request.restrict),
        ];
        if offset > 0 {
            query.push(("offset".into(), offset.to_string()));
        }
        query.sort();
        let body = self
            .get("/v1/user/bookmark-tags/illust", query, operation)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let envelope: Option<TagsEnvelope> =
            serde_json::from_value(body).map_err(|_| malformed())?;
        let envelope = envelope.ok_or_else(malformed)?;
        let items = envelope
            .bookmark_tags
            .ok_or_else(malformed)?
            .into_iter()
            .map(|tag| {
                let tag = tag.unwrap_or_default();
                let name = tag.name.unwrap_or_default();
                if name.is_empty() {
                    return Err(malformed());
                }
                Ok(crate::models::BookmarkTag {
                    name,
                    count: tag.count.unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let next = match envelope.next_url {
            None => crate::cursor::Cursor::default(),
            Some(raw) => {
                let offset = next_tag_offset(&raw).ok_or_else(malformed)?;
                let payload = format!("{{\"k\":\"offset\",\"v\":{offset}}}");
                crate::cursor::Cursor::new(
                    "pixiv",
                    operation,
                    1,
                    &digest,
                    payload.as_bytes(),
                    Default::default(),
                )?
            }
        };
        Ok(crate::cursor::Page { items, next })
    }
    pub async fn user_novel_bookmark_tags(
        &self,
        request: UserNovelBookmarkTagsRequest,
    ) -> Result<crate::cursor::Page<crate::models::BookmarkTag>> {
        let operation = "UserNovelBookmarkTags";
        if request.user_id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, operation)
                .with_detail("user ID must be positive"));
        }
        if !matches!(request.restrict.as_str(), "" | "public" | "private") {
            return Err(Error::new(Reason::InvalidArgument, operation)
                .with_detail("restrict is unsupported"));
        }
        if !request.cursor.is_zero() {
            return Err(Error::new(Reason::InvalidCursor, operation)
                .with_detail("novel bookmark tags continuation is not supported"));
        }
        let body = self
            .get(
                "/v1/user/bookmark-tags/novel",
                vec![
                    ("restrict".into(), request.restrict),
                    ("user_id".into(), request.user_id.to_string()),
                ],
                operation,
            )
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let envelope: Option<TagsEnvelope> =
            serde_json::from_value(body).map_err(|_| malformed())?;
        let envelope = envelope.ok_or_else(malformed)?;
        if envelope.next_url.is_some() {
            return Err(malformed());
        }
        let tags = envelope.bookmark_tags.ok_or_else(malformed)?;
        let items = tags
            .into_iter()
            .map(|tag| {
                let tag = tag.unwrap_or_default();
                let name = tag.name.unwrap_or_default();
                if name.is_empty() {
                    return Err(malformed());
                }
                Ok(crate::models::BookmarkTag {
                    name,
                    count: tag.count.unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(crate::cursor::Page {
            items,
            next: Default::default(),
        })
    }
    pub async fn artwork_bookmark(
        &self,
        request: ArtworkBookmarkRequest,
    ) -> Result<ArtworkBookmarkDetail> {
        self.bookmark_detail(
            request.artwork_id,
            "ArtworkBookmark",
            ("/v2/illust/bookmark/detail", "illust_id", "artwork"),
        )
        .await
    }
    pub async fn novel_bookmark(
        &self,
        request: NovelBookmarkRequest,
    ) -> Result<crate::models::NovelBookmarkDetail> {
        self.bookmark_detail(
            request.novel_id,
            "NovelBookmark",
            ("/v2/novel/bookmark/detail", "novel_id", "novel"),
        )
        .await
    }
    async fn bookmark_detail(
        &self,
        id: i64,
        operation: &'static str,
        endpoint: (&str, &str, &str),
    ) -> Result<ArtworkBookmarkDetail> {
        let (path, field, label) = endpoint;
        if id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, operation)
                .with_detail(format!("{label} ID must be positive")));
        }
        let body = match self
            .get(path, vec![(field.into(), id.to_string())], operation)
            .await
        {
            Ok(body) => body,
            Err(error) if error.code == Reason::NotFound && error.http_status == Some(404) => {
                return Ok(ArtworkBookmarkDetail::default());
            }
            Err(error) => return Err(error),
        };
        let envelope: Option<Envelope> = serde_json::from_value(body)
            .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, operation))?;
        let Some(detail) = envelope.and_then(|value| value.bookmark_detail) else {
            return Ok(ArtworkBookmarkDetail::default());
        };
        if detail.is_bookmarked == Some(false) {
            return Ok(ArtworkBookmarkDetail::default());
        }
        Ok(ArtworkBookmarkDetail {
            restrict: detail.restrict.unwrap_or_default(),
            tags: detail
                .tags
                .unwrap_or_default()
                .into_iter()
                .flatten()
                .filter(|tag| tag.is_registered.unwrap_or_default())
                .map(|tag| tag.name.unwrap_or_default())
                .collect(),
        })
    }
}
