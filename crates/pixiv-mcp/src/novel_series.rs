use crate::{CallToolResult, SearchIllustInput, TextContent};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::{Novel, NovelSeries},
    pixiv::NovelSeriesRequest,
    transport::Transport,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Mutex};
#[derive(Clone, Debug, Default, Deserialize)]
pub struct NovelSeriesInput {
    pub series_id: i64,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn novel_series_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/novel-series.json"))
        .expect("novel series schema is valid JSON")
}
pub(crate) fn decode_input<T: DeserializeOwned>(
    arguments: Option<&Value>,
    schema: &Value,
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
    crate::search::validate_schema_with_bindings(
        &mut arguments,
        schema,
        "",
        "invalid params: validating \"arguments\": validating root",
        &|path| {
            if path.ends_with("/page") || path.ends_with("/limit") {
                ("In", "int")
            } else {
                ("In", "int64")
            }
        },
    )?;
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}
pub(crate) fn decode(arguments: Option<&Value>) -> Result<NovelSeriesInput, String> {
    decode_input(arguments, &novel_series_tool()["inputSchema"])
}
fn validate(input: &NovelSeriesInput) -> Result<crate::search::Plan, String> {
    if input.series_id <= 0 {
        return Err("series_id must be a positive integer".into());
    }
    crate::search::validate(&mut SearchIllustInput {
        page: input.page,
        limit: input.limit,
        ..Default::default()
    })
}
pub(crate) fn failure(message: String) -> CallToolResult<Value> {
    let mut out = crate::failure(message);
    let series =
        serde_json::to_value(pixiv_sdk::dto::NovelSeriesDto::from(&NovelSeries::default()))
            .expect("DTO serializes");
    CallToolResult {
        content: std::mem::take(&mut out.content),
        structured_content: json!({"series":series,"records":[],"pagination":{"page":0,"limit":null,"returned":0,"has_more":false,"next_page":null}}),
        is_error: true,
    }
}
fn result(
    value: Result<(NovelSeries, Vec<Novel>, bool), SchedulerError>,
    input: &NovelSeriesInput,
    plan: &crate::search::Plan,
) -> CallToolResult<Value> {
    match value {
        Err(error) => failure(error.to_string()),
        Ok((series, items, more)) => {
            let mut records = Vec::with_capacity(items.len());
            for item in &items {
                match pixiv_record::from_novel(item) {
                    Ok(record) => records.push(record),
                    Err(error) => return failure(error.to_string()),
                }
            }
            let common = crate::search::list_result(records, more, None, input.limit, plan);
            let mut value = json!({"series":pixiv_sdk::dto::NovelSeriesDto::from(&series),"records":common.structured_content.records,"pagination":common.structured_content.pagination});
            crate::structured_wire_numbers(&mut value);
            CallToolResult {
                content: vec![TextContent {
                    kind: "text",
                    text: format!("Retrieved {} records.", items.len()),
                }],
                structured_content: value,
                is_error: false,
            }
        }
    }
}
pub async fn novel_series<T: Transport>(
    client: &Client<T>,
    input: NovelSeriesInput,
) -> CallToolResult<Value> {
    let plan = match validate(&input) {
        Ok(plan) => plan,
        Err(error) => return failure(error),
    };
    result(collect(client, &input, &plan).await, &input, &plan)
}
pub(crate) async fn saved_novel_series<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: NovelSeriesInput,
    proxy: Option<&str>,
) -> CallToolResult<Value> {
    let plan = match validate(&input) {
        Ok(plan) => plan,
        Err(error) => return failure(error),
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
async fn collect<T: Transport>(
    client: &Client<T>,
    input: &NovelSeriesInput,
    plan: &crate::search::Plan,
) -> Result<(NovelSeries, Vec<Novel>, bool), SchedulerError> {
    let series = Mutex::new(NovelSeries::default());
    let mut seen = BTreeSet::new();
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| {
            let first = cursor.is_zero();
            let series = &series;
            async move {
                let result = client
                    .novel_series(NovelSeriesRequest {
                        series_id: input.series_id,
                        cursor,
                    })
                    .await
                    .map_err(SchedulerError::from)?;
                if first {
                    *series.lock().expect("series metadata lock is not poisoned") = result.series;
                }
                Ok((result.novels.items, result.novels.next))
            }
        },
        |novel: &Novel| Ok(seen.insert(novel.id)),
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => SchedulerError::Message(message),
    })?;
    Ok((
        series
            .into_inner()
            .expect("series metadata lock is not poisoned"),
        page.items,
        page.result.has_more,
    ))
}
