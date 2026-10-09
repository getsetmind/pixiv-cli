use crate::{
    Client, Error, Reason, Result,
    cursor::{Cursor, Page},
    models::Novel,
    ranking::{apply_offset, next_cursor, ranking_error},
    transport::Transport,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
const OPERATION: &str = "SearchNovels";
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchNovelsRequest {
    pub word: String,
    pub target: String,
    pub sort: String,
    pub duration: String,
    pub cursor: Cursor,
}
#[derive(Deserialize)]
struct Envelope {
    novels: Option<Vec<Value>>,
    next_url: Option<String>,
}
impl<T: Transport> Client<T> {
    pub async fn search_novels(&self, request: SearchNovelsRequest) -> Result<Page<Novel>> {
        if request.word.trim().is_empty() {
            return Err(ranking_error(
                OPERATION,
                Reason::InvalidArgument,
                "search word is required",
            ));
        }
        for (value, allowed, detail) in [
            (
                &request.target,
                &[
                    "",
                    "partial_match_for_tags",
                    "exact_match_for_tags",
                    "title_and_caption",
                    "keyword",
                ][..],
                "search target is unsupported",
            ),
            (
                &request.sort,
                &["", "date_desc", "date_asc", "popular_desc"][..],
                "sort mode is unsupported",
            ),
            (
                &request.duration,
                &[
                    "",
                    "within_last_day",
                    "within_last_week",
                    "within_last_month",
                ][..],
                "duration is unsupported",
            ),
        ] {
            if !allowed.contains(&value.as_str()) {
                return Err(ranking_error(OPERATION, Reason::InvalidArgument, detail));
            }
        }
        let target = if request.target.is_empty() {
            "partial_match_for_tags"
        } else {
            &request.target
        };
        let sort = if request.sort.is_empty() {
            "date_desc"
        } else {
            &request.sort
        };
        let mut query = BTreeMap::from([
            ("word".into(), request.word.clone()),
            ("search_target".into(), target.into()),
            ("sort".into(), sort.into()),
        ]);
        if !request.duration.is_empty() {
            query.insert("duration".into(), request.duration);
        }
        let digest = crate::continuation::query_digest(&query);
        apply_offset(&request.cursor, OPERATION, &digest, &mut query)?;
        let body = self
            .get("/v1/search/novel", query.into_iter().collect(), OPERATION)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, OPERATION);
        let envelope: Envelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let wire = crate::novel::decode_list(envelope.novels.ok_or_else(malformed)?, OPERATION)?;
        crate::novel::validate_search(&wire)?;
        let offset = envelope
            .next_url
            .as_deref()
            .map(|raw| {
                crate::continuation::next_offset(
                    raw,
                    "v1/search/novel",
                    &["word", "search_target", "sort", "duration", "offset"],
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
