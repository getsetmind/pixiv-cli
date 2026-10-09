use crate::{
    Client, Error, Reason, Result,
    artwork::WireUser,
    cursor::Cursor,
    models::{Comment, CommentAccessControl, CommentPage},
    transport::Transport,
};
use chrono::DateTime;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArtworkCommentsRequest {
    pub artwork_id: i64,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NovelCommentsRequest {
    pub novel_id: i64,
    pub cursor: Cursor,
}

#[derive(Default, Deserialize)]
struct WireComment {
    id: Option<i64>,
    user: Option<WireUser>,
    comment: Option<String>,
    caption: Option<String>,
    date: Option<String>,
    created_at: Option<String>,
    parent_comment: Option<Box<WireComment>>,
}
#[derive(Default, Deserialize)]
struct WireAccess {
    can_comment: Option<bool>,
    is_locked: Option<bool>,
}
#[derive(Deserialize)]
struct Envelope {
    comments: Option<Vec<Option<WireComment>>>,
    next_url: Option<String>,
    total_comments: Option<i64>,
    comment_access_control: Option<i64>,
    access_control: Option<WireAccess>,
}
impl WireComment {
    fn valid(&self) -> bool {
        self.id.unwrap_or_default() > 0
            && self
                .parent_comment
                .as_ref()
                .is_none_or(|parent| parent.valid())
    }
    fn map(self, policy: &crate::artwork::ResourcePolicy) -> Result<Comment> {
        let date = self
            .date
            .filter(|date| !date.is_empty())
            .or(self.created_at)
            .unwrap_or_default();
        let created_at = DateTime::parse_from_rfc3339(&date)
            .map_err(|_| {
                Error::new(Reason::MalformedUpstreamResponse, "Comment")
                    .with_detail("invalid comment time")
            })?
            .to_utc();
        Ok(Comment {
            id: self.id.unwrap_or_default(),
            user: self.user.unwrap_or_default().map(policy),
            body: self
                .comment
                .filter(|body| !body.is_empty())
                .or(self.caption)
                .unwrap_or_default(),
            created_at,
            parent: self
                .parent_comment
                .map(|parent| parent.map(policy).map(Box::new))
                .transpose()?,
        })
    }
}
impl<T: Transport> Client<T> {
    pub async fn artwork_comments(&self, request: ArtworkCommentsRequest) -> Result<CommentPage> {
        self.comments(
            request.artwork_id,
            request.cursor,
            "ArtworkComments",
            "illust_id",
            "/v3/illust/comments",
            "artwork ID must be positive",
        )
        .await
    }
    pub async fn novel_comments(&self, request: NovelCommentsRequest) -> Result<CommentPage> {
        self.comments(
            request.novel_id,
            request.cursor,
            "NovelComments",
            "novel_id",
            "/v2/novel/comments",
            "novel ID must be positive",
        )
        .await
    }
    async fn comments(
        &self,
        id: i64,
        cursor: Cursor,
        operation: &'static str,
        key: &str,
        endpoint: &str,
        detail: &'static str,
    ) -> Result<CommentPage> {
        if id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, operation).with_detail(detail));
        }
        let mut query = BTreeMap::from([(key.to_owned(), id.to_string())]);
        let digest = crate::continuation::query_digest(&query);
        crate::ranking::apply_offset(&cursor, operation, &digest, &mut query)?;
        let raw = self
            .get_json(endpoint, query.into_iter().collect(), operation)
            .await?;
        let body = crate::user_wire::decode(&raw, operation)?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let envelope: Envelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let values = envelope.comments.ok_or_else(malformed)?;
        if values
            .iter()
            .any(|value| value.as_ref().is_none_or(|value| !value.valid()))
        {
            return Err(malformed());
        }
        let offset = envelope
            .next_url
            .as_deref()
            .map(|url| {
                crate::continuation::next_offset(
                    url,
                    endpoint.trim_start_matches('/'),
                    &["offset", key],
                )
                .filter(|value| *value <= isize::MAX as i64)
                .ok_or_else(malformed)
            })
            .transpose()?;
        let mut items = Vec::with_capacity(values.len());
        for value in values {
            let item = value
                .expect("validated comment")
                .map(&self.resource_policy)?;
            self.remember_comment(&item);
            items.push(item);
        }
        let access_control = if let Some(value) = envelope.comment_access_control {
            Some(CommentAccessControl {
                numeric_value: Some(value),
                ..Default::default()
            })
        } else {
            envelope.access_control.map(|access| CommentAccessControl {
                can_comment: access.can_comment.unwrap_or_default(),
                is_locked: access.is_locked.unwrap_or_default(),
                numeric_value: None,
            })
        };
        Ok(CommentPage {
            items,
            next: crate::ranking::next_cursor(operation, &digest, offset)?,
            total: envelope.total_comments,
            access_control,
        })
    }
    fn remember_comment(&self, comment: &Comment) {
        self.remember_resource(&comment.user.profile_image.resource);
        if let Some(parent) = &comment.parent {
            self.remember_comment(parent);
        }
    }
}
