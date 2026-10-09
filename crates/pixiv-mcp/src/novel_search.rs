use crate::{CallToolResult, SearchIllustInput};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client, cursor::Cursor, models::Novel, pixiv::SearchNovelsRequest, transport::Transport,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct NovelFilter {
    pub id: Option<i64>,
    pub tags: Vec<String>,
    pub min_views: Option<i64>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SearchNovelInput {
    pub word: String,
    pub search_target: String,
    pub sort: String,
    pub duration: String,
    pub novel_filter: Option<NovelFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn search_novel_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/search-novel.json"))
        .expect("novel search schema is valid JSON")
}
pub(crate) fn decode(arguments: Option<&Value>) -> Result<SearchNovelInput, String> {
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
    crate::search::validate_schema_with_integer_binding(
        &mut arguments,
        &search_novel_tool()["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        ("searchNovelIn", "int"),
    )?;
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}
fn validate(input: &SearchNovelInput) -> Result<crate::search::Plan, String> {
    let mut common = SearchIllustInput {
        page: input.page,
        limit: input.limit,
        ..Default::default()
    };
    let plan = crate::search::validate(&mut common)?;
    if let Some(filter) = &input.novel_filter {
        if filter.id.is_some_and(|id| id <= 0) {
            return Err("novel_filter.id must be positive".into());
        }
        if filter.min_views.is_some_and(|value| value < 0) {
            return Err("novel_filter.min_views must be zero or positive".into());
        }
    }
    Ok(plan)
}
pub async fn search_novel<T: Transport>(
    client: &Client<T>,
    input: SearchNovelInput,
) -> CallToolResult {
    let plan = match validate(&input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    result(collect(client, &input, &plan).await, &input, &plan)
}
pub(crate) async fn saved_search_novel<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: SearchNovelInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let plan = match validate(&input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    let requested = input.clone();
    let collected = execution
        .read(context, 0, proxy, move |_, client| {
            let input = requested.clone();
            async move { collect(&client, &input, &plan).await }
        })
        .await;
    result(collected, &input, &plan)
}
fn result(
    result: Result<(Vec<Novel>, bool), SchedulerError>,
    input: &SearchNovelInput,
    plan: &crate::search::Plan,
) -> CallToolResult {
    match result {
        Ok((items, more)) => {
            let mut records = Vec::with_capacity(items.len());
            for item in &items {
                let mut record = match pixiv_record::from_novel(item) {
                    Ok(record) => record,
                    Err(error) => return crate::search::failure(error.to_string()),
                };
                crate::structured_wire_numbers(&mut record);
                records.push(record);
            }
            crate::search::list_result(records, more, None, input.limit, plan)
        }
        Err(error) => crate::search::failure(error.to_string()),
    }
}
async fn collect<T: Transport>(
    client: &Client<T>,
    input: &SearchNovelInput,
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
        |cursor| {
            let request = SearchNovelsRequest {
                word: input.word.clone(),
                target: input.search_target.clone(),
                sort: input.sort.clone(),
                duration: input.duration.clone(),
                cursor,
            };
            async move {
                let page = client
                    .search_novels(request)
                    .await
                    .map_err(SchedulerError::from)?;
                Ok((page.items, page.next))
            }
        },
        |novel: &Novel| {
            let matches = input.novel_filter.as_ref().is_none_or(|filter| {
                filter.id.is_none_or(|id| novel.id == id)
                    && filter
                        .min_views
                        .is_none_or(|minimum| novel.total_views >= minimum)
                    && filter
                        .tags
                        .iter()
                        .all(|tag| novel.tags.iter().any(|actual| actual.name == *tag))
            });
            Ok(matches && seen.insert(novel.id))
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
