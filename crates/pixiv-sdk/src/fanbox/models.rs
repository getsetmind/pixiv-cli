use serde::Serialize;
use std::fmt;

#[derive(Clone, Default, Serialize)]
pub struct SessionCredentials {
    #[serde(skip)]
    pub fanbox_sessid: String,
}
impl fmt::Display for SessionCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fanbox.SessionCredentials{}")
    }
}
impl fmt::Debug for SessionCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct User {
    pub user_id: i64,
    pub display_name: String,
    pub creator_id: String,
    pub creator_status: String,
    pub is_creator: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct UserDto {
    pub user_id: i64,
    pub display_name: String,
    pub creator_id: String,
    pub creator_status: String,
    pub is_creator: bool,
}
impl User {
    pub fn to_dto(&self) -> UserDto {
        UserDto {
            user_id: self.user_id,
            display_name: self.display_name.clone(),
            creator_id: self.creator_id.clone(),
            creator_status: self.creator_status.clone(),
            is_creator: self.is_creator,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct CurrentUserRequest {}

use crate::{cursor::Cursor, resource::Resource};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    #[default]
    #[serde(rename = "")]
    Empty,
    Image,
    File,
}
impl AssetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "",
            Self::Image => "image",
            Self::File => "file",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostBlockKind {
    #[default]
    #[serde(rename = "")]
    Empty,
    Image,
    File,
    Article,
    Video,
    Unknown,
}
impl PostBlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "",
            Self::Image => "image",
            Self::File => "file",
            Self::Article => "article",
            Self::Video => "video",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageResource {
    pub resource: Resource,
    pub variant: String,
    pub width: i64,
    pub height: i64,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileResource {
    pub resource: Resource,
    pub name: String,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreatorSummary {
    pub id: String,
    pub name: String,
    pub icon: ImageResource,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Creator {
    pub id: String,
    pub name: String,
    pub icon: ImageResource,
    pub has_adult_content: bool,
    pub is_following: bool,
    pub cover: ImageResource,
    pub plan_fee: i64,
    pub has_supporting_plan: bool,
}
impl Creator {
    pub fn summary(&self) -> CreatorSummary {
        CreatorSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            icon: self.icon.clone(),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PostImageBlock {
    pub resource: Resource,
    pub caption: String,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PostFileBlock {
    pub resource: Resource,
    pub name: String,
    pub caption: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PostArticleBlock {
    pub text: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PostVideoEmbed {
    pub provider: String,
    pub content_id: String,
    pub canonical_url: String,
    pub title: String,
    pub thumbnail_url: String,
    pub video_id: String,
    pub embedded_data: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PostUnknownBlock {
    pub raw_type: String,
    pub payload: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PostBlock {
    pub kind: PostBlockKind,
    pub image: Option<PostImageBlock>,
    pub file: Option<PostFileBlock>,
    pub article: Option<PostArticleBlock>,
    pub video: Option<PostVideoEmbed>,
    pub unknown: Option<PostUnknownBlock>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asset {
    pub id: String,
    pub kind: AssetKind,
    pub name: String,
    pub resource: Resource,
    pub thumbnail: ImageResource,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PostBody {
    pub text: String,
    pub blocks: Option<Vec<PostBlock>>,
    pub assets: Option<Vec<Asset>>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Post {
    pub id: String,
    pub title: String,
    pub published_at: chrono::DateTime<chrono::Utc>,
    pub creator_id: String,
    pub fee_required: i64,
    pub is_restricted: bool,
    pub is_pinned: bool,
    pub restricted_for: i64,
    pub comment_count: i64,
    pub cover: ImageResource,
    pub body: Option<PostBody>,
}
impl Default for Post {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            published_at: chrono::DateTime::parse_from_rfc3339("0001-01-01T00:00:00Z")
                .expect("fixed zero time")
                .to_utc(),
            creator_id: String::new(),
            fee_required: 0,
            is_restricted: false,
            is_pinned: false,
            restricted_for: 0,
            comment_count: 0,
            cover: ImageResource::default(),
            body: None,
        }
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreatorTag {
    pub name: String,
    pub url: String,
}
#[derive(Clone, Debug, Default)]
pub struct CreatorRequest {
    pub creator_id: String,
}
pub type CreatorListKind = String;
pub const CREATOR_LIST_SUPPORTING: &str = "supporting";
pub const CREATOR_LIST_FOLLOWING: &str = "following";

#[derive(Clone, Debug, Default)]
pub struct CreatorsRequest {
    pub kind: CreatorListKind,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default)]
pub struct CreatorTagsRequest {
    pub creator_id: String,
}
#[derive(Clone, Debug, Default)]
pub struct CreatorPostsRequest {
    pub creator_id: String,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default)]
pub struct TaggedPostsRequest {
    pub creator_id: String,
    pub tag: String,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default)]
pub struct PostRequest {
    pub post_id: String,
}
#[derive(Clone, Debug, Default)]
pub struct HomeRequest {
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default)]
pub struct SupportingRequest {
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default)]
pub struct ResolveURLRequest {
    pub raw_url: String,
}
