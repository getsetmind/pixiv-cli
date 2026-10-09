use crate::{
    Client, Error, Reason, Result,
    cursor::{Cursor, CursorOptions, Page},
    models::Artwork,
    transport::Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const OPERATION: &str = "ArtworkRanking";
pub type RankingMode = String;
pub const RANKING_MODE_DAY: &str = "day";
pub const RANKING_MODE_DAY_MALE: &str = "day_male";
pub const RANKING_MODE_DAY_FEMALE: &str = "day_female";
pub const RANKING_MODE_WEEK: &str = "week";
pub const RANKING_MODE_WEEK_ORIGINAL: &str = "week_original";
pub const RANKING_MODE_WEEK_ROOKIE: &str = "week_rookie";
pub const RANKING_MODE_MONTH: &str = "month";
pub const RANKING_MODE_DAY_MANGA: &str = "day_manga";
pub const RANKING_MODE_WEEK_MANGA: &str = "week_manga";
pub const RANKING_MODE_MONTH_MANGA: &str = "month_manga";
pub const RANKING_MODE_WEEK_ROOKIE_MANGA: &str = "week_rookie_manga";
pub const RANKING_MODE_DAY_R18: &str = "day_r18";
pub const RANKING_MODE_DAY_MALE_R18: &str = "day_male_r18";
pub const RANKING_MODE_DAY_FEMALE_R18: &str = "day_female_r18";
pub const RANKING_MODE_WEEK_R18: &str = "week_r18";
pub const RANKING_MODE_WEEK_R18G: &str = "week_r18g";
pub(crate) const MODES: &[&str] = &[
    RANKING_MODE_DAY,
    RANKING_MODE_DAY_MALE,
    RANKING_MODE_DAY_FEMALE,
    RANKING_MODE_WEEK,
    RANKING_MODE_WEEK_ORIGINAL,
    RANKING_MODE_WEEK_ROOKIE,
    RANKING_MODE_MONTH,
    RANKING_MODE_DAY_MANGA,
    RANKING_MODE_WEEK_MANGA,
    RANKING_MODE_MONTH_MANGA,
    RANKING_MODE_WEEK_ROOKIE_MANGA,
    RANKING_MODE_DAY_R18,
    RANKING_MODE_DAY_MALE_R18,
    RANKING_MODE_DAY_FEMALE_R18,
    RANKING_MODE_WEEK_R18,
    RANKING_MODE_WEEK_R18G,
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArtworkRankingRequest {
    pub mode: RankingMode,
    pub date: String,
    pub cursor: Cursor,
}

#[derive(Default, Deserialize)]
struct Continuation {
    k: Option<String>,
    v: Option<i64>,
    s: Option<i64>,
    p: Option<BTreeMap<String, Option<Vec<String>>>>,
}
#[derive(Serialize)]
struct Position<'a> {
    k: &'a str,
    v: i64,
}
#[derive(Deserialize)]
struct Envelope {
    illusts: Option<Vec<Value>>,
    next_url: Option<String>,
}

fn error(reason: Reason, detail: &'static str) -> Error {
    Error::new(reason, OPERATION).with_detail(detail)
}

impl<T: Transport> Client<T> {
    pub async fn artwork_ranking(&self, request: ArtworkRankingRequest) -> Result<Page<Artwork>> {
        let mode = if request.mode.is_empty() {
            RANKING_MODE_DAY
        } else {
            &request.mode
        };
        if !MODES.contains(&mode) {
            return Err(error(
                Reason::InvalidArgument,
                "ranking mode is unsupported",
            ));
        }
        if !request.date.is_empty() {
            let shape = request.date.len() == 10
                && request.date.bytes().enumerate().all(|(i, b)| {
                    if i == 4 || i == 7 {
                        b == b'-'
                    } else {
                        b.is_ascii_digit()
                    }
                });
            if !shape || chrono::NaiveDate::parse_from_str(&request.date, "%Y-%m-%d").is_err() {
                return Err(error(Reason::InvalidArgument, "date must use YYYY-MM-DD"));
            }
        }
        let mut query = BTreeMap::from([("mode".into(), mode.into())]);
        if !request.date.is_empty() {
            query.insert("date".into(), request.date);
        }
        let digest = crate::continuation::query_digest(&query);
        apply_offset(&request.cursor, OPERATION, &digest, &mut query)?;
        let body = self
            .get("/v1/illust/ranking", query.into_iter().collect(), OPERATION)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, OPERATION);
        let envelope: Envelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let values = envelope.illusts.ok_or_else(malformed)?;
        crate::artwork::validate_list(&values, OPERATION)?;
        let offset = envelope
            .next_url
            .as_deref()
            .map(|raw| {
                crate::continuation::next_offset(
                    raw,
                    "v1/illust/ranking",
                    &["offset", "mode", "date"],
                )
                .ok_or_else(malformed)
            })
            .transpose()?;
        let mut items = Vec::with_capacity(values.len());
        for value in values {
            let artwork = crate::artwork::map(value, "Artwork", false, &self.resource_policy)?;
            self.remember_artwork(&artwork);
            items.push(artwork);
        }
        let next = next_cursor(OPERATION, &digest, offset)?;
        Ok(Page { items, next })
    }
}

