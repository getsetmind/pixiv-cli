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

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UserProfile {
    pub webpage: String,
    pub gender: String,
    pub birth_day: String,
    pub birth_year: i64,
    pub region: String,
    pub country_code: String,
    pub job: String,
    pub total_follow_users: i64,
    pub total_my_pixiv_users: i64,
    pub total_illusts: i64,
    pub total_manga: i64,
    pub total_novels: i64,
    pub total_illust_bookmarks: i64,
    pub total_illust_series: i64,
    pub total_novel_series: i64,
    pub background_image_url: String,
    pub twitter_account: String,
    pub twitter_url: String,
    pub pawoo_url: String,
    pub is_premium: bool,
    pub is_using_custom_profile_image: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UserWorkspace {
    pub pc: String,
    pub monitor: String,
    pub tool: String,
    pub scanner: String,
    pub tablet: String,
    pub mouse: String,
    pub printer: String,
    pub desktop: String,
    pub music: String,
    pub desk: String,
    pub chair: String,
    pub comment: String,
    pub workspace_image_url: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UserProfilePublicity {
    pub gender: bool,
    pub region: bool,
    pub birth_day: bool,
    pub birth_year: bool,
    pub job: bool,
    pub pawoo: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserDetail {
    pub user: User,
    pub profile: UserProfile,
    pub profile_publicity: UserProfilePublicity,
    pub workspace: UserWorkspace,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserPreview {
    pub user: User,
    pub illusts: Vec<Artwork>,
    pub novels: Vec<Novel>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NovelSeries {
    pub id: i64,
    pub title: String,
    pub caption: String,
    pub user: User,
    pub is_concluded: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NovelSeriesResult {
    pub series: NovelSeries,
    pub novels: crate::cursor::Page<Novel>,
}
pub type NovelBlockKind = String;
pub const NOVEL_BLOCK_PARAGRAPH: &str = "paragraph";
pub const NOVEL_BLOCK_HEADER: &str = "header";
pub const NOVEL_BLOCK_IMAGE: &str = "image";
pub const NOVEL_BLOCK_FILE: &str = "file";
pub const NOVEL_BLOCK_UNKNOWN: &str = "unknown";
pub type NovelMarkKind = String;
pub const NOVEL_MARK_STRONG: &str = "strong";
pub const NOVEL_MARK_EMPHASIS: &str = "emphasis";
pub const NOVEL_MARK_DELETE: &str = "delete";
pub const NOVEL_MARK_RUBY: &str = "ruby";
pub const NOVEL_MARK_LINK: &str = "link";
pub const NOVEL_MARK_CUSTOM: &str = "custom";
pub const NOVEL_MARK_UNKNOWN: &str = "unknown";
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NovelRuby {
    pub text: String,
    pub furigana: String,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NovelMark {
    pub kind: NovelMarkKind,
    pub text: String,
    pub ruby: Option<NovelRuby>,
    pub href: String,
    pub class: String,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NovelImageBlock {
    pub resource: Resource,
    pub caption: String,
    pub width: i64,
    pub height: i64,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NovelFileBlock {
    pub resource: Resource,
    pub filename: String,
    pub caption: String,
    pub size: i64,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NovelUnknownBlock {
    pub raw_type: String,
    pub payload: std::collections::BTreeMap<String, String>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NovelBlock {
    pub kind: NovelBlockKind,
    pub text: String,
    pub marks: Vec<NovelMark>,
    pub image: Option<NovelImageBlock>,
    pub file: Option<NovelFileBlock>,
    pub unknown: Option<NovelUnknownBlock>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NovelContent {
    pub novel_id: i64,
    pub title: String,
    pub caption: String,
    pub blocks: Vec<NovelBlock>,
}
