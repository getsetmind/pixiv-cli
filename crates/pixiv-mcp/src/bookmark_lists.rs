use crate::{CallToolResult, IllustFilter, SearchIllustInput};
use pixiv_app::{pagination, scheduler::SchedulerError};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::{Artwork, Novel},
    pixiv::{UserArtworkBookmarksRequest, UserNovelBookmarksRequest},
    transport::Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookmarkList {
    Artwork,
    Novel,
    All,
}
impl BookmarkList {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "user_bookmarks" => Some(Self::Artwork),
            "user_novel_bookmarks" => Some(Self::Novel),
            "bookmark_list_all" => Some(Self::All),
            _ => None,
        }
    }
    pub(crate) fn operation(self) -> &'static str {
        match self {
            Self::Artwork => "UserArtworkBookmarks",
            Self::Novel => "UserNovelBookmarks",
            Self::All => "UserArtworkBookmarks",
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct BookmarkListInput {
    pub user_id: i64,
    pub restrict: String,
    pub tag: String,
    pub illust_filter: Option<IllustFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn bookmark_list_tool(kind: BookmarkList) -> Value {
    serde_json::from_str(match kind {
        BookmarkList::Artwork => include_str!("../schemas/user-bookmarks.json"),
        BookmarkList::Novel => include_str!("../schemas/user-novel-bookmarks.json"),
        BookmarkList::All => include_str!("../schemas/bookmark-list-all.json"),
    })
    .expect("bookmark list schema is valid JSON")
}
pub(crate) fn decode(
    kind: BookmarkList,
    arguments: Option<&Value>,
) -> Result<BookmarkListInput, String> {
    let mut arguments = arguments
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            match &arguments {
                Value::Number(_) => "number",
                Value::Bool(_) => "bool",
                value => crate::stdio::value_type(value),
            }
        ));
    }
    crate::search::validate_schema_with_bindings(
        &mut arguments,
        &bookmark_list_tool(kind)["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        &|path| {
            (
                "In",
                if path.ends_with("/user_id") || path.ends_with("/id") {
                    "int64"
                } else {
                    "int"
                },
            )
        },
    )?;
    serde_json::from_value(arguments).map_err(|e| format!("invalid params: {e}"))
}
fn plan(kind: BookmarkList, input: &BookmarkListInput) -> Result<crate::search::Plan, String> {
    crate::search::validate(&mut SearchIllustInput {
        page: input.page,
        limit: input.limit,
        illust_filter: if kind == BookmarkList::Artwork {
            input.illust_filter.clone()
        } else {
            None
        },
        ..Default::default()
    })
}
fn resolve<T: Transport>(client: &Client<T>, id: i64) -> Result<i64, SchedulerError> {
    if id > 0 {
        Ok(id)
    } else if id < 0 {
        Err(SchedulerError::Message(
            "user_id must be positive when provided".into(),
        ))
    } else if client.user_id() > 0 {
        Ok(client.user_id())
    } else {
        Err(SchedulerError::Message(
            "cannot determine current user id".into(),
        ))
    }
}
pub async fn bookmark_list<T: Transport>(
    client: &Client<T>,
    kind: BookmarkList,
    input: BookmarkListInput,
) -> CallToolResult {
    let prior = if kind == BookmarkList::Novel {
        None
    } else {
        match plan(kind, &input) {
            Ok(p) => Some(p),
            Err(e) => return crate::search::failure(e),
        }
    };
    let id = match resolve(client, input.user_id) {
        Ok(id) => id,
        Err(e) => return crate::search::failure(e.to_string()),
    };
    let plan = match prior.map(Ok).unwrap_or_else(|| plan(kind, &input)) {
        Ok(p) => p,
        Err(e) => return crate::search::failure(e),
    };
    result(
        collect(client, kind, id, &input, &plan).await,
        input.limit,
        &plan,
    )
}
pub(crate) async fn saved_list<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    kind: BookmarkList,
    input: BookmarkListInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let prior = if kind == BookmarkList::Novel {
        None
    } else {
        match plan(kind, &input) {
            Ok(p) => Some(p),
            Err(e) => return crate::search::failure(e),
        }
    };
    let id = if input.user_id != 0 {
        resolve_id(input.user_id)
    } else {
        execution
            .read(
                context,
                0,
                proxy,
                |_, client| async move { resolve(&client, 0) },
            )
            .await
    };
    let id = match id {
        Ok(id) => id,
        Err(e) => return crate::search::failure(e.to_string()),
    };
    let plan = match prior.map(Ok).unwrap_or_else(|| plan(kind, &input)) {
        Ok(p) => p,
        Err(e) => return crate::search::failure(e),
    };
    let limit = input.limit;
    let collected = execution
        .read(context, 0, proxy, move |_, client| {
            let input = input.clone();
            async move { collect(&client, kind, id, &input, &plan).await }
        })
        .await;
    result(collected, limit, &plan)
}
fn resolve_id(id: i64) -> Result<i64, SchedulerError> {
    if id > 0 {
        Ok(id)
    } else {
        Err(SchedulerError::Message(
            "user_id must be positive when provided".into(),
        ))
    }
}
enum Item {
    Artwork(Artwork),
    Novel(Novel),
}
fn result(
    output: Result<(Vec<Item>, bool), SchedulerError>,
    limit: Option<i64>,
    plan: &crate::search::Plan,
) -> CallToolResult {
    match output {
        Ok((items, more)) => {
            let mut records = vec![];
            for item in items {
                let mut record = match match item {
                    Item::Artwork(a) => pixiv_record::from_artwork(&a),
                    Item::Novel(n) => pixiv_record::from_novel(&n),
                } {
                    Ok(r) => r,
                    Err(e) => return crate::search::failure(e.to_string()),
                };
                crate::structured_wire_numbers(&mut record);
                records.push(record)
            }
            crate::search::list_result(records, more, None, limit, plan)
        }
        Err(e) => crate::search::failure(e.to_string()),
    }
}
async fn fetch<T: Transport>(
    client: &Client<T>,
    artwork: bool,
    id: i64,
    input: &BookmarkListInput,
    cursor: Cursor,
) -> Result<(Vec<Item>, Cursor), SchedulerError> {
    if artwork {
        let page = client
            .user_artwork_bookmarks(UserArtworkBookmarksRequest {
                user_id: id,
                restrict: input.restrict.clone(),
                tag: input.tag.clone(),
                cursor,
            })
            .await
            .map_err(SchedulerError::from)?;
        Ok((
            page.items.into_iter().map(Item::Artwork).collect(),
            page.next,
        ))
    } else {
        let page = client
            .user_novel_bookmarks(UserNovelBookmarksRequest {
                user_id: id,
                restrict: input.restrict.clone(),
                tag: input.tag.clone(),
                cursor,
            })
            .await
            .map_err(SchedulerError::from)?;
        Ok((page.items.into_iter().map(Item::Novel).collect(), page.next))
    }
}
fn source(error: pagination::Failure<SchedulerError>) -> SchedulerError {
    match error.cause {
        pagination::Cause::Source(e) => e,
        pagination::Cause::Message(e) => SchedulerError::Message(e),
    }
}
async fn collect<T: Transport>(
    client: &Client<T>,
    kind: BookmarkList,
    id: i64,
    input: &BookmarkListInput,
    plan: &crate::search::Plan,
) -> Result<(Vec<Item>, bool), SchedulerError> {
    let page_plan = pagination::Plan {
        skip: plan.skip,
        limit: plan.limit.max(0),
        one_batch: plan.one_batch,
    };
    if kind == BookmarkList::All {
        let mut streams = [true, false].map(|artwork| pagination::Stream {
            fetch: move |cursor: BookmarkCursor| async move {
                let (mut items, next) = fetch(client, artwork, id, input, cursor.upstream).await?;
                if cursor.consumed > items.len() {
                    return Err(SchedulerError::Message(format!(
                        "{} bookmark stream checkpoint exceeds batch",
                        if artwork { "artwork" } else { "novel" }
                    )));
                }
                items.drain(..cursor.consumed);
                Ok((items, BookmarkCursor::new(next, 0)))
            },
            include: |_: &Item| Ok(true),
            checkpoint: |cursor: BookmarkCursor, position: usize| {
                if position == 0 {
                    return Err(SchedulerError::Message(
                        "bookmark stream checkpoint position must be positive".into(),
                    ));
                }
                Ok(BookmarkCursor::new(
                    cursor.upstream,
                    cursor.consumed + position,
                ))
            },
        });
        let page = pagination::collect_streams(
            page_plan,
            &mut streams,
            pagination::StreamState::default(),
        )
        .await
        .map_err(source)?;
        return Ok((page.items, page.result.has_more));
    }
    let local = pixiv_app::search_filter::normalize_filter(
        "",
        if kind == BookmarkList::Artwork {
            input
                .illust_filter
                .as_ref()
                .map(|f| f.r#type.as_str())
                .unwrap_or_default()
        } else {
            ""
        },
    )
    .map_err(|e| SchedulerError::Message(e.to_string()))?;
    let mut seen = BTreeSet::new();
    let page = pagination::collect_pages(
        page_plan,
        Cursor::default(),
        |cursor| fetch(client, kind == BookmarkList::Artwork, id, input, cursor),
        |item: &Item| {
            Ok(match item {
                Item::Artwork(a) => {
                    input
                        .illust_filter
                        .as_ref()
                        .is_none_or(|f| crate::search::matches(a, f, &local))
                        && seen.insert(format!("{:?}:{}", a.kind, a.id))
                }
                Item::Novel(n) => seen.insert(format!("novel:{}", n.id)),
            })
        },
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(source)?;
    Ok((page.items, page.result.has_more))
}
#[derive(Clone, Default)]
struct BookmarkCursor {
    upstream: Cursor,
    consumed: usize,
    text: String,
}
impl BookmarkCursor {
    fn new(upstream: Cursor, consumed: usize) -> Self {
        let text = if consumed == 0 && upstream.is_zero() {
            String::new()
        } else {
            format!("{}#{consumed}", upstream.as_str())
        };
        Self {
            upstream,
            consumed,
            text,
        }
    }
}
impl pagination::Cursor for BookmarkCursor {
    fn is_zero(&self) -> bool {
        self.consumed == 0 && self.upstream.is_zero()
    }
    fn text(&self) -> &str {
        &self.text
    }
}
