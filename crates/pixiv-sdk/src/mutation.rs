use crate::{Client, Error, Reason, Result, transport::Transport};

pub type Restrict = String;
pub const RESTRICT_PUBLIC: &str = "public";
pub const RESTRICT_PRIVATE: &str = "private";
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddArtworkBookmarkRequest {
    pub artwork_id: i64,
    pub restrict: Restrict,
    pub tags: Vec<String>,
}
pub type AddBookmarkRequest = AddArtworkBookmarkRequest;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoveArtworkBookmarkRequest {
    pub artwork_id: i64,
}
pub type RemoveBookmarkRequest = RemoveArtworkBookmarkRequest;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddNovelBookmarkRequest {
    pub novel_id: i64,
    pub restrict: Restrict,
    pub tags: Vec<String>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoveNovelBookmarkRequest {
    pub novel_id: i64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FollowUserRequest {
    pub user_id: i64,
    pub restrict: Restrict,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnfollowUserRequest {
    pub user_id: i64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetAiArtworkVisibilityRequest {
    pub visible: bool,
}

impl<T: Transport> Client<T> {
    pub async fn add_artwork_bookmark(&self, request: AddArtworkBookmarkRequest) -> Result<()> {
        self.mutate(
            "AddArtworkBookmark",
            ("/v2/illust/bookmark/add", "illust_id"),
            request.artwork_id,
            Some(request.restrict),
            request.tags,
        )
        .await
    }
    pub async fn add_bookmark(&self, request: AddBookmarkRequest) -> Result<()> {
        self.mutate(
            "AddBookmark",
            ("/v2/illust/bookmark/add", "illust_id"),
            request.artwork_id,
            Some(request.restrict),
            request.tags,
        )
        .await
    }
    pub async fn remove_artwork_bookmark(
        &self,
        request: RemoveArtworkBookmarkRequest,
    ) -> Result<()> {
        self.mutate(
            "RemoveArtworkBookmark",
            ("/v1/illust/bookmark/delete", "illust_id"),
            request.artwork_id,
            None,
            vec![],
        )
        .await
    }
    pub async fn remove_bookmark(&self, request: RemoveBookmarkRequest) -> Result<()> {
        self.mutate(
            "RemoveBookmark",
            ("/v1/illust/bookmark/delete", "illust_id"),
            request.artwork_id,
            None,
            vec![],
        )
        .await
    }
    pub async fn add_novel_bookmark(&self, request: AddNovelBookmarkRequest) -> Result<()> {
        self.mutate(
            "AddNovelBookmark",
            ("/v2/novel/bookmark/add", "novel_id"),
            request.novel_id,
            Some(request.restrict),
            request.tags,
        )
        .await
    }
    pub async fn remove_novel_bookmark(&self, request: RemoveNovelBookmarkRequest) -> Result<()> {
        self.mutate(
            "RemoveNovelBookmark",
            ("/v1/novel/bookmark/delete", "novel_id"),
            request.novel_id,
            None,
            vec![],
        )
        .await
    }
    pub async fn follow_user(&self, request: FollowUserRequest) -> Result<()> {
        self.mutate(
            "FollowUser",
            ("/v1/user/follow/add", "user_id"),
            request.user_id,
            Some(request.restrict),
            vec![],
        )
        .await
    }
    pub async fn unfollow_user(&self, request: UnfollowUserRequest) -> Result<()> {
        self.mutate(
            "UnfollowUser",
            ("/v1/user/follow/delete", "user_id"),
            request.user_id,
            None,
            vec![],
        )
        .await
    }
    pub async fn set_ai_artwork_visibility(
        &self,
        _request: SetAiArtworkVisibilityRequest,
    ) -> Result<()> {
        Err(
            Error::new(Reason::ContentUnavailable, "SetAIArtworkVisibility")
                .with_detail("AI artwork visibility is unsupported by the current App API"),
        )
    }
    async fn mutate(
        &self,
        operation: &'static str,
        endpoint: (&str, &str),
        id: i64,
        restrict: Option<String>,
        tags: Vec<String>,
    ) -> Result<()> {
        let (path, field) = endpoint;
        if id <= 0 {
            let label = match field {
                "illust_id" => "artwork",
                "novel_id" => "novel",
                _ => "user",
            };
            return Err(Error::new(Reason::InvalidArgument, operation)
                .with_detail(format!("{label} ID must be positive")));
        }
        let mut form = vec![(field.into(), id.to_string())];
        if let Some(restrict) = restrict {
            let restrict = if restrict.is_empty() {
                RESTRICT_PUBLIC.into()
            } else {
                restrict
            };
            if !matches!(restrict.as_str(), RESTRICT_PUBLIC | RESTRICT_PRIVATE) {
                return Err(Error::new(Reason::InvalidArgument, operation)
                    .with_detail("restrict is unsupported"));
            }
            form.push(("restrict".into(), restrict));
        }
        form.extend(tags.into_iter().map(|tag| ("tags[]".into(), tag)));
        self.post_form(path, form, operation).await
    }
}
