use crate::{
    Client, Error, Reason, Result,
    artwork::WireUser,
    cursor::{Cursor, Page},
    models::UserPreview,
    ranking::apply_offset,
    transport::Transport,
};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserFollowingRequest {
    pub user_id: i64,
    pub restrict: String,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserFollowersRequest {
    pub user_id: i64,
    pub restrict: String,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelatedUsersRequest {
    pub user_id: i64,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserBlockedUsersRequest {
    pub user_id: i64,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MyPixivUsersRequest {
    pub cursor: Cursor,
}
#[derive(Deserialize)]
struct Preview {
    user: Option<WireUser>,
}
#[derive(Deserialize)]
struct Envelope {
    user_previews: Option<Vec<Option<Preview>>>,
    users: Option<Vec<Option<Preview>>>,
    next_url: Option<String>,
}
fn validate(user_id: i64, restrict: Option<&str>, operation: &'static str) -> Result<()> {
    if user_id <= 0 {
        return Err(
            Error::new(Reason::InvalidArgument, operation).with_detail("user ID must be positive")
        );
    }
    if restrict.is_some_and(|restrict| !matches!(restrict, "" | "public" | "private")) {
        return Err(
            Error::new(Reason::InvalidArgument, operation).with_detail("restrict is unsupported")
        );
    }
    Ok(())
}
impl<T: Transport> Client<T> {
    pub async fn my_pixiv_users(&self, request: MyPixivUsersRequest) -> Result<Page<UserPreview>> {
        if self.user_id <= 0 {
            return Err(Error::new(Reason::Unauthorized, "MyPixivUsers")
                .with_detail("current user identity is unknown"));
        }
        self.user_relationship(
            "MyPixivUsers",
            "/v1/user/mypixiv",
            self.user_id,
            None,
            request.cursor,
        )
        .await
    }
    pub async fn user_following(&self, request: UserFollowingRequest) -> Result<Page<UserPreview>> {
        validate(request.user_id, Some(&request.restrict), "UserFollowing")?;
        self.user_relationship(
            "UserFollowing",
            "/v1/user/following",
            request.user_id,
            Some(request.restrict),
            request.cursor,
        )
        .await
    }
    pub async fn user_followers(&self, request: UserFollowersRequest) -> Result<Page<UserPreview>> {
        validate(request.user_id, Some(&request.restrict), "UserFollowers")?;
        self.user_relationship(
            "UserFollowers",
            "/v1/user/follower",
            request.user_id,
            Some(request.restrict),
            request.cursor,
        )
        .await
    }
    pub async fn related_users(&self, request: RelatedUsersRequest) -> Result<Page<UserPreview>> {
        validate(request.user_id, None, "RelatedUsers")?;
        self.user_relationship(
            "RelatedUsers",
            "/v1/user/related",
            request.user_id,
            None,
            request.cursor,
        )
        .await
    }
    pub async fn user_blocked_users(
        &self,
        request: UserBlockedUsersRequest,
    ) -> Result<Page<UserPreview>> {
        validate(request.user_id, None, "UserBlockedUsers")?;
        self.user_relationship(
            "UserBlockedUsers",
            "/v2/user/list",
            request.user_id,
            None,
            request.cursor,
        )
        .await
    }
    async fn user_relationship(
        &self,
        operation: &'static str,
        path: &'static str,
        user_id: i64,
        restrict: Option<String>,
        cursor: Cursor,
    ) -> Result<Page<UserPreview>> {
        let user_key = if operation == "RelatedUsers" {
            "seed_user_id"
        } else {
            "user_id"
        };
        let mut query = BTreeMap::from([(user_key.into(), user_id.to_string())]);
        if let Some(restrict) = restrict {
            query.insert(
                "restrict".into(),
                if restrict.is_empty() {
                    "public".into()
                } else {
                    restrict
                },
            );
        }
        let digest_query = if operation == "MyPixivUsers" {
            BTreeMap::new()
        } else {
            query.clone()
        };
        let digest = crate::continuation::query_digest(&digest_query);
        self.validate_scoped_cursor(&cursor, operation, 1, &digest)?;
        apply_offset(&cursor, operation, &digest, &mut query)?;
        let mut allowed = vec!["offset", user_key];
        if query.contains_key("restrict") {
            allowed.push("restrict");
        }
        if matches!(operation, "UserBlockedUsers" | "MyPixivUsers") {
            query.insert("filter".into(), "for_android".into());
            allowed.push("filter");
        }
        let body = self
            .get_json(path, query.into_iter().collect(), operation)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let value = crate::user_wire::decode(&body, operation)?;
        let users_present = value.get("users").is_some();
        let envelope: Envelope = serde_json::from_value(value).map_err(|_| malformed())?;
        let previews = if operation == "UserBlockedUsers" && users_present {
            envelope.users
        } else {
            envelope.user_previews
        }
        .ok_or_else(malformed)?;
        let mut users = Vec::with_capacity(previews.len());
        for preview in previews {
            let user = preview
                .and_then(|preview| preview.user)
                .ok_or_else(malformed)?;
            if user.id.is_none_or(|id| id <= 0) {
                return Err(malformed());
            }
            users.push(user);
        }
        let offset = envelope
            .next_url
            .as_deref()
            .map(|raw| {
                crate::continuation::next_offset(raw, path.trim_start_matches('/'), &allowed)
                    .filter(|offset| *offset <= isize::MAX as i64)
                    .ok_or_else(malformed)
            })
            .transpose()?;
        let next = if let Some(offset) = offset {
            let options = self.scoped_cursor_options(operation)?;
            Cursor::new(
                "pixiv",
                operation,
                1,
                &digest,
                format!(r#"{{"k":"offset","v":{offset}}}"#).as_bytes(),
                options,
            )?
        } else {
            Cursor::default()
        };
        let items = users
            .into_iter()
            .map(|user| {
                let user = user.map(&self.resource_policy);
                self.remember_resource(&user.profile_image.resource);
                UserPreview {
                    user,
                    ..Default::default()
                }
            })
            .collect();
        Ok(Page { items, next })
    }
}
