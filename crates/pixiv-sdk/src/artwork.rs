use crate::{
    Error, Reason, Result,
    models::{Artwork, ArtworkKind, ArtworkPage, ImageResource, Tag, User},
    resource::{Resource, ResourceRef},
};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Default, Deserialize)]
struct Images {
    original: Option<String>,
    large: Option<String>,
    medium: Option<String>,
    square_medium: Option<String>,
}
impl Images {
    fn first(&self) -> Option<(&str, &str)> {
        [
            ("original", &self.original),
            ("large", &self.large),
            ("medium", &self.medium),
            ("square_medium", &self.square_medium),
        ]
        .into_iter()
        .find_map(|(variant, value)| {
            value
                .as_deref()
                .filter(|url| !url.is_empty())
                .map(|url| (variant, url))
        })
    }
}
#[derive(Default, Deserialize)]
struct WireUser {
    id: Option<i64>,
    name: Option<String>,
    account: Option<String>,
    comment: Option<String>,
    is_followed: Option<bool>,
    profile_image_urls: Option<ProfileImages>,
}
#[derive(Default, Deserialize)]
struct ProfileImages {
    medium: Option<String>,
}
#[derive(Default, Deserialize)]
struct WireTag {
    name: Option<String>,
    translated_name: Option<String>,
}
#[derive(Default, Deserialize)]
struct SinglePage {
    original_image_url: Option<String>,
}
#[derive(Default, Deserialize)]
struct MetaPage {
    width: Option<i64>,
    height: Option<i64>,
    image_urls: Option<Images>,
}
#[derive(Deserialize)]
struct WireArtwork {
    id: Option<i64>,
    title: Option<String>,
    caption: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    tags: Option<Vec<WireTag>>,
    user: Option<WireUser>,
    create_date: Option<String>,
    total_bookmarks: Option<i64>,
    total_view: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
    page_count: Option<i64>,
    x_restrict: Option<i64>,
    illust_ai_type: Option<i64>,
    ai_type: Option<i64>,
    tools: Option<Vec<String>>,
    image_urls: Option<Images>,
    meta_single_page: Option<SinglePage>,
    meta_pages: Option<Vec<MetaPage>>,
}

