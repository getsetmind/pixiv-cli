use crate::{
    Client, Error, Reason, Result,
    cursor::{Cursor, CursorOptions, Page},
    models::Artwork,
    transport::Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) type Params = BTreeMap<String, Option<Vec<String>>>;
const RELATED: &str = "RelatedArtworks";
const RECOMMENDED: &str = "RecommendedArtworks";
const RECOMMENDED_KEYS: &[&str] = &[
    "offset",
    "min_bookmark_id_for_recent_illust",
    "max_bookmark_id_for_recommend",
    "include_ranking_illusts",
    "include_privacy_policy",
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelatedArtworksRequest {
    pub artwork_id: i64,
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecommendedArtworksRequest {
    pub cursor: Cursor,
}
#[derive(Default, Deserialize)]
struct State {
    k: Option<String>,
    v: Option<i64>,
    s: Option<i64>,
    p: Option<Params>,
}
#[derive(Serialize)]
struct Position {
    k: &'static str,
    v: i64,
    p: Params,
}
#[derive(Deserialize)]
struct Envelope {
    illusts: Option<Vec<Value>>,
    next_url: Option<String>,
}
fn error(operation: &'static str, reason: Reason, detail: &'static str) -> Error {
    Error::new(reason, operation).with_detail(detail)
}
fn positive(values: Option<&Option<Vec<String>>>) -> bool {
    values
        .and_then(|values| values.as_ref())
        .is_some_and(|values| {
            !values.is_empty()
                && values
                    .iter()
                    .all(|value| value.parse::<i64>().is_ok_and(|value| value > 0))
        })
}
fn valid_params(operation: &str, params: &Params) -> bool {
    if operation == RELATED {
        if params
            .get("illust_id")
            .and_then(|v| v.as_ref())
            .is_none_or(|v| v.len() != 1)
            || !positive(params.get("illust_id"))
        {
            return false;
        }
        if let Some(offset) = params.get("offset") {
            return params.len() == 2
                && offset.as_ref().is_some_and(|values| values.len() == 1)
                && positive(Some(offset));
        }
        return params.len() == 3
            && positive(params.get("seed_illust_ids[]"))
            && positive(params.get("viewed[]"));
    }
    let keys: &[&str] = if operation == "RecommendedNovels" {
        &[
            "offset",
            "already_recommended",
            "max_bookmark_id_for_recommend",
            "include_ranking_novels",
            "include_privacy_policy",
        ]
    } else {
        RECOMMENDED_KEYS
    };
    !params.is_empty()
        && params.iter().all(|(key, values)| {
            let Some(values) = values else {
                return false;
            };
            if !keys.contains(&key.as_str()) || values.len() != 1 || values[0].is_empty() {
                return false;
            }
            let value = &values[0];
            match key.as_str() {
                "offset" => value.parse::<i64>().is_ok_and(|v| v >= 0),
                "min_bookmark_id_for_recent_illust" | "max_bookmark_id_for_recommend" => {
                    value.parse::<i64>().is_ok_and(|v| v > 0)
                }
                "include_ranking_illusts" | "include_ranking_novels" | "include_privacy_policy" => {
                    matches!(value.as_str(), "true" | "false")
                }
                "already_recommended" => true,
                _ => false,
            }
        })
}

fn indexed(params: &Params, prefix: &str) -> Option<Vec<String>> {
    let mut entries = BTreeMap::new();
    for (key, values) in params {
        if let Some(index) = key.strip_prefix(prefix) {
            let index = index.strip_suffix(']')?.parse::<isize>().ok()?;
            if index < 0 || !positive(Some(values)) {
                return None;
            }
            let values = values.as_ref()?;
            if values.len() != 1 {
                return None;
            }
            entries.insert(index as usize, values[0].clone());
        }
    }
    if entries.is_empty() {
        return None;
    }
    (0..entries.len())
        .map(|index| entries.get(&index).cloned())
        .collect()
}
fn next_params(operation: &str, raw: &str) -> Option<Params> {
    if operation == RECOMMENDED {
        return crate::continuation::next_params(
            raw,
            "v1/illust/recommended",
            RECOMMENDED_KEYS,
            &[],
            &["viewed["],
        );
    }
    let params = crate::continuation::next_params(
        raw,
        "v2/illust/related",
        &["offset", "illust_id"],
        &["seed_illust_ids[", "viewed["],
        &[],
    )?;
    if let Some(offset) = params
        .get("offset")
        .and_then(|v| v.as_ref())
        .and_then(|v| v.first())
    {
        let value = offset.parse::<i64>().ok()?;
        return (value > 0 && value <= isize::MAX as i64).then_some(params);
    }
    let seed = indexed(&params, "seed_illust_ids[")?;
    let viewed = indexed(&params, "viewed[")?;
    Some(BTreeMap::from([
        (
            "illust_id".into(),
            params.get("illust_id").cloned().unwrap_or_default(),
        ),
        ("seed_illust_ids[]".into(), Some(seed)),
        ("viewed[]".into(), Some(viewed)),
    ]))
}
impl<T: Transport> Client<T> {
    pub(crate) fn artwork_feed_params(
        &self,
        operation: &'static str,
        digest: &str,
        cursor: &Cursor,
    ) -> Result<Option<Params>> {
        if cursor.is_zero() {
            return Ok(None);
        }
        let invalid = |detail| error(operation, Reason::InvalidCursor, detail);
        cursor
            .validate("pixiv", operation, 2, digest)
            .map_err(|_| invalid("cursor does not match this operation and query"))?;
        if let Some(identity) = cursor.identity() {
            if identity != self.user_id.to_string() {
                return Err(invalid("cursor belongs to a different account"));
            }
        } else {
            cursor
                .validate_instance(self.cursor_instance.as_deref().unwrap_or_default())
                .map_err(|_| invalid("cursor belongs to a different client instance"))?;
        }
        let payload = cursor
            .payload()
            .map_err(|_| invalid("cursor payload is unavailable"))?;
        let state: Option<State> =
            serde_json::from_slice(&payload).map_err(|_| invalid("cursor payload is malformed"))?;
        let state = state.unwrap_or_default();
        let key = state.k.unwrap_or_default();
        let params = state.p.unwrap_or_default();
        if state.v.unwrap_or_default() < 0
            || state.s.unwrap_or_default() < 0
            || (key.is_empty() && params.is_empty())
            || (!key.is_empty() && !params.is_empty())
        {
            return Err(invalid("cursor payload is malformed"));
        }
        if params.is_empty() {
            return Err(invalid("cursor continuation params are missing"));
        }
        if !valid_params(operation, &params) {
            return Err(invalid("cursor continuation params are malformed"));
        }
        Ok(Some(params))
    }
    pub async fn related_artworks(&self, request: RelatedArtworksRequest) -> Result<Page<Artwork>> {
        if request.artwork_id <= 0 {
            return Err(error(
                RELATED,
                Reason::InvalidArgument,
                "artwork ID must be positive",
            ));
        }
        let query = BTreeMap::from([("illust_id".into(), request.artwork_id.to_string())]);
        self.artwork_feed(RELATED, "/v2/illust/related", query, request.cursor)
            .await
    }
    pub async fn recommended_artworks(
        &self,
        request: RecommendedArtworksRequest,
    ) -> Result<Page<Artwork>> {
        self.artwork_feed(
            RECOMMENDED,
            "/v1/illust/recommended",
            BTreeMap::new(),
            request.cursor,
        )
        .await
    }
    async fn artwork_feed(
        &self,
        operation: &'static str,
        endpoint: &str,
        query: BTreeMap<String, String>,
        cursor: Cursor,
    ) -> Result<Page<Artwork>> {
        let digest = crate::continuation::query_digest(&query);
        let params = self.artwork_feed_params(operation, &digest, &cursor)?;
        if operation == RELATED
            && let Some(params) = &params
        {
            let id = params
                .get("illust_id")
                .and_then(|v| v.as_ref())
                .and_then(|v| v.first());
            if id != query.get("illust_id") {
                return Err(Error::new(Reason::UpstreamError, operation).with_cause(
                    crate::error::Cause::Redacted("pixiv upstream request failed".into()),
                ));
            }
        }
        let parameters = match params {
            Some(params) => params
                .into_iter()
                .flat_map(|(key, values)| {
                    values
                        .unwrap_or_default()
                        .into_iter()
                        .map(move |value| (key.clone(), value))
                })
                .collect(),
            None => query.into_iter().collect(),
        };
        let body = self.get(endpoint, parameters, operation).await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
        let envelope: Envelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let values = envelope.illusts.ok_or_else(malformed)?;
        crate::artwork::validate_list(&values, operation)?;
        let params = envelope
            .next_url
            .as_deref()
            .map(|raw| next_params(operation, raw).ok_or_else(malformed))
            .transpose()?;
        let mut items = Vec::with_capacity(values.len());
        for value in values {
            let artwork = crate::artwork::map(value, "Artwork", false, &self.resource_policy)?;
            self.remember_artwork(&artwork);
            items.push(artwork);
        }
        let next = if let Some(params) = params {
            let options = if self.user_id > 0 {
                CursorOptions {
                    identity: self.user_id.to_string(),
                    ..Default::default()
                }
            } else {
                CursorOptions {
                    instance: Some(self.cursor_instance.clone().ok_or_else(|| {
                        error(
                            operation,
                            Reason::LocalStateError,
                            "cursor instance is not configured",
                        )
                    })?),
                    ..Default::default()
                }
            };
            let payload = serde_json::to_vec(&Position {
                k: "",
                v: 0,
                p: params,
            })
            .map_err(|_| error(operation, Reason::UpstreamError, "cannot encode cursor"))?;
            Cursor::new("pixiv", operation, 2, &digest, &payload, options)?
        } else {
            Cursor::default()
        };
        Ok(Page { items, next })
    }
}
