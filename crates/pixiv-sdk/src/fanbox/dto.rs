use super::models::*;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ResourceDto {
    pub r#ref: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub requires_credentials: bool,
}
fn resource(value: &crate::resource::Resource) -> Option<ResourceDto> {
    (!value.reference.is_zero()).then(|| ResourceDto {
        r#ref: value.reference.as_str().into(),
        requires_credentials: value.requires_credentials,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ImageResourceDto {
    pub resource: Option<ResourceDto>,
    pub variant: String,
    pub width: i64,
    pub height: i64,
}
impl ImageResource {
    pub fn to_dto(&self) -> ImageResourceDto {
        ImageResourceDto {
            resource: resource(&self.resource),
            variant: self.variant.clone(),
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FileResourceDto {
    pub resource: Option<ResourceDto>,
    pub name: String,
}
impl FileResource {
    pub fn to_dto(&self) -> FileResourceDto {
        FileResourceDto {
            resource: resource(&self.resource),
            name: self.name.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CreatorSummaryDto {
    pub id: String,
    pub name: String,
    pub icon: ImageResourceDto,
}
impl CreatorSummary {
    pub fn to_dto(&self) -> CreatorSummaryDto {
        CreatorSummaryDto {
            id: self.id.clone(),
            name: self.name.clone(),
            icon: self.icon.to_dto(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CreatorDto {
    pub id: String,
    pub name: String,
    pub icon: ImageResourceDto,
    pub has_adult_content: bool,
    pub is_following: bool,
    pub cover: ImageResourceDto,
    pub plan_fee: i64,
    pub has_supporting_plan: bool,
}
impl Creator {
    pub fn to_dto(&self) -> CreatorDto {
        CreatorDto {
            id: self.id.clone(),
            name: self.name.clone(),
            icon: self.icon.to_dto(),
            has_adult_content: self.has_adult_content,
            is_following: self.is_following,
            cover: self.cover.to_dto(),
            plan_fee: self.plan_fee,
            has_supporting_plan: self.has_supporting_plan,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PostImageBlockDto {
    pub resource: Option<ResourceDto>,
    pub caption: String,
}
impl PostImageBlock {
    pub fn to_dto(&self) -> PostImageBlockDto {
        PostImageBlockDto {
            resource: resource(&self.resource),
            caption: self.caption.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PostFileBlockDto {
    pub resource: Option<ResourceDto>,
    pub name: String,
    pub caption: String,
}
impl PostFileBlock {
    pub fn to_dto(&self) -> PostFileBlockDto {
        PostFileBlockDto {
            resource: resource(&self.resource),
            name: self.name.clone(),
            caption: self.caption.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PostArticleBlockDto {
    pub text: String,
}
impl PostArticleBlock {
    pub fn to_dto(&self) -> PostArticleBlockDto {
        PostArticleBlockDto {
            text: self.text.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PostVideoEmbedDto {
    pub provider: String,
    pub content_id: String,
    pub canonical_url: String,
    pub title: String,
    pub thumbnail_url: String,
    pub video_id: String,
    pub embedded_data: BTreeMap<String, String>,
}
impl PostVideoEmbed {
    pub fn to_dto(&self) -> PostVideoEmbedDto {
        PostVideoEmbedDto {
            provider: self.provider.clone(),
            content_id: self.content_id.clone(),
            canonical_url: self.canonical_url.clone(),
            title: self.title.clone(),
            thumbnail_url: self.thumbnail_url.clone(),
            video_id: self.video_id.clone(),
            embedded_data: self.embedded_data.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PostUnknownBlockDto {
    pub raw_type: String,
    pub payload: BTreeMap<String, String>,
}
impl PostUnknownBlock {
    pub fn to_dto(&self) -> PostUnknownBlockDto {
        PostUnknownBlockDto {
            raw_type: self.raw_type.clone(),
            payload: self.payload.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PostBlockDto {
    pub kind: PostBlockKind,
    pub image: Option<PostImageBlockDto>,
    pub file: Option<PostFileBlockDto>,
    pub article: Option<PostArticleBlockDto>,
    pub video: Option<PostVideoEmbedDto>,
    pub unknown: Option<PostUnknownBlockDto>,
}
impl PostBlock {
    pub fn to_dto(&self) -> PostBlockDto {
        PostBlockDto {
            kind: self.kind,
            image: self.image.as_ref().map(PostImageBlock::to_dto),
            file: self.file.as_ref().map(PostFileBlock::to_dto),
            article: self.article.as_ref().map(PostArticleBlock::to_dto),
            video: self.video.as_ref().map(PostVideoEmbed::to_dto),
            unknown: self.unknown.as_ref().map(PostUnknownBlock::to_dto),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AssetDto {
    pub id: String,
    pub kind: AssetKind,
    pub name: String,
    pub resource: Option<ResourceDto>,
    pub thumbnail: ImageResourceDto,
}
impl Asset {
    pub fn to_dto(&self) -> AssetDto {
        AssetDto {
            id: self.id.clone(),
            kind: self.kind,
            name: self.name.clone(),
            resource: resource(&self.resource),
            thumbnail: self.thumbnail.to_dto(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PostBodyDto {
    pub text: String,
    pub blocks: Vec<PostBlockDto>,
    pub assets: Vec<AssetDto>,
}
impl PostBody {
    pub fn to_dto(&self) -> PostBodyDto {
        PostBodyDto {
            text: self.text.clone(),
            blocks: self
                .blocks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(PostBlock::to_dto)
                .collect(),
            assets: self
                .assets
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(Asset::to_dto)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PostDto {
    pub id: String,
    pub title: String,
    pub published_at: String,
    pub creator_id: String,
    pub fee_required: i64,
    pub is_restricted: bool,
    pub is_pinned: bool,
    pub restricted_for: i64,
    pub comment_count: i64,
    pub cover: ImageResourceDto,
    pub body: Option<PostBodyDto>,
}
impl Post {
    pub fn to_dto(&self) -> PostDto {
        PostDto {
            id: self.id.clone(),
            title: self.title.clone(),
            published_at: format_time(self.published_at),
            creator_id: self.creator_id.clone(),
            fee_required: self.fee_required,
            is_restricted: self.is_restricted,
            is_pinned: self.is_pinned,
            restricted_for: self.restricted_for,
            comment_count: self.comment_count,
            cover: self.cover.to_dto(),
            body: self.body.as_ref().map(PostBody::to_dto),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CreatorTagDto {
    pub name: String,
    pub url: String,
}
impl CreatorTag {
    pub fn to_dto(&self) -> CreatorTagDto {
        CreatorTagDto {
            name: self.name.clone(),
            url: self.url.clone(),
        }
    }
}

fn format_time(value: chrono::DateTime<chrono::Utc>) -> String {
    let mut text = value.format("%Y-%m-%dT%H:%M:%S").to_string();
    if value.timestamp_subsec_nanos() != 0 {
        text.push_str(format!(".{:09}", value.timestamp_subsec_nanos()).trim_end_matches('0'));
    }
    text.push('Z');
    text
}
