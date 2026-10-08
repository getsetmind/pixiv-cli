use crate::{
    Error, Reason, Result,
    models::{Artwork, ArtworkKind, ArtworkPage, ImageResource, Tag, User},
    resource::{Resource, ResourceRef},
};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct ResourcePolicy {
    pub allowed_hosts: Vec<String>,
}

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

pub(crate) fn map(
    value: Value,
    operation: &'static str,
    detail: bool,
    policy: &ResourcePolicy,
) -> Result<Artwork> {
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
        .and_then(|url| {
            policy
                .image("user_profile", user_id, -1, "medium", &url, (0, 0))
                .ok()
        })
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
        Some((variant, url)) => policy.image("artwork", id, -1, variant, url, (width, height))?,
        None => ImageResource::default(),
    };
    let pages = if detail {
        map_pages(
            id,
            (width, height),
            wire.meta_pages.as_deref(),
            wire.meta_single_page.as_ref(),
            policy,
        )?
    } else {
        Vec::new()
    };
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

impl ResourcePolicy {
    fn image(
        &self,
        kind: &str,
        id: i64,
        page: i64,
        variant: &str,
        url: &str,
        dimensions: (i64, i64),
    ) -> Result<ImageResource> {
        let (width, height) = dimensions;
        let forbidden =
            |detail| Error::new(Reason::ResourceForbidden, "resource").with_detail(detail);
        let parsed = url::Url::parse(url).map_err(|_| forbidden("invalid resource URL"))?;
        let userinfo = url.split_once("://").is_some_and(|(_, tail)| {
            tail.split(['/', '?', '#'])
                .next()
                .is_some_and(|authority| authority.contains('@'))
        });
        if parsed.scheme() != "https" || userinfo {
            return Err(forbidden("resource URL must be https without userinfo"));
        }
        if parsed.path().is_empty() || parsed.path() == "/" {
            return Err(forbidden("resource URL has no path"));
        }
        if !matches!(
            parsed.host_str(),
            Some("i.pximg.net" | "s.pximg.net" | "i-f.pximg.net")
        ) && !self.allowed_hosts.iter().any(|allowed| {
            parsed
                .host_str()
                .is_some_and(|host| host.eq_ignore_ascii_case(allowed.trim()))
        }) {
            return Err(forbidden("resource host is not allowed"));
        }
        let reference = encode_identity(kind, id, page, variant)?;
        let resource = Resource {
            reference,
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
}

pub(crate) fn pages(value: Value, policy: &ResourcePolicy) -> Result<Vec<ArtworkPage>> {
    let wire: WireArtwork = serde_json::from_value(value)
        .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, "ArtworkPages"))?;
    if wire.id.unwrap_or_default() <= 0 {
        return Err(Error::new(
            Reason::MalformedUpstreamResponse,
            "ArtworkPages",
        ));
    }
    map_pages(
        wire.id.unwrap_or_default(),
        (
            wire.width.unwrap_or_default(),
            wire.height.unwrap_or_default(),
        ),
        wire.meta_pages.as_deref(),
        wire.meta_single_page.as_ref(),
        policy,
    )
}

fn map_pages(
    id: i64,
    dimensions: (i64, i64),
    meta_pages: Option<&[MetaPage]>,
    single_page: Option<&SinglePage>,
    policy: &ResourcePolicy,
) -> Result<Vec<ArtworkPage>> {
    let mut pages = Vec::new();
    let meta_pages = meta_pages.unwrap_or_default();
    if !meta_pages.is_empty() {
        for (index, page) in meta_pages.iter().enumerate() {
            let images = page.image_urls.as_ref();
            let (_, url) = images.and_then(Images::first).ok_or_else(|| {
                Error::new(Reason::MalformedUpstreamResponse, "ArtworkPages")
                    .with_detail("page has no image URL")
            })?;
            let width = page.width.unwrap_or_default();
            let height = page.height.unwrap_or_default();
            pages.push(ArtworkPage {
                page_index: index,
                image: policy.image(
                    "artwork",
                    id,
                    index as i64,
                    "original",
                    url,
                    (width, height),
                )?,
                width,
                height,
            });
        }
    } else if let Some(url) = single_page
        .and_then(|page| page.original_image_url.as_deref())
        .filter(|url| !url.is_empty())
    {
        let (width, height) = dimensions;
        pages.push(ArtworkPage {
            page_index: 0,
            image: policy.image("artwork", id, 0, "original", url, (width, height))?,
            width,
            height,
        });
    }
    Ok(pages)
}

fn encode_identity(kind: &str, id: i64, page: i64, variant: &str) -> Result<ResourceRef> {
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
    let payload = serde_json::to_string(&Identity {
        k: kind,
        id,
        p: page,
        v: variant,
    })
    .map_err(|_| {
        Error::new(Reason::UpstreamError, "resource")
            .with_detail("cannot encode resource reference")
    })?
    .replace('<', "\\u003c")
    .replace('>', "\\u003e")
    .replace('&', "\\u0026")
    .replace('\u{2028}', "\\u2028")
    .replace('\u{2029}', "\\u2029");
    ResourceRef::new("pixiv", payload.as_bytes())
}

pub fn artwork_variant_resource(original: &Resource, variant: &str) -> Result<ResourceRef> {
    if variant.is_empty() || variant == "original" {
        return Ok(original.reference.clone());
    }
    #[derive(Default, Deserialize)]
    struct Identity {
        k: Option<String>,
        id: Option<i64>,
        p: Option<i64>,
        v: Option<String>,
    }
    let payload = original.reference.payload()?;
    let invalid = || {
        Error::new(Reason::InvalidArgument, "resource")
            .with_detail("cannot decode resource reference")
    };
    let normalized = crate::codec::normalize_json(&payload).map_err(|_| invalid())?;
    let identity = serde_json::from_str::<Option<Identity>>(&normalized)
        .map_err(|_| invalid())?
        .unwrap_or_default();
    let kind = identity.k.unwrap_or_default();
    let id = identity.id.unwrap_or_default();
    if kind.is_empty() || id <= 0 {
        return Err(invalid());
    }
    if kind != "artwork" {
        return Err(Error::new(Reason::InvalidArgument, "resource")
            .with_detail("only artwork resources support quality variants"));
    }
    let _ = identity.v;
    encode_identity(&kind, id, identity.p.unwrap_or_default(), variant)
}
