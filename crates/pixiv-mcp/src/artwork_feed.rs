use crate::{CallToolResult, IllustFilter, SearchIllustInput};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::Artwork,
    pixiv::{RecommendedArtworksRequest, RelatedArtworksRequest},
    transport::Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct IllustRelatedInput {
    pub illust_id: i64,
    pub illust_filter: Option<IllustFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct IllustRecommendedInput {
    pub illust_filter: Option<IllustFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn illust_related_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/illust-related.json"))
        .expect("related schema is valid JSON")
}
pub fn illust_recommended_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/illust-recommended.json"))
        .expect("recommended schema is valid JSON")
}
fn decode<T: serde::de::DeserializeOwned>(
    arguments: Option<&Value>,
    related: bool,
) -> Result<T, String> {
    let mut arguments = arguments
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            crate::stdio::value_type(&arguments)
        ));
    }
    let tool = if related {
        illust_related_tool()
    } else {
        illust_recommended_tool()
    };
    crate::search::validate_schema_with_bindings(
        &mut arguments,
        &tool["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        &|path| {
            let structure = if related {
                "relatedIn"
            } else {
                "recommendedArtworkIn"
            };
            (
                structure,
                if path.ends_with("/illust_id") || path.ends_with("/id") {
                    "int64"
                } else {
                    "int"
                },
            )
        },
    )?;
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}
pub(crate) fn decode_related(arguments: Option<&Value>) -> Result<IllustRelatedInput, String> {
    decode(arguments, true)
}
pub(crate) fn decode_recommended(
    arguments: Option<&Value>,
) -> Result<IllustRecommendedInput, String> {
    decode(arguments, false)
}
fn common(
    filter: Option<IllustFilter>,
    page: Option<i64>,
    limit: Option<i64>,
) -> SearchIllustInput {
    SearchIllustInput {
        illust_filter: filter,
        page,
        limit,
        ..Default::default()
    }
}
fn validate(
    id: Option<i64>,
    common: &mut SearchIllustInput,
) -> Result<crate::search::Plan, String> {
    if id.is_some_and(|id| id <= 0) {
        return Err("illust_id must be a positive integer".into());
    }
    crate::search::validate(common)
}
pub async fn illust_related<T: Transport>(
    client: &Client<T>,
    input: IllustRelatedInput,
) -> CallToolResult {
    run(
        client,
        Some(input.illust_id),
        common(input.illust_filter, input.page, input.limit),
    )
    .await
}
pub async fn illust_recommended<T: Transport>(
    client: &Client<T>,
    input: IllustRecommendedInput,
) -> CallToolResult {
    run(
        client,
        None,
        common(input.illust_filter, input.page, input.limit),
    )
    .await
}
async fn run<T: Transport>(
    client: &Client<T>,
    id: Option<i64>,
    mut input: SearchIllustInput,
) -> CallToolResult {
    let plan = match validate(id, &mut input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    crate::search::search_result(collect(client, id, &input, &plan).await, &input, &plan)
}
pub(crate) async fn saved_related<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: IllustRelatedInput,
    proxy: Option<&str>,
) -> CallToolResult {
    saved(
        execution,
        context,
        Some(input.illust_id),
        common(input.illust_filter, input.page, input.limit),
        proxy,
    )
    .await
}
pub(crate) async fn saved_recommended<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: IllustRecommendedInput,
    proxy: Option<&str>,
) -> CallToolResult {
    saved(
        execution,
        context,
        None,
        common(input.illust_filter, input.page, input.limit),
        proxy,
    )
    .await
}
async fn saved<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    id: Option<i64>,
    mut input: SearchIllustInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let plan = match validate(id, &mut input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    let requested = input.clone();
    let result = execution
        .read(context, 0, proxy, move |_, client| {
            let input = requested.clone();
            async move { collect(&client, id, &input, &plan).await }
        })
        .await;
    crate::search::search_result(result, &input, &plan)
}
async fn collect<T: Transport>(
    client: &Client<T>,
    id: Option<i64>,
    input: &SearchIllustInput,
    plan: &crate::search::Plan,
) -> Result<(Vec<Artwork>, bool, Option<Value>), SchedulerError> {
    let local_type = pixiv_app::search_filter::normalize_filter(
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
        |cursor| async move {
            let page = if let Some(artwork_id) = id {
                client
                    .related_artworks(RelatedArtworksRequest { artwork_id, cursor })
                    .await
            } else {
                client
                    .recommended_artworks(RecommendedArtworksRequest { cursor })
                    .await
            }
            .map_err(SchedulerError::from)?;
            Ok((page.items, page.next))
        },
        |artwork: &Artwork| {
            Ok(input
                .illust_filter
                .as_ref()
                .is_none_or(|filter| crate::search::matches(artwork, filter, &local_type))
                && seen.insert(format!("{:?}:{}", artwork.kind, artwork.id)))
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
