use crate::{CallToolResult, IllustFilter, SearchIllustInput};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client, cursor::Cursor, models::Artwork, pixiv::ArtworkRankingRequest, transport::Transport,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct IllustRankingInput {
    pub mode: String,
    pub date: String,
    pub illust_filter: Option<IllustFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn illust_ranking_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/illust-ranking.json"))
        .expect("ranking tool schema is valid JSON")
}
pub(crate) fn decode(arguments: Option<&Value>) -> Result<IllustRankingInput, String> {
    let mut arguments = arguments
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if !arguments.is_object() {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            crate::stdio::value_type(&arguments)
        ));
    }
    crate::search::validate_schema(
        &mut arguments,
        &illust_ranking_tool()["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
    )?;
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}
fn validate(
    input: &mut IllustRankingInput,
) -> Result<(SearchIllustInput, crate::search::Plan), String> {
    if input.mode.is_empty() {
        input.mode = "day".into();
    }
    let schema = illust_ranking_tool();
    if !schema["inputSchema"]["properties"]["mode"]["enum"]
        .as_array()
        .expect("ranking modes are an array")
        .contains(&serde_json::json!(input.mode))
    {
        return Err("mode must be a supported Pixiv ranking mode".into());
    }
    if !input.date.is_empty() && !crate::search::valid_date(&input.date) {
        return Err("date must be a valid YYYY-MM-DD calendar date".into());
    }
    let mut common = SearchIllustInput {
        page: input.page,
        limit: input.limit,
        illust_filter: input.illust_filter.clone(),
        ..Default::default()
    };
    let plan = crate::search::validate(&mut common)?;
    Ok((common, plan))
}
pub async fn illust_ranking<T: Transport>(
    client: &Client<T>,
    mut input: IllustRankingInput,
) -> CallToolResult {
    let (common, plan) = match validate(&mut input) {
        Ok(value) => value,
        Err(error) => return crate::search::failure(error),
    };
    crate::search::search_result(collect(client, &input, &plan).await, &common, &plan)
}
pub(crate) async fn saved_illust_ranking<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    mut input: IllustRankingInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let (common, plan) = match validate(&mut input) {
        Ok(value) => value,
        Err(error) => return crate::search::failure(error),
    };
    let result = execution
        .read(context, 0, proxy, move |_, client| {
            let input = input.clone();
            async move { collect(&client, &input, &plan).await }
        })
        .await;
    crate::search::search_result(result, &common, &plan)
}
async fn collect<T: Transport>(
    client: &Client<T>,
    input: &IllustRankingInput,
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
            let request = ArtworkRankingRequest {
                mode: input.mode.clone(),
                date: input.date.clone(),
                cursor,
            };
            async move {
                let page = client
                    .artwork_ranking(request)
                    .await
                    .map_err(SchedulerError::from)?;
                Ok((page.items, page.next))
            }
        },
        |artwork: &Artwork| {
            Ok(input
                .illust_filter
                .as_ref()
                .is_none_or(|filter| crate::search::matches(artwork, filter, &local))
                && seen.insert(artwork.id))
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
