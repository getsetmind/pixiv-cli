use crate::{CallToolResult, SearchIllustInput, TextContent};
use pixiv_app::{pagination, scheduler::SchedulerError};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::{Comment, CommentAccessControl},
    pixiv::{ArtworkCommentsRequest, NovelCommentsRequest},
    transport::Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Mutex};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommentRead {
    Artwork,
    Novel,
}
impl CommentRead {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "illust_comments" => Some(Self::Artwork),
            "novel_comments" => Some(Self::Novel),
            _ => None,
        }
    }
    pub(crate) fn operation(self) -> &'static str {
        match self {
            Self::Artwork => "ArtworkComments",
            Self::Novel => "NovelComments",
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct CommentReadInput {
    pub id: i64,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
#[derive(Debug, Serialize)]
pub struct CommentReadOutput {
    pub comments: Vec<Value>,
    pub pagination: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_control: Option<Value>,
}
pub fn comment_read_tool(kind: CommentRead) -> Value {
    serde_json::from_str(match kind {
        CommentRead::Artwork => include_str!("../schemas/illust-comments.json"),
        CommentRead::Novel => include_str!("../schemas/novel-comments.json"),
    })
    .expect("comments schema is valid JSON")
}
pub(crate) fn decode(
    kind: CommentRead,
    arguments: Option<&Value>,
) -> Result<CommentReadInput, String> {
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
        &comment_read_tool(kind)["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        &|path| {
            (
                "In",
                if path.ends_with("/id") {
                    "int64"
                } else {
                    "int"
                },
            )
        },
    )?;
    serde_json::from_value(arguments).map_err(|e| format!("invalid params: {e}"))
}
fn plan(input: &CommentReadInput) -> Result<crate::search::Plan, String> {
    if input.id <= 0 {
        return Err("id must be a positive integer".into());
    }
    crate::search::validate(&mut SearchIllustInput {
        page: input.page,
        limit: input.limit,
        ..Default::default()
    })
}
pub async fn comment_read<T: Transport>(
    client: &Client<T>,
    kind: CommentRead,
    input: CommentReadInput,
) -> CallToolResult<CommentReadOutput> {
    let plan = match plan(&input) {
        Ok(p) => p,
        Err(e) => return failure(e),
    };
    result(
        collect(client, kind, input.id, &plan).await,
        input.limit,
        &plan,
    )
}
pub(crate) async fn saved_read<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    kind: CommentRead,
    input: CommentReadInput,
    proxy: Option<&str>,
) -> CallToolResult<CommentReadOutput> {
    let plan = match plan(&input) {
        Ok(p) => p,
        Err(e) => return failure(e),
    };
    let output = execution
        .read(context, 0, proxy, move |_, client| async move {
            collect(&client, kind, input.id, &plan).await
        })
        .await;
    result(output, input.limit, &plan)
}
pub(crate) fn failure(message: String) -> CallToolResult<CommentReadOutput> {
    CallToolResult {
        content: vec![TextContent {
            kind: "text",
            text: format!("Error: {message}"),
        }],
        structured_content: CommentReadOutput {
            comments: vec![],
            pagination: json!({"page":1,"limit":null,"returned":0,"has_more":false,"next_page":null}),
            total: None,
            access_control: None,
        },
        is_error: true,
    }
}
type Collected = (
    Vec<Comment>,
    bool,
    Option<i64>,
    Option<CommentAccessControl>,
);
fn result(
    output: Result<Collected, SchedulerError>,
    limit: Option<i64>,
    plan: &crate::search::Plan,
) -> CallToolResult<CommentReadOutput> {
    match output {
        Err(e) => failure(e.to_string()),
        Ok((items, more, total, access)) => {
            let mut comments = items
                .iter()
                .map(|item| {
                    serde_json::to_value(pixiv_sdk::dto::to_comment_dto(item))
                        .expect("comment DTO is serializable")
                })
                .collect::<Vec<_>>();
            comments.iter_mut().for_each(crate::structured_wire_numbers);
            let mut pagination = crate::search::pagination(plan, limit, comments.len(), more);
            crate::structured_wire_numbers(&mut pagination);
            let mut total = total.map(|v| json!(v));
            if let Some(v) = total.as_mut() {
                crate::structured_wire_numbers(v);
            }
            let mut access_control = access.as_ref().map(|v| {
                serde_json::to_value(pixiv_sdk::dto::to_comment_access_control_dto(v))
                    .expect("access DTO is serializable")
            });
            if let Some(v) = access_control.as_mut() {
                crate::structured_wire_numbers(v);
            }
            CallToolResult {
                content: vec![TextContent {
                    kind: "text",
                    text: format!("Retrieved {} comments.", comments.len()),
                }],
                structured_content: CommentReadOutput {
                    comments,
                    pagination,
                    total,
                    access_control,
                },
                is_error: false,
            }
        }
    }
}
async fn collect<T: Transport>(
    client: &Client<T>,
    kind: CommentRead,
    id: i64,
    plan: &crate::search::Plan,
) -> Result<Collected, SchedulerError> {
    let metadata = Mutex::new((None, None));
    let mut seen = BTreeSet::new();
    let page = pagination::collect_pages(
        pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| {
            let metadata = &metadata;
            async move {
                let page = match kind {
                    CommentRead::Artwork => {
                        client
                            .artwork_comments(ArtworkCommentsRequest {
                                artwork_id: id,
                                cursor,
                            })
                            .await
                    }
                    CommentRead::Novel => {
                        client
                            .novel_comments(NovelCommentsRequest {
                                novel_id: id,
                                cursor,
                            })
                            .await
                    }
                }
                .map_err(SchedulerError::from)?;
                let mut metadata = metadata
                    .lock()
                    .expect("comment metadata lock is not poisoned");
                if metadata.0.is_none() {
                    metadata.0 = page.total;
                }
                if metadata.1.is_none() {
                    metadata.1 = page.access_control;
                }
                Ok((page.items, page.next))
            }
        },
        |item: &Comment| {
            // Go's generic key includes parent pointer identity, so separately decoded parent chains remain distinct.
            Ok(item.parent.is_some() || seen.insert(comment_key(item)))
        },
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(|error| match error.cause {
        pagination::Cause::Source(e) => e,
        pagination::Cause::Message(e) => SchedulerError::Message(e),
    })?;
    let (total, access) = metadata
        .into_inner()
        .expect("comment metadata lock is not poisoned");
    Ok((page.items, page.result.has_more, total, access))
}

fn go_time(value: &chrono::DateTime<chrono::Utc>) -> String {
    let mut time = value.format("%Y-%m-%d %H:%M:%S").to_string();
    let nanos = value.timestamp_subsec_nanos();
    if nanos != 0 {
        time.push('.');
        time.push_str(format!("{nanos:09}").trim_end_matches('0'));
    }
    time.push_str(" +0000 UTC");
    time
}
fn comment_key(item: &Comment) -> String {
    let user = &item.user;
    let image = &user.profile_image;
    let resource = &image.resource;
    let headers = format!(
        "map[{}]",
        resource
            .request_headers
            .iter()
            .map(|(key, value)| format!("{key}:{value}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let expires = resource
        .expires_at
        .as_ref()
        .map(go_time)
        .unwrap_or_else(|| "<nil>".into());
    let resource = format!(
        "{{{} {} {} {} {}}}",
        resource.reference.as_str(),
        resource.url,
        headers,
        expires,
        resource.requires_credentials
    );
    let image = format!(
        "{{{} {} {} {}}}",
        resource, image.variant, image.width, image.height
    );
    let user = format!(
        "{{{} {} {} {} {} {}}}",
        user.id, user.name, user.account, user.comment, user.is_followed, image
    );
    format!(
        "pixiv.Comment:{{{} {} {} {} <nil>}}",
        item.id,
        user,
        item.body,
        go_time(&item.created_at)
    )
}