pub(crate) fn ranking_error(
    operation: &'static str,
    reason: Reason,
    detail: &'static str,
) -> Error {
    Error::new(reason, operation).with_detail(detail)
}
pub(crate) fn apply_offset(
    cursor: &Cursor,
    operation: &'static str,
    digest: &str,
    query: &mut BTreeMap<String, String>,
) -> Result<()> {
    apply_value(
        cursor,
        operation,
        digest,
        query,
        "offset",
        "cursor continuation offset must be positive",
    )
}
pub(crate) fn apply_value(
    cursor: &Cursor,
    operation: &'static str,
    digest: &str,
    query: &mut BTreeMap<String, String>,
    expected_key: &str,
    positive_detail: &'static str,
) -> Result<()> {
    if !cursor.is_zero() {
        cursor
            .validate("pixiv", operation, 1, digest)
            .map_err(|_| {
                ranking_error(
                    operation,
                    Reason::InvalidCursor,
                    "cursor does not match this operation and query",
                )
            })?;
        let payload = cursor.payload().map_err(|_| {
            ranking_error(
                operation,
                Reason::InvalidCursor,
                "cursor payload is unavailable",
            )
        })?;
        let position: Option<Continuation> = serde_json::from_slice(&payload).map_err(|_| {
            ranking_error(
                operation,
                Reason::InvalidCursor,
                "cursor payload is malformed",
            )
        })?;
        let position = position.unwrap_or_default();
        let key = position.k.unwrap_or_default();
        let value = position.v.unwrap_or_default();
        let params = position.p.unwrap_or_default();
        if value < 0
            || position.s.unwrap_or_default() < 0
            || (key.is_empty() && params.is_empty())
            || (!key.is_empty() && !params.is_empty())
        {
            return Err(ranking_error(
                operation,
                Reason::InvalidCursor,
                "cursor payload is malformed",
            ));
        }
        if key != expected_key {
            return Err(ranking_error(
                operation,
                Reason::InvalidCursor,
                "cursor continuation kind mismatch",
            ));
        }
        if value <= 0 || (expected_key == "offset" && value > isize::MAX as i64) {
            return Err(ranking_error(
                operation,
                Reason::InvalidCursor,
                positive_detail,
            ));
        }
        query.insert(expected_key.into(), value.to_string());
    }
    Ok(())
}
pub(crate) fn next_cursor(
    operation: &'static str,
    digest: &str,
    offset: Option<i64>,
) -> Result<Cursor> {
    value_cursor(operation, digest, "offset", offset)
}
pub(crate) fn value_cursor(
    operation: &'static str,
    digest: &str,
    key: &str,
    offset: Option<i64>,
) -> Result<Cursor> {
    match offset {
        Some(offset) => {
            let payload = serde_json::to_vec(&Position { k: key, v: offset }).map_err(|_| {
                ranking_error(operation, Reason::UpstreamError, "cannot encode cursor")
            })?;
            Cursor::new(
                "pixiv",
                operation,
                1,
                digest,
                &payload,
                CursorOptions::default(),
            )
        }
        None => Ok(Cursor::default()),
    }
}
