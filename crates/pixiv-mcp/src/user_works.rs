use crate::{CallToolResult, IllustFilter, NovelFilter, SearchIllustInput};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::{Artwork, Novel},
    pixiv::{UserArtworksRequest, UserNovelsRequest},
    transport::Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UserArtworksInput {
    pub user_id: i64,
    pub r#type: String,
    pub illust_filter: Option<IllustFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UserNovelsInput {
    pub user_id: i64,
    pub novel_filter: Option<NovelFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn user_artworks_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/user-artworks.json"))
        .expect("user artworks schema is valid JSON")
}
pub fn user_novels_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/user-novels.json"))
        .expect("user novels schema is valid JSON")
}
fn decode<D: serde::de::DeserializeOwned>(
    arguments: Option<&Value>,
    artworks: bool,
) -> Result<D, String> {
    let mut arguments = arguments
        .filter(|value| !value.is_null())
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
    let tool = if artworks {
        user_artworks_tool()
    } else {
        user_novels_tool()
    };
    crate::search::validate_schema_with_bindings(
        &mut arguments,
        &tool["inputSchema"],
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
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}
pub(crate) fn decode_artworks(arguments: Option<&Value>) -> Result<UserArtworksInput, String> {
    decode(arguments, true)
}
pub(crate) fn decode_novels(arguments: Option<&Value>) -> Result<UserNovelsInput, String> {
    decode(arguments, false)
}
fn artwork_common(input: &UserArtworksInput) -> SearchIllustInput {
    SearchIllustInput {
        illust_filter: input.illust_filter.clone(),
        page: input.page,
        limit: input.limit,
        ..Default::default()
    }
}
fn novel_plan(input: &UserNovelsInput) -> Result<crate::search::Plan, String> {
    let mut common = SearchIllustInput {
        page: input.page,
        limit: input.limit,
        ..Default::default()
    };
    let plan = crate::search::validate(&mut common)?;
    crate::novel_search::validate_filter(input.novel_filter.as_ref())?;
    Ok(plan)
}
fn resolve<T: Transport>(client: &Client<T>, requested: i64) -> Result<i64, SchedulerError> {
    if requested > 0 {
        return Ok(requested);
    };
    if requested < 0 {
        return Err(SchedulerError::Message(
            "user_id must be positive when provided".into(),
        ));
    };
    if client.user_id() > 0 {
        Ok(client.user_id())
    } else {
        Err(SchedulerError::Message(
            "cannot determine current user id".into(),
        ))
    }
}
async fn saved_id<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    id: i64,
    proxy: Option<&str>,
) -> Result<i64, SchedulerError> {
    if id > 0 {
        return Ok(id);
    };
    if id < 0 {
        return Err(SchedulerError::Message(
            "user_id must be positive when provided".into(),
        ));
    };
    execution
        .read(
            context,
            0,
            proxy,
            |_, client| async move { resolve(&client, 0) },
        )
        .await
}
pub async fn user_artworks<T: Transport>(
    client: &Client<T>,
    input: UserArtworksInput,
) -> CallToolResult {
    let mut common = artwork_common(&input);
    let plan = match crate::search::validate(&mut common) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    let id = match resolve(client, input.user_id) {
        Ok(id) => id,
        Err(error) => return crate::search::failure(error.to_string()),
    };
    crate::search::search_result(
        collect_artworks(client, id, &input, &plan).await,
        &common,
        &plan,
    )
}
pub(crate) async fn saved_artworks<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: UserArtworksInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let mut common = artwork_common(&input);
    let plan = match crate::search::validate(&mut common) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    let id = match saved_id(execution, context, input.user_id, proxy).await {
        Ok(id) => id,
        Err(error) => return crate::search::failure(error.to_string()),
    };
    let output = execution
        .read(context, 0, proxy, move |_, client| {
            let input = input.clone();
            async move { collect_artworks(&client, id, &input, &plan).await }
        })
        .await;
    crate::search::search_result(output, &common, &plan)
}
pub async fn user_novels<T: Transport>(
    client: &Client<T>,
    input: UserNovelsInput,
) -> CallToolResult {
    let plan = match novel_plan(&input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    let id = match resolve(client, input.user_id) {
        Ok(id) => id,
        Err(error) => return crate::search::failure(error.to_string()),
    };
    novel_result(
        collect_novels(client, id, &input, &plan).await,
        input.limit,
        &plan,
    )
}
pub(crate) async fn saved_novels<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: UserNovelsInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let plan = match novel_plan(&input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    let id = match saved_id(execution, context, input.user_id, proxy).await {
        Ok(id) => id,
        Err(error) => return crate::search::failure(error.to_string()),
    };
    let limit = input.limit;
    let output = execution
        .read(context, 0, proxy, move |_, client| {
            let input = input.clone();
            async move { collect_novels(&client, id, &input, &plan).await }
        })
        .await;
    novel_result(output, limit, &plan)
}
fn novel_result(
    output: Result<(Vec<Novel>, bool), SchedulerError>,
    limit: Option<i64>,
    plan: &crate::search::Plan,
) -> CallToolResult {
    match output {
        Ok((items, more)) => {
            let mut records = vec![];
            for item in &items {
                let mut record = match pixiv_record::from_novel(item) {
                    Ok(record) => record,
                    Err(error) => return crate::search::failure(error.to_string()),
                };
                crate::structured_wire_numbers(&mut record);
                records.push(record)
            }
            crate::search::list_result(records, more, None, limit, plan)
        }
        Err(error) => crate::search::failure(error.to_string()),
    }
}
async fn collect_artworks<T: Transport>(
    client: &Client<T>,
    id: i64,
    input: &UserArtworksInput,
    plan: &crate::search::Plan,
) -> Result<(Vec<Artwork>, bool, Option<Value>), SchedulerError> {
    let local = pixiv_app::search_filter::normalize_filter(
        "",
        input
            .illust_filter
            .as_ref()
            .map(|filter| filter.r#type.as_str())
            .unwrap_or_default(),
    )
    .map_err(|error| SchedulerError::Message(error.to_string()))?;
    let mut seen = BTreeSet::new();
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| {
            let request = UserArtworksRequest {
                user_id: id,
                kind: input.r#type.clone(),
                cursor,
            };
            async move {
                let page = client
                    .user_artworks(request)
                    .await
                    .map_err(SchedulerError::from)?;
                Ok((page.items, page.next))
            }
        },
        |item: &Artwork| {
            Ok(input
                .illust_filter
                .as_ref()
                .is_none_or(|filter| crate::search::matches(item, filter, &local))
                && seen.insert(format!("{:?}:{}", item.kind, item.id)))
        },
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => SchedulerError::Message(message),
    })?;
    Ok((page.items, page.result.has_more, None))
}
async fn collect_novels<T: Transport>(
    client: &Client<T>,
    id: i64,
    input: &UserNovelsInput,
    plan: &crate::search::Plan,
) -> Result<(Vec<Novel>, bool), SchedulerError> {
    let mut seen = BTreeSet::new();
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| async move {
            let page = client
                .user_novels(UserNovelsRequest {
                    user_id: id,
                    cursor,
                })
                .await
                .map_err(SchedulerError::from)?;
            Ok((page.items, page.next))
        },
        |item: &Novel| {
            Ok(
                crate::novel_search::matches(item, input.novel_filter.as_ref())
                    && seen.insert(item.id),
            )
        },
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => SchedulerError::Message(message),
    })?;
    Ok((page.items, page.result.has_more))
}
