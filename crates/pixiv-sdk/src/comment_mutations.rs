pub use crate::models::CommentMutationResult;
use crate::{Client, Error, Reason, Result, transport::Transport};
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PostArtworkCommentRequest {
    pub artwork_id: i64,
    pub comment: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReplyArtworkCommentRequest {
    pub artwork_id: i64,
    pub comment: String,
    pub parent_comment_id: i64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StampArtworkCommentRequest {
    pub artwork_id: i64,
    pub comment: String,
    pub stamp_id: i64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeleteArtworkCommentRequest {
    pub comment_id: i64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PostNovelCommentRequest {
    pub novel_id: i64,
    pub comment: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReplyNovelCommentRequest {
    pub novel_id: i64,
    pub comment: String,
    pub parent_comment_id: i64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StampNovelCommentRequest {
    pub novel_id: i64,
    pub comment: String,
    pub stamp_id: i64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeleteNovelCommentRequest {
    pub comment_id: i64,
}
impl<T: Transport> Client<T> {
    pub async fn post_artwork_comment(
        &self,
        request: PostArtworkCommentRequest,
    ) -> Result<CommentMutationResult> {
        self.add_comment(
            "PostArtworkComment",
            ("/v1/illust/comment/add", "illust_id"),
            request.artwork_id,
            request.comment,
            None,
        )
        .await
    }
    pub async fn reply_artwork_comment(
        &self,
        request: ReplyArtworkCommentRequest,
    ) -> Result<CommentMutationResult> {
        self.add_comment(
            "ReplyArtworkComment",
            ("/v1/illust/comment/add", "illust_id"),
            request.artwork_id,
            request.comment,
            Some(("parent_comment_id", request.parent_comment_id)),
        )
        .await
    }
    pub async fn stamp_artwork_comment(
        &self,
        request: StampArtworkCommentRequest,
    ) -> Result<CommentMutationResult> {
        self.add_comment(
            "StampArtworkComment",
            ("/v1/illust/comment/add", "illust_id"),
            request.artwork_id,
            request.comment,
            Some(("stamp_id", request.stamp_id)),
        )
        .await
    }
    pub async fn delete_artwork_comment(&self, request: DeleteArtworkCommentRequest) -> Result<()> {
        if request.comment_id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, "DeleteArtworkComment")
                .with_detail("comment ID must be positive"));
        }
        self.post_form(
            "/v1/illust/comment/delete",
            vec![("comment_id".into(), request.comment_id.to_string())],
            "DeleteArtworkComment",
        )
        .await
    }
    pub async fn post_novel_comment(
        &self,
        request: PostNovelCommentRequest,
    ) -> Result<CommentMutationResult> {
        self.add_comment(
            "PostNovelComment",
            ("/v1/novel/comment/add", "novel_id"),
            request.novel_id,
            request.comment,
            None,
        )
        .await
    }
    pub async fn reply_novel_comment(
        &self,
        request: ReplyNovelCommentRequest,
    ) -> Result<CommentMutationResult> {
        self.add_comment(
            "ReplyNovelComment",
            ("/v1/novel/comment/add", "novel_id"),
            request.novel_id,
            request.comment,
            Some(("parent_comment_id", request.parent_comment_id)),
        )
        .await
    }
    pub async fn stamp_novel_comment(
        &self,
        request: StampNovelCommentRequest,
    ) -> Result<CommentMutationResult> {
        self.add_comment(
            "StampNovelComment",
            ("/v1/novel/comment/add", "novel_id"),
            request.novel_id,
            request.comment,
            Some(("stamp_id", request.stamp_id)),
        )
        .await
    }
    pub async fn delete_novel_comment(&self, request: DeleteNovelCommentRequest) -> Result<()> {
        if request.comment_id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, "DeleteNovelComment")
                .with_detail("comment ID must be positive"));
        }
        self.post_form(
            "/v1/novel/comment/delete",
            vec![("comment_id".into(), request.comment_id.to_string())],
            "DeleteNovelComment",
        )
        .await
    }
    async fn add_comment(
        &self,
        operation: &'static str,
        endpoint: (&str, &str),
        id: i64,
        comment: String,
        extra: Option<(&str, i64)>,
    ) -> Result<CommentMutationResult> {
        let (path, field) = endpoint;
        let invalid =
            |detail: &str| Error::new(Reason::InvalidArgument, operation).with_detail(detail);
        if id <= 0 {
            return Err(invalid(if field == "illust_id" {
                "artwork ID must be positive"
            } else {
                "novel ID must be positive"
            }));
        }
        if !matches!(extra, Some(("stamp_id", _))) && comment.is_empty() {
            return Err(invalid("comment body must not be empty"));
        }
        let mut form = vec![(field.into(), id.to_string()), ("comment".into(), comment)];
        if let Some((field, id)) = extra {
            if id <= 0 {
                return Err(invalid(if field == "stamp_id" {
                    "stamp ID must be positive"
                } else {
                    "parent comment ID must be positive"
                }));
            }
            form.push((field.into(), id.to_string()));
        }
        let raw = self.post_form_json(path, form, operation).await?;
        let value = crate::user_wire::decode_comment_mutation(&raw, operation)?;
        let id = value
            .get("comment_id")
            .and_then(serde_json::Value::as_i64)
            .or_else(|| {
                value
                    .get("comment")
                    .and_then(|comment| comment.get("id"))
                    .and_then(serde_json::Value::as_i64)
            })
            .filter(|id| *id > 0)
            .ok_or_else(|| Error::new(Reason::MalformedUpstreamResponse, operation))?;
        Ok(CommentMutationResult { comment_id: id })
    }
}
