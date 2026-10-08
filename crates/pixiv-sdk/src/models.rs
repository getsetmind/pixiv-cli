use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Tag {
    pub name: String,
    #[serde(default)]
    pub translated_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub account: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkKind {
    Illust,
    Manga,
    Ugoira,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Artwork {
    pub id: i64,
    pub title: String,
    pub caption: String,
    pub kind: ArtworkKind,
    pub raw_kind: String,
    pub tags: Vec<Tag>,
    pub user: User,
    pub published_at: DateTime<Utc>,
    pub total_bookmarks: u64,
    pub total_views: u64,
    pub width: u32,
    pub height: u32,
    pub page_count: u32,
    pub x_restrict: u32,
    pub ai_type: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UgoiraFrame {
    pub filename: String,
    pub delay_milliseconds: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UgoiraMetadata {
    pub artwork_id: i64,
    pub frames: Vec<UgoiraFrame>,
}
