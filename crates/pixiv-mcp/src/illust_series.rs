use crate::{CallToolResult, SearchIllustInput};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client, cursor::Cursor, models::Artwork, pixiv::ArtworkSeriesRequest, transport::Transport,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct IllustSeriesInput {
    pub series_id: i64,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn illust_series_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/illust-series.json"))
        .expect("series tool schema is valid JSON")
}
pub(crate) fn decode(arguments: Option<&Value>) -> Result<IllustSeriesInput, String> {
    crate::novel_series::decode_input(arguments, &illust_series_tool()["inputSchema"])
}
fn validate(input: &IllustSeriesInput) -> Result<(SearchIllustInput, crate::search::Plan), String> {
    if input.series_id <= 0 {
        return Err("series_id must be a positive integer".into());
    }
    let mut common = SearchIllustInput {
        page: input.page,
        limit: input.limit,
        ..Default::default()
    };
    let plan = crate::search::validate(&mut common)?;
    Ok((common, plan))
}
pub async fn illust_series<T: Transport>(
    client: &Client<T>,
    input: IllustSeriesInput,
) -> CallToolResult {
    let (common, plan) = match validate(&input) {
        Ok(value) => value,
        Err(error) => return crate::search::failure(error),
    };
    crate::search::search_result(collect(client, &input, &plan).await, &common, &plan)
}
pub(crate) async fn saved_illust_series<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: IllustSeriesInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let (common, plan) = match validate(&input) {
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
    input: &IllustSeriesInput,
    plan: &crate::search::Plan,
) -> Result<(Vec<Artwork>, bool, Option<Value>), SchedulerError> {
    let mut seen = BTreeSet::new();
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| {
            let request = ArtworkSeriesRequest {
                series_id: input.series_id,
                cursor,
            };
            async move {
                let page = client
                    .artwork_series(request)
                    .await
                    .map_err(SchedulerError::from)?;
                Ok((page.items, page.next))
            }
        },
        |artwork: &Artwork| Ok(seen.insert(format!("{:?}:{}", artwork.kind, artwork.id))),
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => SchedulerError::Message(message),
    })?;
    Ok((page.items, page.result.has_more, None))
}
