use crate::{
    Client, Error, Reason, Result,
    cursor::{Cursor, Page},
    models::Novel,
    ranking::{MODES, RANKING_MODE_DAY, RankingMode, apply_offset, next_cursor, ranking_error},
    transport::Transport,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
const OPERATION: &str = "NovelRanking";
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NovelRankingRequest {
    pub mode: RankingMode,
    pub cursor: Cursor,
}
#[derive(Deserialize)]
struct Envelope {
    novels: Option<Vec<Value>>,
    next_url: Option<String>,
}
impl<T: Transport> Client<T> {
    pub async fn novel_ranking(&self, request: NovelRankingRequest) -> Result<Page<Novel>> {
        let mode = if request.mode.is_empty() {
            RANKING_MODE_DAY
        } else {
            &request.mode
        };
        if !MODES.contains(&mode) {
            return Err(ranking_error(
                OPERATION,
                Reason::InvalidArgument,
                "ranking mode is unsupported",
            ));
        }
        let mut query = BTreeMap::from([
            ("filter".into(), "for_android".into()),
            ("mode".into(), mode.into()),
        ]);
        let digest = crate::continuation::query_digest(&query);
        apply_offset(&request.cursor, OPERATION, &digest, &mut query)?;
        let body = self
            .get("/v1/novel/ranking", query.into_iter().collect(), OPERATION)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, OPERATION);
        let envelope: Envelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let wire = crate::novel::decode_list(envelope.novels.ok_or_else(malformed)?, OPERATION)?;
        let offset = envelope
            .next_url
            .as_deref()
            .map(|raw| {
                crate::continuation::next_offset(
                    raw,
                    "v1/novel/ranking",
                    &["filter", "mode", "offset"],
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
        Ok(Page {
            items,
            next: next_cursor(OPERATION, &digest, offset)?,
        })
    }
}
