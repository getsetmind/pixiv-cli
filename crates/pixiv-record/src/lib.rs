use pixiv_sdk::{
    dto::ArtworkDto,
    models::{Artwork, ArtworkKind},
};
use serde_json::Value;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordError {
    UnsupportedArtworkKind,
    InvalidId,
    Serialization,
}
impl RecordError {
    pub fn message(self) -> &'static str {
        match self {
            Self::UnsupportedArtworkKind => "unsupported artwork kind for record",
            Self::InvalidId => "record id must be positive",
            Self::Serialization => "record serialization failed",
        }
    }
}
impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for RecordError {}

pub fn from_artwork(artwork: &Artwork) -> Result<Value, RecordError> {
    let kind = match artwork.kind {
        ArtworkKind::Illust => "illust",
        ArtworkKind::Manga => "manga",
        ArtworkKind::Ugoira => "ugoira",
        ArtworkKind::Unknown => return Err(RecordError::UnsupportedArtworkKind),
    };
    if artwork.id <= 0 {
        return Err(RecordError::InvalidId);
    }
    let mut record =
        serde_json::to_value(ArtworkDto::from(artwork)).map_err(|_| RecordError::Serialization)?;
    record["id"] = artwork.id.to_string().into();
    record["type"] = kind.into();
    record["url"] = format!("https://www.pixiv.net/artworks/{}", artwork.id).into();
    Ok(record)
}

pub fn from_novel(novel: &pixiv_sdk::models::Novel) -> Result<Value, RecordError> {
    if novel.id <= 0 {
        return Err(RecordError::InvalidId);
    }
    let mut record = serde_json::to_value(pixiv_sdk::dto::NovelDto::from(novel))
        .map_err(|_| RecordError::Serialization)?;
    record["id"] = novel.id.to_string().into();
    record["type"] = "novel".into();
    record["url"] = format!("https://www.pixiv.net/novel/show.php?id={}", novel.id).into();
    Ok(record)
}

pub fn from_user_detail(detail: &pixiv_sdk::models::UserDetail) -> Result<Value, RecordError> {
    if detail.user.id <= 0 {
        return Err(RecordError::InvalidId);
    }
    let mut record = serde_json::to_value(pixiv_sdk::dto::UserDetailDto::from(detail))
        .map_err(|_| RecordError::Serialization)?;
    record["id"] = detail.user.id.to_string().into();
    record["type"] = "user".into();
    record["url"] = format!("https://www.pixiv.net/users/{}", detail.user.id).into();
    Ok(record)
}

pub fn from_user_preview(preview: &pixiv_sdk::models::UserPreview) -> Result<Value, RecordError> {
    if preview.user.id <= 0 {
        return Err(RecordError::InvalidId);
    }
    let mut record = serde_json::to_value(pixiv_sdk::dto::UserPreviewDto::from(preview))
        .map_err(|_| RecordError::Serialization)?;
    record["id"] = preview.user.id.to_string().into();
    record["type"] = "user".into();
    record["url"] = format!("https://www.pixiv.net/users/{}", preview.user.id).into();
    Ok(record)
}
