use crate::{
    models::{
        Artwork, ArtworkKind, ArtworkPage, ImageResource, Tag, UgoiraArchive, UgoiraFrame,
        UgoiraMetadata, User,
    },
    resource::Resource,
};
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Serialize)]
pub struct TrendingTagDto<'a> {
    pub tag: &'a str,
    pub translated_name: &'a str,
    pub artwork: ArtworkDto<'a>,
}

impl<'a> From<&'a crate::models::TrendingTag> for TrendingTagDto<'a> {
    fn from(value: &'a crate::models::TrendingTag) -> Self {
        Self {
            tag: &value.tag,
            translated_name: &value.translated_name,
            artwork: ArtworkDto::from(&value.artwork),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtworkBookmarkDetailDto {
    pub restrict: String,
    pub tags: Option<Vec<String>>,
}
impl From<&crate::models::ArtworkBookmarkDetail> for ArtworkBookmarkDetailDto {
    fn from(value: &crate::models::ArtworkBookmarkDetail) -> Self {
        Self {
            restrict: value.restrict.clone(),
            tags: (!value.tags.is_empty()).then(|| value.tags.clone()),
        }
    }
}
pub type NovelBookmarkDetailDto = ArtworkBookmarkDetailDto;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BookmarkTagDto {
    pub name: String,
    pub count: i64,
}
impl From<&crate::models::BookmarkTag> for BookmarkTagDto {
    fn from(value: &crate::models::BookmarkTag) -> Self {
        Self {
            name: value.name.clone(),
            count: value.count,
        }
    }
}

#[derive(Serialize)]
pub struct UgoiraArchiveDto<'a> {
    pub quality: &'a str,
    pub resource: Option<ResourceDto<'a>>,
}
impl<'a> From<&'a UgoiraArchive> for UgoiraArchiveDto<'a> {
    fn from(value: &'a UgoiraArchive) -> Self {
        Self {
            quality: &value.quality,
            resource: ResourceDto::from_resource(&value.resource),
        }
    }
}
#[derive(Serialize)]
pub struct UgoiraFrameDto<'a> {
    pub filename: &'a str,
    pub delay_milliseconds: i64,
}
impl<'a> From<&'a UgoiraFrame> for UgoiraFrameDto<'a> {
    fn from(value: &'a UgoiraFrame) -> Self {
        Self {
            filename: &value.filename,
            delay_milliseconds: value.delay_milliseconds,
        }
    }
}
#[derive(Serialize)]
pub struct UgoiraMetadataDto<'a> {
    pub artwork_id: i64,
    pub archives: Vec<UgoiraArchiveDto<'a>>,
    pub frames: Vec<UgoiraFrameDto<'a>>,
}
impl<'a> From<&'a UgoiraMetadata> for UgoiraMetadataDto<'a> {
    fn from(value: &'a UgoiraMetadata) -> Self {
        Self {
            artwork_id: value.artwork_id,
            archives: value.archives.iter().map(UgoiraArchiveDto::from).collect(),
            frames: value.frames.iter().map(UgoiraFrameDto::from).collect(),
        }
    }
}

#[derive(Serialize)]
pub struct ResourceDto<'a> {
    pub r#ref: &'a str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub requires_credentials: bool,
}
impl<'a> ResourceDto<'a> {
    pub fn from_resource(resource: &'a Resource) -> Option<Self> {
        (!resource.reference.is_zero()).then(|| Self {
            r#ref: resource.reference.as_str(),
            requires_credentials: resource.requires_credentials,
        })
    }
}
#[derive(Serialize)]
pub struct ImageResourceDto<'a> {
    pub resource: Option<ResourceDto<'a>>,
    pub variant: &'a str,
    pub width: i64,
    pub height: i64,
}
impl<'a> From<&'a ImageResource> for ImageResourceDto<'a> {
    fn from(value: &'a ImageResource) -> Self {
        Self {
            resource: ResourceDto::from_resource(&value.resource),
            variant: &value.variant,
            width: value.width,
            height: value.height,
        }
    }
}
#[derive(Serialize)]
pub struct UserDto<'a> {
    pub id: i64,
    pub name: &'a str,
    pub account: &'a str,
    pub comment: &'a str,
    pub is_followed: bool,
    pub profile_image: ImageResourceDto<'a>,
}
impl<'a> From<&'a User> for UserDto<'a> {
    fn from(value: &'a User) -> Self {
        Self {
            id: value.id,
            name: &value.name,
            account: &value.account,
            comment: &value.comment,
            is_followed: value.is_followed,
            profile_image: (&value.profile_image).into(),
        }
    }
}
#[derive(Serialize)]
pub struct ArtworkPageDto<'a> {
    pub page_index: usize,
    pub image: ImageResourceDto<'a>,
    pub width: i64,
    pub height: i64,
}
impl<'a> From<&'a ArtworkPage> for ArtworkPageDto<'a> {
    fn from(value: &'a ArtworkPage) -> Self {
        Self {
            page_index: value.page_index,
            image: (&value.image).into(),
            width: value.width,
            height: value.height,
        }
    }
}
#[derive(Serialize)]
pub struct ArtworkDto<'a> {
    pub id: i64,
    pub title: &'a str,
    pub caption: &'a str,
    pub kind: &'a ArtworkKind,
    pub raw_kind: &'a str,
    pub tags: &'a [Tag],
    pub user: UserDto<'a>,
    pub published_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    pub total_bookmarks: i64,
    pub total_views: i64,
    pub width: i64,
    pub height: i64,
    pub page_count: i64,
    pub x_restrict: i64,
    pub ai_type: i64,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub tools: &'a [String],
    pub cover: ImageResourceDto<'a>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<ArtworkPageDto<'a>>,
}
impl<'a> From<&'a Artwork> for ArtworkDto<'a> {
    fn from(value: &'a Artwork) -> Self {
        Self {
            id: value.id,
            title: &value.title,
            caption: &value.caption,
            kind: &value.kind,
            raw_kind: &value.raw_kind,
            tags: &value.tags,
            user: (&value.user).into(),
            published_at: value.published_at,
            updated_at: value.updated_at,
            total_bookmarks: value.total_bookmarks,
            total_views: value.total_views,
            width: value.width,
            height: value.height,
            page_count: value.page_count,
            x_restrict: value.x_restrict,
            ai_type: value.ai_type,
            tools: &value.tools,
            cover: (&value.cover).into(),
            pages: value.pages.iter().map(Into::into).collect(),
        }
    }
}

#[derive(Serialize)]
pub struct NovelDto<'a> {
    pub id: i64,
    pub title: &'a str,
    pub caption: &'a str,
    pub user: UserDto<'a>,
    pub tags: &'a [Tag],
    pub published_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub x_restrict: i64,
    pub text_length: i64,
    pub is_original: bool,
    pub total_bookmarks: i64,
    pub total_views: i64,
    pub cover: ImageResourceDto<'a>,
}
impl<'a> From<&'a crate::models::Novel> for NovelDto<'a> {
    fn from(value: &'a crate::models::Novel) -> Self {
        Self {
            id: value.id,
            title: &value.title,
            caption: &value.caption,
            user: (&value.user).into(),
            tags: &value.tags,
            published_at: value.published_at,
            updated_at: value.updated_at,
            x_restrict: value.x_restrict,
            text_length: value.text_length,
            is_original: value.is_original,
            total_bookmarks: value.total_bookmarks,
            total_views: value.total_views,
            cover: (&value.cover).into(),
        }
    }
}
