use crate::{Client, Error, Reason, Result, models::ArtworkBookmarkDetail, transport::Transport};
use serde::Deserialize;

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
                    ("user_id".into(), request.user_id.to_string()),
                    ("restrict".into(), request.restrict),
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
