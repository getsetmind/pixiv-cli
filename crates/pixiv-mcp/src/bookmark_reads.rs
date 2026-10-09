use crate::{CallToolResult, SearchIllustInput, TextContent};
use pixiv_app::{pagination, scheduler::SchedulerError};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::BookmarkTag,
    pixiv::{ArtworkBookmarkRequest, NovelBookmarkRequest, UserArtworkBookmarkTagsRequest},
    transport::Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookmarkRead {
    ArtworkDetail,
    NovelDetail,
    ArtworkTags,
    NovelTags,
    AllTags,
}
impl BookmarkRead {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "bookmark_detail" => Some(Self::ArtworkDetail),
            "novel_bookmark_detail" => Some(Self::NovelDetail),
            "bookmark_tags" => Some(Self::ArtworkTags),
            "novel_bookmark_tags" => Some(Self::NovelTags),
            "bookmark_tags_all" => Some(Self::AllTags),
            _ => None,
        }
    }
    pub(crate) fn operation(self) -> &'static str {
        match self {
            Self::ArtworkDetail => "ArtworkBookmark",
            Self::NovelDetail => "NovelBookmark",
            Self::ArtworkTags | Self::AllTags => "UserArtworkBookmarkTags",
            Self::NovelTags => "UserNovelBookmarkTags",
        }
    }
    fn detail(self) -> bool {
        matches!(self, Self::ArtworkDetail | Self::NovelDetail)
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct BookmarkReadInput {
    pub illust_id: i64,
    pub novel_id: i64,
    pub user_id: i64,
    pub restrict: String,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
#[derive(Debug, Serialize)]
pub struct BookmarkDetailOutput {
    pub bookmarked: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub restrict: String,
    pub tags: Option<Vec<String>>,
}
#[derive(Debug, Serialize)]
pub struct BookmarkTagOutput {
    pub name: String,
    #[serde(serialize_with = "serialize_count")]
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<&'static str>,
}
#[derive(Debug, Serialize)]
pub struct BookmarkTagsOutput {
    pub bookmark_tags: Vec<BookmarkTagOutput>,
    pub pagination: Value,
}
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum BookmarkReadOutput {
    Detail(BookmarkDetailOutput),
    Tags(BookmarkTagsOutput),
}
pub fn bookmark_read_tool(kind: BookmarkRead) -> Value {
    serde_json::from_str(match kind {
        BookmarkRead::ArtworkDetail => include_str!("../schemas/bookmark-detail.json"),
        BookmarkRead::NovelDetail => include_str!("../schemas/novel-bookmark-detail.json"),
        BookmarkRead::ArtworkTags => include_str!("../schemas/bookmark-tags.json"),
        BookmarkRead::NovelTags => include_str!("../schemas/novel-bookmark-tags.json"),
        BookmarkRead::AllTags => include_str!("../schemas/bookmark-tags-all.json"),
    })
    .expect("bookmark read schema is valid JSON")
}
pub(crate) fn decode(
    kind: BookmarkRead,
    arguments: Option<&Value>,
) -> Result<BookmarkReadInput, String> {
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
                v => crate::stdio::value_type(v),
            }
        ));
    }
    crate::search::validate_schema_with_bindings(
        &mut arguments,
        &bookmark_read_tool(kind)["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        &|path| {
            (
                "In",
                if path.ends_with("_id") {
                    "int64"
                } else {
                    "int"
                },
            )
        },
    )?;
    serde_json::from_value(arguments).map_err(|e| format!("invalid params: {e}"))
}
fn response(
    output: BookmarkReadOutput,
    text: String,
    is_error: bool,
) -> CallToolResult<BookmarkReadOutput> {
    CallToolResult {
        content: vec![TextContent { kind: "text", text }],
        structured_content: output,
        is_error,
    }
}
pub(crate) fn failure(kind: BookmarkRead, message: String) -> CallToolResult<BookmarkReadOutput> {
    let output = if kind.detail() {
        BookmarkReadOutput::Detail(BookmarkDetailOutput {
            bookmarked: false,
            restrict: String::new(),
            tags: Some(vec![]),
        })
    } else {
        BookmarkReadOutput::Tags(BookmarkTagsOutput {
            bookmark_tags: vec![],
            pagination: json!({"page":1,"limit":null,"returned":0,"has_more":false,"next_page":null}),
        })
    };
    response(output, format!("Error: {message}"), true)
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
fn plan(input: &BookmarkReadInput) -> Result<crate::search::Plan, String> {
    crate::search::validate(&mut SearchIllustInput {
        page: input.page,
        limit: input.limit,
        ..Default::default()
    })
}
async fn detail<T: Transport>(
    client: &Client<T>,
    kind: BookmarkRead,
    input: &BookmarkReadInput,
) -> Result<BookmarkDetailOutput, SchedulerError> {
    let id = if kind == BookmarkRead::ArtworkDetail {
        input.illust_id
    } else {
        input.novel_id
    };
    let value = if kind == BookmarkRead::ArtworkDetail {
        client
            .artwork_bookmark(ArtworkBookmarkRequest { artwork_id: id })
            .await
    } else {
        client
            .novel_bookmark(NovelBookmarkRequest { novel_id: id })
            .await
    }
    .map_err(SchedulerError::from)?;
    Ok(BookmarkDetailOutput {
        bookmarked: !value.restrict.is_empty(),
        restrict: value.restrict,
        tags: if kind == BookmarkRead::ArtworkDetail && value.tags.is_empty() {
            None
        } else {
            Some(value.tags)
        },
    })
}
fn detail_validation(kind: BookmarkRead, input: &BookmarkReadInput) -> Result<i64, String> {
    let (id, field) = if kind == BookmarkRead::ArtworkDetail {
        (input.illust_id, "illust_id")
    } else {
        (input.novel_id, "novel_id")
    };
    if id <= 0 {
        Err(format!("{field} must be a positive integer"))
    } else {
        Ok(id)
    }
}
fn detail_result(
    kind: BookmarkRead,
    id: i64,
    out: Result<BookmarkDetailOutput, SchedulerError>,
) -> CallToolResult<BookmarkReadOutput> {
    match out {
        Ok(out) => {
            let text = format!(
                "{} {id} bookmarked: {}.",
                if kind == BookmarkRead::ArtworkDetail {
                    "Artwork"
                } else {
                    "Novel"
                },
                out.bookmarked
            );
            response(BookmarkReadOutput::Detail(out), text, false)
        }
        Err(e) => failure(kind, e.to_string()),
    }
}
pub async fn bookmark_read<T: Transport>(
    client: &Client<T>,
    kind: BookmarkRead,
    input: BookmarkReadInput,
) -> CallToolResult<BookmarkReadOutput> {
    if kind.detail() {
        let id = match detail_validation(kind, &input) {
            Ok(id) => id,
            Err(e) => return failure(kind, e),
        };
        return detail_result(kind, id, detail(client, kind, &input).await);
    }
    let prior = if kind == BookmarkRead::AllTags {
        match plan(&input) {
            Ok(p) => Some(p),
            Err(e) => return failure(kind, e),
        }
    } else {
        None
    };
    let id = match resolve(client, input.user_id) {
        Ok(id) => id,
        Err(e) => return failure(kind, e.to_string()),
    };
    let plan = match prior.map(Ok).unwrap_or_else(|| plan(&input)) {
        Ok(p) => p,
        Err(e) => return failure(kind, e),
    };
    tag_result(
        kind,
        collect(client, kind, id, &input, &plan).await,
        input.limit,
        &plan,
    )
}
pub(crate) async fn saved_read<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    kind: BookmarkRead,
    input: BookmarkReadInput,
    proxy: Option<&str>,
) -> CallToolResult<BookmarkReadOutput> {
    if kind.detail() {
        let id = match detail_validation(kind, &input) {
            Ok(id) => id,
            Err(e) => return failure(kind, e),
        };
        return detail_result(
            kind,
            id,
            execution
                .read(context, 0, proxy, move |_, client| {
                    let input = input.clone();
                    async move { detail(&client, kind, &input).await }
                })
                .await,
        );
    }
    let prior = if kind == BookmarkRead::AllTags {
        match plan(&input) {
            Ok(p) => Some(p),
            Err(e) => return failure(kind, e),
        }
    } else {
        None
    };
    let id = if input.user_id == 0 {
        execution
            .read(
                context,
                0,
                proxy,
                |_, client| async move { resolve(&client, 0) },
            )
            .await
    } else if input.user_id > 0 {
        Ok(input.user_id)
    } else {
        Err(SchedulerError::Message(
            "user_id must be positive when provided".into(),
        ))
    };
    let id = match id {
        Ok(id) => id,
        Err(e) => return failure(kind, e.to_string()),
    };
    let plan = match prior.map(Ok).unwrap_or_else(|| plan(&input)) {
        Ok(p) => p,
        Err(e) => return failure(kind, e),
    };
    let limit = input.limit;
    let out = execution
        .read(context, 0, proxy, move |_, client| {
            let input = input.clone();
            async move { collect(&client, kind, id, &input, &plan).await }
        })
        .await;
    tag_result(kind, out, limit, &plan)
}
fn tag_result(
    kind: BookmarkRead,
    out: Result<(Vec<BookmarkTagOutput>, bool), SchedulerError>,
    limit: Option<i64>,
    plan: &crate::search::Plan,
) -> CallToolResult<BookmarkReadOutput> {
    match out {
        Ok((tags, more)) => {
            let mut pagination = crate::search::pagination(plan, limit, tags.len(), more);
            crate::structured_wire_numbers(&mut pagination);
            let text = format!("Retrieved {} bookmark tags.", tags.len());
            response(
                BookmarkReadOutput::Tags(BookmarkTagsOutput {
                    bookmark_tags: tags,
                    pagination,
                }),
                text,
                false,
            )
        }
        Err(e) => failure(kind, e.to_string()),
    }
}
async fn fetch<T: Transport>(
    client: &Client<T>,
    artwork: bool,
    id: i64,
    input: &BookmarkReadInput,
    cursor: Cursor,
) -> Result<(Vec<BookmarkTag>, Cursor), SchedulerError> {
    let request = UserArtworkBookmarkTagsRequest {
        user_id: id,
        restrict: input.restrict.clone(),
        cursor,
    };
    let page = if artwork {
        client.user_artwork_bookmark_tags(request).await
    } else {
        client.user_novel_bookmark_tags(request).await
    }
    .map_err(SchedulerError::from)?;
    Ok((page.items, page.next))
}
fn source(error: pagination::Failure<SchedulerError>) -> SchedulerError {
    match error.cause {
        pagination::Cause::Source(e) => e,
        pagination::Cause::Message(e) => SchedulerError::Message(e),
    }
}
async fn collect<T: Transport>(
    client: &Client<T>,
    kind: BookmarkRead,
    id: i64,
    input: &BookmarkReadInput,
    plan: &crate::search::Plan,
) -> Result<(Vec<BookmarkTagOutput>, bool), SchedulerError> {
    let page_plan = pagination::Plan {
        skip: plan.skip,
        limit: plan.limit.max(0),
        one_batch: plan.one_batch,
    };
    if kind == BookmarkRead::AllTags {
        let mut streams = [true, false].map(|artwork| pagination::Stream {
            fetch: move |cursor: TagCursor| async move {
                let (mut items, next) = fetch(client, artwork, id, input, cursor.upstream).await?;
                if cursor.consumed > items.len() {
                    return Err(SchedulerError::Message(
                        "bookmark tag stream checkpoint exceeds batch".into(),
                    ));
                }
                items.drain(..cursor.consumed);
                Ok((
                    items
                        .into_iter()
                        .map(|tag| BookmarkTagOutput {
                            name: tag.name,
                            count: tag.count,
                            content_type: Some(if artwork { "artwork" } else { "novel" }),
                        })
                        .collect(),
                    TagCursor::new(next, 0),
                ))
            },
            include: |_: &BookmarkTagOutput| Ok(true),
            checkpoint: |cursor: TagCursor, position: usize| {
                if position == 0 {
                    return Err(SchedulerError::Message(
                        "bookmark tag stream checkpoint position must be positive".into(),
                    ));
                }
                Ok(TagCursor::new(cursor.upstream, cursor.consumed + position))
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
    let mut seen = BTreeSet::new();
    let page = pagination::collect_pages(
        page_plan,
        Cursor::default(),
        |cursor| fetch(client, kind == BookmarkRead::ArtworkTags, id, input, cursor),
        |tag: &BookmarkTag| Ok(seen.insert((tag.name.clone(), tag.count))),
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(source)?;
    Ok((
        page.items
            .into_iter()
            .map(|tag| BookmarkTagOutput {
                name: tag.name,
                count: tag.count,
                content_type: None,
            })
            .collect(),
        page.result.has_more,
    ))
}
#[derive(Clone, Default)]
struct TagCursor {
    upstream: Cursor,
    consumed: usize,
    text: String,
}
impl TagCursor {
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
impl pagination::Cursor for TagCursor {
    fn is_zero(&self) -> bool {
        self.consumed == 0 && self.upstream.is_zero()
    }
    fn text(&self) -> &str {
        &self.text
    }
}

fn serialize_count<S: serde::Serializer>(value: &i64, serializer: S) -> Result<S::Ok, S::Error> {
    let mut value = json!(value);
    crate::structured_wire_numbers(&mut value);
    value.serialize(serializer)
}
