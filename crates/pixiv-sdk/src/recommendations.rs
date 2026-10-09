use crate::{
    Client, Error, Reason, Result,
    artwork::WireUser,
    artwork_feeds::Params,
    cursor::{Cursor, CursorOptions, Page},
    models::{Novel, UserPreview},
    transport::Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
const NOVELS: &str = "RecommendedNovels";
const USERS: &str = "RecommendedUsers";
const NOVEL_KEYS: &[&str] = &[
    "offset",
    "already_recommended",
    "max_bookmark_id_for_recommend",
    "include_ranking_novels",
    "include_privacy_policy",
];
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecommendedNovelsRequest {
    pub cursor: Cursor,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecommendedUsersRequest {
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
struct Position<'a> {
    k: &'a str,
    v: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    p: Option<Params>,
}
#[derive(Deserialize)]
struct NovelEnvelope {
    novels: Option<Vec<Value>>,
    next_url: Option<String>,
}
#[derive(Deserialize)]
struct Preview {
    user: Option<WireUser>,
    illusts: Option<Vec<Value>>,
    novels: Option<Vec<Value>>,
}
#[derive(Deserialize)]
struct UserEnvelope {
    user_previews: Option<Vec<Option<Preview>>>,
    next_url: Option<String>,
}
#[derive(Deserialize)]
struct SamplePageFields {
    #[serde(rename = "page_index")]
    _page_index: Option<i64>,
    #[serde(rename = "extension")]
    _extension: Option<String>,
}
fn decode_sample_artwork(mut value: Value) -> Result<Value> {
    let malformed = || Error::new(Reason::MalformedUpstreamResponse, USERS);
    if let Some(tools) = value.get_mut("tools").filter(|tools| !tools.is_null()) {
        let decoded: Vec<Option<String>> =
            serde_json::from_value(tools.clone()).map_err(|_| malformed())?;
        *tools = serde_json::to_value(
            decoded
                .into_iter()
                .map(Option::unwrap_or_default)
                .collect::<Vec<_>>(),
        )
        .map_err(|_| malformed())?;
    }
    if let Some(pages) = value.get_mut("meta_pages").filter(|pages| !pages.is_null()) {
        let decoded: Vec<Option<SamplePageFields>> =
            serde_json::from_value(pages.clone()).map_err(|_| malformed())?;
        let values = pages.as_array_mut().ok_or_else(malformed)?;
        for (value, decoded) in values.iter_mut().zip(decoded) {
            if decoded.is_none() {
                *value = serde_json::json!({});
            }
        }
    }
    Ok(value)
}
fn invalid(operation: &'static str, detail: &'static str) -> Error {
    Error::new(Reason::InvalidCursor, operation).with_detail(detail)
}
impl<T: Transport> Client<T> {
    fn recommendation_cursor(
        &self,
        operation: &'static str,
        position: Position<'_>,
    ) -> Result<Cursor> {
        let options = if self.user_id > 0 {
            CursorOptions {
                identity: self.user_id.to_string(),
                ..Default::default()
            }
        } else {
            CursorOptions {
                instance: Some(self.cursor_instance.clone().ok_or_else(|| {
                    Error::new(Reason::LocalStateError, operation)
                        .with_detail("cursor instance is not configured")
                })?),
                ..Default::default()
            }
        };
        let payload = serde_json::to_string(&position).map_err(|_| {
            Error::new(Reason::UpstreamError, operation).with_detail("cannot encode cursor")
        })?;
        let payload = payload
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
            .replace('&', "\\u0026")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        Cursor::new(
            "pixiv",
            operation,
            if operation == NOVELS { 2 } else { 1 },
            &crate::continuation::query_digest(&BTreeMap::new()),
            payload.as_bytes(),
            options,
        )
    }
    pub async fn recommended_novels(
        &self,
        request: RecommendedNovelsRequest,
    ) -> Result<Page<Novel>> {
        let digest = crate::continuation::query_digest(&BTreeMap::new());
        let params = self.artwork_feed_params(NOVELS, &digest, &request.cursor)?;
        let parameters = params
            .unwrap_or_default()
            .into_iter()
            .flat_map(|(key, values)| {
                values
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |value| (key.clone(), value))
            })
            .collect();
        let body = self
            .get("/v1/novel/recommended", parameters, NOVELS)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, NOVELS);
        let envelope: NovelEnvelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let novels = crate::novel::decode_list(envelope.novels.ok_or_else(malformed)?, NOVELS)?;
        let params = envelope
            .next_url
            .as_deref()
            .map(|raw| {
                crate::continuation::next_params(raw, "v1/novel/recommended", NOVEL_KEYS, &[], &[])
                    .ok_or_else(malformed)
            })
            .transpose()?;
        let mut items = Vec::with_capacity(novels.len());
        for novel in novels {
            let novel = novel.map(&self.resource_policy)?;
            self.remember_resource(&novel.cover.resource);
            self.remember_resource(&novel.user.profile_image.resource);
            items.push(novel);
        }
        let next = match params {
            Some(params) => self.recommendation_cursor(
                NOVELS,
                Position {
                    k: "",
                    v: 0,
                    p: Some(params),
                },
            )?,
            None => Cursor::default(),
        };
        Ok(Page { items, next })
    }
    pub async fn recommended_users(
        &self,
        request: RecommendedUsersRequest,
    ) -> Result<Page<UserPreview>> {
        let mut query = Vec::new();
        if !request.cursor.is_zero() {
            request
                .cursor
                .validate(
                    "pixiv",
                    USERS,
                    1,
                    &crate::continuation::query_digest(&BTreeMap::new()),
                )
                .map_err(|_| invalid(USERS, "cursor does not match this operation and query"))?;
            if let Some(identity) = request.cursor.identity() {
                if identity != self.user_id.to_string() {
                    return Err(invalid(USERS, "cursor belongs to a different account"));
                }
            } else {
                request
                    .cursor
                    .validate_instance(self.cursor_instance.as_deref().unwrap_or_default())
                    .map_err(|_| invalid(USERS, "cursor belongs to a different client instance"))?;
            }
            let payload = request
                .cursor
                .payload()
                .map_err(|_| invalid(USERS, "cursor payload is unavailable"))?;
            let state: Option<State> = serde_json::from_slice(&payload)
                .map_err(|_| invalid(USERS, "cursor payload is malformed"))?;
            let state = state.unwrap_or_default();
            let key = state.k.unwrap_or_default();
            let params = state.p.unwrap_or_default();
            let value = state.v.unwrap_or_default();
            if value < 0
                || state.s.unwrap_or_default() < 0
                || (key.is_empty() && params.is_empty())
                || (!key.is_empty() && !params.is_empty())
            {
                return Err(invalid(USERS, "cursor payload is malformed"));
            }
            if key != "offset" {
                return Err(invalid(USERS, "cursor continuation kind mismatch"));
            }
            query.push(("offset".into(), value.to_string()));
        }
        let body = self.get("/v1/user/recommended", query, USERS).await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, USERS);
        let envelope: UserEnvelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let previews = envelope.user_previews.ok_or_else(malformed)?;
        let mut decoded = Vec::with_capacity(previews.len());
        for preview in previews {
            let preview = preview.ok_or_else(malformed)?;
            let user = preview.user.ok_or_else(malformed)?;
            if user.id.unwrap_or_default() <= 0 {
                return Err(malformed());
            }
            let illusts = preview
                .illusts
                .unwrap_or_default()
                .into_iter()
                .map(decode_sample_artwork)
                .collect::<Result<Vec<_>>>()?;
            crate::artwork::validate_list(&illusts, USERS)?;
            if illusts.iter().any(|item| {
                item.get("user")
                    .and_then(|user| user.get("id"))
                    .and_then(Value::as_i64)
                    .unwrap_or_default()
                    <= 0
            }) {
                return Err(malformed());
            }
            let novels = crate::novel::decode_list(preview.novels.unwrap_or_default(), USERS)?;
            decoded.push((user, illusts, novels));
        }
        let offset = envelope
            .next_url
            .as_deref()
            .map(|raw| {
                crate::continuation::next_zero_offset(raw, "v1/user/recommended", &["offset"])
                    .ok_or_else(malformed)
            })
            .transpose()?;
        let mut items = Vec::with_capacity(decoded.len());
        for (user, illusts, novels) in decoded {
            let user = user.map(&self.resource_policy);
            self.remember_resource(&user.profile_image.resource);
            let mut preview = UserPreview {
                user,
                ..Default::default()
            };
            for illust in illusts {
                if let Ok(artwork) =
                    crate::artwork::map(illust, "Artwork", false, &self.resource_policy)
                {
                    self.remember_artwork(&artwork);
                    preview.illusts.push(artwork);
                }
            }
            for novel in novels {
                if let Ok(novel) = novel.map(&self.resource_policy) {
                    self.remember_resource(&novel.cover.resource);
                    self.remember_resource(&novel.user.profile_image.resource);
                    preview.novels.push(novel);
                }
            }
            items.push(preview);
        }
        let next = match offset {
            Some(offset) => self.recommendation_cursor(
                USERS,
                Position {
                    k: "offset",
                    v: offset,
                    p: None,
                },
            )?,
            None => Cursor::default(),
        };
        Ok(Page { items, next })
    }
}
