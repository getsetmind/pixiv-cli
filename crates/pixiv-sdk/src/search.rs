use crate::continuation::query_digest;
use crate::{
    Client, Error, Reason, Result,
    cursor::{Cursor, CursorOptions, Page},
    models::Artwork,
    transport::Transport,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const OPERATION: &str = "SearchArtworks";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchArtworksRequest {
    pub word: String,
    pub target: String,
    pub sort: String,
    pub duration: String,
    pub start_date: String,
    pub end_date: String,
    pub content_type: String,
    pub ai_mode: String,
    pub aspect_ratio: String,
    pub resolution: String,
    pub tool: String,
    pub bookmark_min: Option<i64>,
    pub bookmark_max: Option<i64>,
    pub cursor_context: String,
    pub cursor: Cursor,
}

fn error(reason: Reason, detail: &str) -> Error {
    Error::new(reason, OPERATION).with_detail(detail)
}
fn date(raw: &str, field: &str) -> Result<Option<NaiveDate>> {
    if raw.is_empty() {
        return Ok(None);
    }
    let valid = raw.len() == 10
        && raw.bytes().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        });
    if valid && let Ok(value) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Ok(Some(value));
    }
    Err(error(
        Reason::InvalidArgument,
        &format!("{field} date must use YYYY-MM-DD"),
    ))
}
fn queries(
    request: &SearchArtworksRequest,
) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>)> {
    if request.word.trim().is_empty() {
        return Err(error(Reason::InvalidArgument, "search word is required"));
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
        (
            &request.content_type,
            &["", "all", "illust-and-ugoira", "illust", "manga", "ugoira"][..],
            "content type is unsupported",
        ),
        (
            &request.ai_mode,
            &["", "all", "exclude", "only"][..],
            "AI mode is unsupported",
        ),
        (
            &request.aspect_ratio,
            &["", "all", "landscape", "portrait", "square"][..],
            "aspect ratio is unsupported",
        ),
        (
            &request.resolution,
            &["", "all", "high", "medium", "low"][..],
            "resolution is unsupported",
        ),
    ] {
        if !allowed.contains(&value.as_str()) {
            return Err(error(Reason::InvalidArgument, detail));
        }
    }
    let start = date(&request.start_date, "start")?;
    let end = date(&request.end_date, "end")?;
    if start.zip(end).is_some_and(|(start, end)| start > end) {
        return Err(error(
            Reason::InvalidArgument,
            "start date must not be later than end date",
        ));
    }
    if request.bookmark_min.is_some_and(|min| min < 0) {
        return Err(error(
            Reason::InvalidArgument,
            "bookmark minimum must be non-negative",
        ));
    }
    if request.bookmark_max.is_some_and(|max| max < 0) {
        return Err(error(
            Reason::InvalidArgument,
            "bookmark maximum must be non-negative",
        ));
    }
    if request
        .bookmark_min
        .zip(request.bookmark_max)
        .is_some_and(|(min, max)| min > max)
    {
        return Err(error(
            Reason::InvalidArgument,
            "bookmark minimum must not exceed maximum",
        ));
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
    let mut digest = BTreeMap::from([
        ("word".into(), request.word.clone()),
        ("search_target".into(), target.into()),
        ("sort".into(), sort.into()),
    ]);
    for (key, value, skip_all) in [
        ("duration", &request.duration, false),
        ("start_date", &request.start_date, false),
        ("end_date", &request.end_date, false),
        ("content_type", &request.content_type, true),
        ("ai_mode", &request.ai_mode, true),
        ("ratio_pattern", &request.aspect_ratio, true),
        ("resolution", &request.resolution, true),
        ("tool", &request.tool, false),
        ("cursor_context", &request.cursor_context, false),
    ] {
        if !(value.is_empty() || skip_all && value == "all") {
            digest.insert(key.into(), value.clone());
        }
    }
    for (key, value) in [
        ("bookmark_num_min", request.bookmark_min),
        ("bookmark_num_max", request.bookmark_max),
    ] {
        if let Some(value) = value {
            digest.insert(key.into(), value.to_string());
        }
    }
    let mut wire = digest.clone();
    wire.remove("cursor_context");
    wire.remove("ai_mode");
    wire.remove("resolution");
    wire.insert(
        "search_ai_type".into(),
        if request.ai_mode == "exclude" {
            "1"
        } else {
            "0"
        }
        .into(),
    );
    if request.content_type == "illust-and-ugoira" {
        wire.insert("content_type".into(), "illust_and_ugoira".into());
    }
    let bounds: &[(&str, &str)] = match request.resolution.as_str() {
        "high" => &[("width_min", "3000"), ("height_min", "3000")],
        "medium" => &[
            ("width_min", "1000"),
            ("height_min", "1000"),
            ("width_max", "2999"),
            ("height_max", "2999"),
        ],
        "low" => &[("width_max", "999"), ("height_max", "999")],
        _ => &[],
    };
    for (key, value) in bounds {
        wire.insert((*key).into(), (*value).into());
    }
    Ok((digest, wire))
}

#[derive(Default, Deserialize)]
struct Continuation {
    k: Option<String>,
    v: Option<i64>,
    s: Option<i64>,
    p: Option<BTreeMap<String, Option<Vec<String>>>>,
}
#[derive(Serialize)]
struct Position {
    k: &'static str,
    v: i64,
    #[serde(skip_serializing_if = "is_zero")]
    s: i64,
}
fn is_zero(value: &i64) -> bool {
    *value == 0
}
#[derive(Deserialize)]
struct Envelope {
    illusts: Option<Vec<Value>>,
    next_url: Option<String>,
}

impl<T: Transport> Client<T> {
    fn search_position(&self, cursor: &Cursor, digest: &str) -> Result<Position> {
        if cursor.is_zero() {
            return Ok(Position {
                k: "offset",
                v: 0,
                s: 0,
            });
        }
        cursor
            .validate("pixiv", OPERATION, 2, digest)
            .map_err(|_| {
                error(
                    Reason::InvalidCursor,
                    "cursor does not match this operation and query",
                )
            })?;
        if let Some(identity) = cursor.identity() {
            if identity != self.user_id.to_string() {
                return Err(error(
                    Reason::InvalidCursor,
                    "cursor belongs to a different account",
                ));
            }
        } else {
            cursor
                .validate_instance(self.cursor_instance.as_deref().unwrap_or_default())
                .map_err(|_| {
                    error(
                        Reason::InvalidCursor,
                        "cursor belongs to a different client instance",
                    )
                })?;
        }
        let payload = cursor
            .payload()
            .map_err(|_| error(Reason::InvalidCursor, "cursor payload is unavailable"))?;
        let state: Option<Continuation> = serde_json::from_slice(&payload)
            .map_err(|_| error(Reason::InvalidCursor, "cursor payload is malformed"))?;
        let state = state.unwrap_or_default();
        let key = state.k.unwrap_or_default();
        let value = state.v.unwrap_or_default();
        let consumed = state.s.unwrap_or_default();
        let params = state.p.unwrap_or_default();
        if value < 0
            || consumed < 0
            || (key.is_empty() && params.is_empty())
            || (!key.is_empty() && !params.is_empty())
        {
            return Err(error(Reason::InvalidCursor, "cursor payload is malformed"));
        }
        if key != "offset" || value > isize::MAX as i64 {
            return Err(error(
                Reason::InvalidCursor,
                "cursor continuation kind or position mismatch",
            ));
        }
        Ok(Position {
            k: "offset",
            v: value,
            s: consumed,
        })
    }
    fn search_cursor(&self, digest: &str, position: Position) -> Result<Cursor> {
        let options = if self.user_id > 0 {
            CursorOptions {
                identity: self.user_id.to_string(),
                ..Default::default()
            }
        } else {
            CursorOptions {
                instance: Some(self.cursor_instance.clone().ok_or_else(|| {
                    error(Reason::LocalStateError, "cursor instance is not configured")
                })?),
                ..Default::default()
            }
        };
        let payload = serde_json::to_vec(&position)
            .map_err(|_| error(Reason::UpstreamError, "cannot encode cursor"))?;
        Cursor::new("pixiv", OPERATION, 2, digest, &payload, options)
    }
    pub fn checkpoint_search_artworks(
        &self,
        request: SearchArtworksRequest,
        consumed: i64,
    ) -> Result<Cursor> {
        let (query, _) = queries(&request)?;
        if consumed <= 0 {
            return Err(error(Reason::InvalidArgument, "consumed must be positive"));
        }
        let digest = query_digest(&query);
        let mut position = self.search_position(&request.cursor, &digest)?;
        position.s = position
            .s
            .checked_add(consumed)
            .filter(|value| *value <= isize::MAX as i64)
            .ok_or_else(|| error(Reason::InvalidArgument, "consumed position overflows"))?;
        self.search_cursor(&digest, position)
    }
    pub async fn search_artworks(&self, request: SearchArtworksRequest) -> Result<Page<Artwork>> {
        let (query, mut wire) = queries(&request)?;
        let digest = query_digest(&query);
        let position = self.search_position(&request.cursor, &digest)?;
        if position.v > 0 {
            wire.insert("offset".into(), position.v.to_string());
        }
        let body = self
            .get("/v1/search/illust", wire.into_iter().collect(), OPERATION)
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
                    "v1/search/illust",
                    &[
                        "offset",
                        "word",
                        "search_target",
                        "sort",
                        "duration",
                        "start_date",
                        "end_date",
                        "search_ai_type",
                        "ratio_pattern",
                        "content_type",
                        "tool",
                        "bookmark_num_min",
                        "bookmark_num_max",
                        "width_min",
                        "width_max",
                        "height_min",
                        "height_max",
                    ],
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
        let next = if let Some(offset) = offset {
            self.search_cursor(
                &digest,
                Position {
                    k: "offset",
                    v: offset,
                    s: 0,
                },
            )?
        } else {
            Cursor::default()
        };
        if request.ai_mode == "only" {
            items.retain(|artwork| artwork.ai_type == 2);
        }
        let consumed = usize::try_from(position.s).map_err(|_| {
            error(
                Reason::InvalidCursor,
                "checkpoint exceeds the current batch; restart pagination",
            )
        })?;
        if consumed > items.len() {
            return Err(error(
                Reason::InvalidCursor,
                "checkpoint exceeds the current batch; restart pagination",
            ));
        }
        items.drain(..consumed);
        Ok(Page { items, next })
    }
}