pub(crate) fn map(value: Value, operation: &'static str, detail: bool) -> Result<Artwork> {
    let wire: WireArtwork = serde_json::from_value(value)
        .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, operation))?;
    let id = wire.id.unwrap_or_default();
    if id <= 0 {
        return Err(Error::new(Reason::MalformedUpstreamResponse, operation));
    }
    let published_at = DateTime::parse_from_rfc3339(&wire.create_date.unwrap_or_default())
        .map_err(|_| {
            Error::new(Reason::MalformedUpstreamResponse, "Artwork")
                .with_detail("invalid publish time")
        })?
        .to_utc();
    let raw_kind = wire.kind.unwrap_or_default();
    let kind = match raw_kind.as_str() {
        "illust" => ArtworkKind::Illust,
        "manga" => ArtworkKind::Manga,
        "ugoira" => ArtworkKind::Ugoira,
        _ => ArtworkKind::Unknown,
    };
    let width = wire.width.unwrap_or_default();
    let height = wire.height.unwrap_or_default();
    let wire_user = wire.user.unwrap_or_default();
    let user_id = wire_user.id.unwrap_or_default();
    let profile_image = wire_user
        .profile_image_urls
        .and_then(|images| images.medium)
        .filter(|url| !url.is_empty())
        .and_then(|url| image("user_profile", user_id, -1, "medium", &url, 0, 0).ok())
        .unwrap_or_default();
    let user = User {
        id: user_id,
        name: wire_user.name.unwrap_or_default(),
        account: wire_user.account.unwrap_or_default(),
        comment: wire_user.comment.unwrap_or_default(),
        is_followed: wire_user.is_followed.unwrap_or_default(),
        profile_image,
    };
    let images = wire.image_urls.unwrap_or_default();
    let cover = match images.first() {
        Some((variant, url)) => image("artwork", id, -1, variant, url, width, height)?,
        None => ImageResource::default(),
    };
    let mut pages = Vec::new();
    if detail {
        let meta_pages = wire.meta_pages.unwrap_or_default();
        if !meta_pages.is_empty() {
            for (index, page) in meta_pages.into_iter().enumerate() {
                let images = page.image_urls.unwrap_or_default();
                let (_, url) = images.first().ok_or_else(|| {
                    Error::new(Reason::MalformedUpstreamResponse, "ArtworkPages")
                        .with_detail("page has no image URL")
                })?;
                let width = page.width.unwrap_or_default();
                let height = page.height.unwrap_or_default();
                pages.push(ArtworkPage {
                    page_index: index,
                    image: image("artwork", id, index as i64, "original", url, width, height)?,
                    width,
                    height,
                });
            }
        } else if let Some(url) = wire
            .meta_single_page
            .and_then(|page| page.original_image_url)
            .filter(|url| !url.is_empty())
        {
            pages.push(ArtworkPage {
                page_index: 0,
                image: image("artwork", id, 0, "original", &url, width, height)?,
                width,
                height,
            });
        }
    }
    Ok(Artwork {
        id,
        title: wire.title.unwrap_or_default(),
        caption: wire.caption.unwrap_or_default(),
        kind,
        raw_kind,
        tags: wire
            .tags
            .unwrap_or_default()
            .into_iter()
            .map(|tag| Tag {
                name: tag.name.unwrap_or_default(),
                translated_name: tag.translated_name.unwrap_or_default(),
            })
            .collect(),
        user,
        published_at,
        updated_at: None,
        total_bookmarks: wire.total_bookmarks.unwrap_or_default(),
        total_views: wire.total_view.unwrap_or_default(),
        width,
        height,
        page_count: wire.page_count.unwrap_or_default(),
        x_restrict: wire.x_restrict.unwrap_or_default(),
        ai_type: wire.illust_ai_type.or(wire.ai_type).unwrap_or_default(),
        tools: wire.tools.unwrap_or_default(),
        cover,
        pages,
    })
}

fn image(
    kind: &str,
    id: i64,
    page: i64,
    variant: &str,
    url: &str,
    width: i64,
    height: i64,
) -> Result<ImageResource> {
    let forbidden = |detail| Error::new(Reason::ResourceForbidden, "resource").with_detail(detail);
    let parsed = url::Url::parse(url).map_err(|_| forbidden("invalid resource URL"))?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(forbidden("resource URL must be https without userinfo"));
    }
    if parsed.path().is_empty() || parsed.path() == "/" {
        return Err(forbidden("resource URL has no path"));
    }
    if !matches!(
        parsed.host_str(),
        Some("i.pximg.net" | "s.pximg.net" | "i-f.pximg.net")
    ) {
        return Err(forbidden("resource host is not allowed"));
    }
    #[derive(Serialize)]
    struct Identity<'a> {
        k: &'a str,
        id: i64,
        #[serde(skip_serializing_if = "is_zero")]
        p: i64,
        #[serde(skip_serializing_if = "str::is_empty")]
        v: &'a str,
    }
    fn is_zero(value: &i64) -> bool {
        *value == 0
    }
    let payload = serde_json::to_vec(&Identity {
        k: kind,
        id,
        p: page,
        v: variant,
    })
    .map_err(|_| {
        Error::new(Reason::UpstreamError, "resource")
            .with_detail("cannot encode resource reference")
    })?;
    let resource = Resource {
        reference: ResourceRef::new("pixiv", &payload)?,
        url: url.to_owned(),
        request_headers: [("Referer".into(), "https://app-api.pixiv.net/".into())].into(),
        expires_at: None,
        requires_credentials: false,
    };
    Ok(ImageResource {
        resource,
        variant: variant.to_owned(),
        width,
        height,
    })
}
