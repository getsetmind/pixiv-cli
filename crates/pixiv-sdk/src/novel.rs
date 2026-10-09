use crate::{
    Error, Reason, Result,
    artwork::{Images, ResourcePolicy, WireTag, WireUser},
    models::{ImageResource, Novel},
};
use chrono::DateTime;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub(crate) struct WireNovel {
    id: Option<i64>,
    title: Option<String>,
    caption: Option<String>,
    user: Option<WireUser>,
    tags: Option<Vec<Option<WireTag>>>,
    create_date: Option<String>,
    x_restrict: Option<i64>,
    text_length: Option<i64>,
    is_original: Option<bool>,
    total_bookmarks: Option<i64>,
    total_view: Option<i64>,
    image_urls: Option<Images>,
}
pub(crate) fn decode_list(values: Vec<Value>, operation: &'static str) -> Result<Vec<WireNovel>> {
    let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
    let items: Vec<Option<WireNovel>> = values
        .into_iter()
        .map(|value| serde_json::from_value(value).map_err(|_| malformed()))
        .collect::<Result<_>>()?;
    if items.iter().any(|item| {
        item.as_ref().is_none_or(|item| {
            item.id.unwrap_or_default() <= 0
                || item
                    .user
                    .as_ref()
                    .and_then(|user| user.id)
                    .unwrap_or_default()
                    <= 0
        })
    }) {
        return Err(malformed());
    }
    Ok(items.into_iter().flatten().collect())
}
impl WireNovel {
    pub(crate) fn map(self, policy: &ResourcePolicy) -> Result<Novel> {
        let published_at = DateTime::parse_from_rfc3339(&self.create_date.unwrap_or_default())
            .map_err(|_| {
                Error::new(Reason::MalformedUpstreamResponse, "Novel")
                    .with_detail("invalid publish time")
            })?
            .to_utc();
        let id = self.id.unwrap_or_default();
        let images = self.image_urls.unwrap_or_default();
        let cover = match images.first() {
            Some((variant, url)) => policy.image("novel_cover", id, -1, variant, url, (0, 0))?,
            None => ImageResource::default(),
        };
        Ok(Novel {
            id,
            title: self.title.unwrap_or_default(),
            caption: self.caption.unwrap_or_default(),
            user: self.user.unwrap_or_default().map(policy),
            tags: self
                .tags
                .unwrap_or_default()
                .into_iter()
                .map(|tag| tag.unwrap_or_default().map())
                .collect(),
            published_at,
            updated_at: None,
            x_restrict: self.x_restrict.unwrap_or_default(),
            text_length: self.text_length.unwrap_or_default(),
            is_original: self.is_original.unwrap_or_default(),
            total_bookmarks: self.total_bookmarks.unwrap_or_default(),
            total_views: self.total_view.unwrap_or_default(),
            cover,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NovelRequest {
    pub novel_id: i64,
}
#[derive(Deserialize)]
struct DetailEnvelope {
    novel: Option<Value>,
    series_next: Option<SeriesReference>,
    series_prev: Option<SeriesReference>,
}
#[derive(Deserialize)]
struct SeriesReference {
    id: Option<i64>,
    #[serde(rename = "title")]
    _title: Option<String>,
}
impl<T: crate::transport::Transport> crate::Client<T> {
    pub async fn novel(&self, request: NovelRequest) -> Result<Novel> {
        if request.novel_id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, "Novel")
                .with_detail("novel ID must be positive"));
        }
        let body = self
            .get(
                "/v2/novel/detail",
                vec![("novel_id".into(), request.novel_id.to_string())],
                "Novel",
            )
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, "Novel");
        let envelope: DetailEnvelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let mut wire = decode_list(vec![envelope.novel.ok_or_else(malformed)?], "Novel")?;
        if [envelope.series_next, envelope.series_prev]
            .into_iter()
            .flatten()
            .any(|series| series.id.unwrap_or_default() <= 0)
        {
            return Err(malformed());
        }
        let novel = wire
            .pop()
            .ok_or_else(malformed)?
            .map(&self.resource_policy)?;
        self.remember_resource(&novel.cover.resource);
        self.remember_resource(&novel.user.profile_image.resource);
        Ok(novel)
    }
}

pub(crate) fn validate_search(items: &[WireNovel]) -> Result<()> {
    if items.iter().any(|item| {
        item.x_restrict.is_none() || item.text_length.is_none() || item.is_original.is_none()
    }) {
        return Err(Error::new(
            Reason::MalformedUpstreamResponse,
            "SearchNovels",
        ));
    }
    Ok(())
}
