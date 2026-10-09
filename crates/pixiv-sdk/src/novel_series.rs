use crate::{
    Client, Error, Reason, Result,
    artwork::WireUser,
    cursor::Cursor,
    models::{NovelSeries, NovelSeriesResult},
    transport::Transport,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
const OPERATION: &str = "NovelSeries";
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NovelSeriesRequest {
    pub series_id: i64,
    pub cursor: Cursor,
}
#[derive(Deserialize)]
struct Envelope {
    novel_series_detail: Option<Detail>,
    novels: Option<Vec<Value>>,
    next_url: Option<String>,
}
#[derive(Deserialize)]
struct Detail {
    id: Option<i64>,
    title: Option<String>,
    caption: Option<String>,
    user: Option<WireUser>,
    is_concluded: Option<bool>,
}
impl<T: Transport> Client<T> {
    pub async fn novel_series(&self, request: NovelSeriesRequest) -> Result<NovelSeriesResult> {
        if request.series_id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, OPERATION)
                .with_detail("series ID must be positive"));
        }
        let mut query = BTreeMap::from([("series_id".into(), request.series_id.to_string())]);
        let digest = crate::continuation::query_digest(&query);
        crate::ranking::apply_value(
            &request.cursor,
            OPERATION,
            &digest,
            &mut query,
            "last_order",
            "cursor continuation value must be positive",
        )?;
        let body = self
            .get("/v2/novel/series", query.into_iter().collect(), OPERATION)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, OPERATION);
        let envelope: Envelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let detail = envelope.novel_series_detail.ok_or_else(malformed)?;
        if detail.id.unwrap_or_default() <= 0
            || detail
                .user
                .as_ref()
                .and_then(|user| user.id)
                .unwrap_or_default()
                <= 0
        {
            return Err(malformed());
        }
        let wire = crate::novel::decode_list(envelope.novels.ok_or_else(malformed)?, OPERATION)?;
        let next = envelope
            .next_url
            .as_deref()
            .map(|raw| {
                crate::continuation::next_value(
                    raw,
                    "v2/novel/series",
                    &["series_id", "last_order"],
                    "last_order",
                )
                .ok_or_else(malformed)
            })
            .transpose()?;
        let mut items = Vec::with_capacity(wire.len());
        for item in wire {
            let novel = item.map(&self.resource_policy)?;
            self.remember_resource(&novel.cover.resource);
            self.remember_resource(&novel.user.profile_image.resource);
            items.push(novel);
        }
        let series = NovelSeries {
            id: detail.id.unwrap_or_default(),
            title: detail.title.unwrap_or_default(),
            caption: detail.caption.unwrap_or_default(),
            user: detail.user.unwrap_or_default().map(&self.resource_policy),
            is_concluded: detail.is_concluded.unwrap_or_default(),
        };
        self.remember_resource(&series.user.profile_image.resource);
        Ok(NovelSeriesResult {
            series,
            novels: crate::cursor::Page {
                items,
                next: crate::ranking::value_cursor(OPERATION, &digest, "last_order", next)?,
            },
        })
    }
}
