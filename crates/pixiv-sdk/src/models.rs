use crate::resource::Resource;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Tag {
    pub name: String,
    #[serde(default)]
    pub translated_name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrendingTag {
    pub tag: String,
    pub translated_name: String,
    pub artwork: Artwork,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub account: String,
    pub comment: String,
    pub is_followed: bool,
    pub profile_image: ImageResource,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageResource {
    pub resource: Resource,
    pub variant: String,
    pub width: i64,
    pub height: i64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ArtworkPage {
    pub page_index: usize,
    pub image: ImageResource,
    pub width: i64,
    pub height: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkKind {
    #[serde(rename = "illustration")]
    Illust,
    Manga,
    Ugoira,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Artwork {
    pub id: i64,
    pub title: String,
    pub caption: String,
    pub kind: ArtworkKind,
    pub raw_kind: String,
    pub tags: Vec<Tag>,
    pub user: User,
    pub published_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub total_bookmarks: i64,
    pub total_views: i64,
    pub width: i64,
    pub height: i64,
    pub page_count: i64,
    pub x_restrict: i64,
    pub ai_type: i64,
    pub tools: Vec<String>,
    pub cover: ImageResource,
    pub pages: Vec<ArtworkPage>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UgoiraFrame {
    pub filename: String,
    pub delay_milliseconds: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UgoiraArchive {
    pub quality: String,
    pub resource: Resource,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UgoiraMetadata {
    pub artwork_id: i64,
    pub archives: Vec<UgoiraArchive>,
    pub frames: Vec<UgoiraFrame>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArtworkBookmarkDetail {
    pub restrict: String,
    pub tags: Vec<String>,
}
pub type NovelBookmarkDetail = ArtworkBookmarkDetail;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BookmarkTag {
    pub name: String,
    pub count: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Novel {
    pub id: i64,
    pub title: String,
    pub caption: String,
    pub user: User,
    pub tags: Vec<Tag>,
    pub published_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub x_restrict: i64,
    pub text_length: i64,
    pub is_original: bool,
    pub total_bookmarks: i64,
    pub total_views: i64,
    pub cover: ImageResource,
}
