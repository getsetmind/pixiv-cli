use crate::{CallToolResult, IllustFilter, NovelFilter, SearchIllustInput, UserFilter};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::{Artwork, Novel, UserPreview},
    pixiv::{RecommendedArtworksRequest, RecommendedNovelsRequest, RecommendedUsersRequest},
    transport::Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    future::Future,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RecommendedInput {
    pub kind: String,
    pub illust_filter: Option<IllustFilter>,
    pub novel_filter: Option<NovelFilter>,
    pub user_filter: Option<UserFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn recommended_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/recommended.json"))
        .expect("mixed recommendation schema is valid JSON")
}
pub(crate) fn decode(arguments: Option<&Value>) -> Result<RecommendedInput, String> {
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
        &recommended_tool()["inputSchema"],
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
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}
fn validate(input: &RecommendedInput) -> Result<crate::search::Plan, String> {
    let mut common = SearchIllustInput {
        page: input.page,
        limit: input.limit,
        ..Default::default()
    };
    let plan = crate::search::validate(&mut common)?;
    match input.kind.as_str() {
        "all" => {}
        "illust" | "manga" => {
            if input.novel_filter.is_some() {
                return Err("novel_filter conflicts with kind".into());
            }
            if input.user_filter.is_some() {
                return Err("user_filter conflicts with kind".into());
            }
            if input
                .illust_filter
                .as_ref()
                .is_some_and(|filter| !filter.r#type.is_empty() && filter.r#type != input.kind)
            {
                return Err("illust_filter.type conflicts with kind".into());
            }
        }
        "novel" => {
            if input.illust_filter.is_some() {
                return Err("illust_filter conflicts with kind".into());
            }
            if input.user_filter.is_some() {
                return Err("user_filter conflicts with kind".into());
            }
        }
        "user" => {
            if input.illust_filter.is_some() {
                return Err("illust_filter conflicts with kind".into());
            }
            if input.novel_filter.is_some() {
                return Err("novel_filter conflicts with kind".into());
            }
        }
        _ => return Err("kind must be one of: all, illust, manga, novel, user".into()),
    }
    common.illust_filter = input.illust_filter.clone();
    crate::search::validate(&mut common)?;
    crate::novel_search::validate_filter(input.novel_filter.as_ref())?;
    crate::user_search::validate_filter(input.user_filter.as_ref())?;
    Ok(plan)
}
#[derive(Default)]
struct Output {
    records: Vec<Value>,
    pagination: serde_json::Map<String, Value>,
}
type SharedOutput = Arc<Mutex<Output>>;
pub(crate) fn failure(message: String) -> CallToolResult {
    let mut result = crate::failure(message);
    result.structured_content.pagination = Some(json!({}));
    result
}
fn result(output: &SharedOutput, outcome: Result<(), SchedulerError>) -> CallToolResult {
    if let Err(error) = outcome {
        return failure(error.to_string());
    }
    let mut output = output.lock().unwrap_or_else(|error| error.into_inner());
    let records = std::mem::take(&mut output.records);
    CallToolResult {
        content: vec![crate::TextContent {
            kind: "text",
            text: format!("Retrieved {} records.", records.len()),
        }],
        structured_content: crate::Records {
            records,
            pagination: Some(Value::Object(std::mem::take(&mut output.pagination))),
            filter: None,
        },
        is_error: false,
    }
}
pub async fn recommended<T: Transport>(
    client: &Client<T>,
    input: RecommendedInput,
) -> CallToolResult {
    let plan = match validate(&input) {
        Ok(plan) => plan,
        Err(error) => return failure(error),
    };
    let output = Arc::new(Mutex::new(Output::default()));
    result(&output, collect(client, &input, &plan, &output).await)
}
pub(crate) async fn saved_recommended<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: RecommendedInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let plan = match validate(&input) {
        Ok(plan) => plan,
        Err(error) => return failure(error),
    };
    // Go retains completed sections across account replay, while overwriting their pagination.
    let output = Arc::new(Mutex::new(Output::default()));
    let shared = output.clone();
    let outcome = execution
        .read(context, 0, proxy, move |_, client| {
            let input = input.clone();
            let output = shared.clone();
            async move { collect(&client, &input, &plan, &output).await }
        })
        .await;
    result(&output, outcome)
}
fn append<I>(
    output: &SharedOutput,
    kind: &str,
    items: &[I],
    more: bool,
    input: &RecommendedInput,
    plan: &crate::search::Plan,
    map: impl Fn(&I) -> Result<Value, pixiv_record::RecordError>,
) -> Result<(), SchedulerError> {
    let mut records = items
        .iter()
        .map(map)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| SchedulerError::Message(error.to_string()))?;
    records.iter_mut().for_each(crate::structured_wire_numbers);
    let mut pagination = crate::search::pagination(plan, input.limit, records.len(), more);
    crate::structured_wire_numbers(&mut pagination);
    let mut output = output.lock().unwrap_or_else(|error| error.into_inner());
    output.records.extend(records);
    output.pagination.insert(kind.into(), pagination);
    Ok(())
}
async fn collect<T: Transport>(
    client: &Client<T>,
    input: &RecommendedInput,
    plan: &crate::search::Plan,
    output: &SharedOutput,
) -> Result<(), SchedulerError> {
    if matches!(input.kind.as_str(), "all" | "illust" | "manga") {
        let mut seen = BTreeSet::new();
        let (items, more) = feed(
            plan,
            |cursor| async move {
                let page = client
                    .recommended_artworks(RecommendedArtworksRequest { cursor })
                    .await
                    .map_err(SchedulerError::from)?;
                Ok((page.items, page.next))
            },
            |artwork: &Artwork| seen.insert(format!("{:?}:{}", artwork.kind, artwork.id)),
        )
        .await?;
        for kind in ["illust", "manga"] {
            if input.kind != "all" && input.kind != kind {
                continue;
            }
            let mut filter = input.illust_filter.clone().unwrap_or_default();
            if filter.r#type.is_empty() {
                filter.r#type = kind.into();
            }
            let local_type = pixiv_app::search_filter::normalize_filter("", &filter.r#type)
                .map_err(|error| SchedulerError::Message(error.to_string()))?;
            let filtered = items
                .iter()
                .filter(|artwork| crate::search::matches(artwork, &filter, &local_type))
                .cloned()
                .collect::<Vec<_>>();
            append(
                output,
                kind,
                &filtered,
                more,
                input,
                plan,
                pixiv_record::from_artwork,
            )?;
        }
    }
    if matches!(input.kind.as_str(), "all" | "novel") {
        let mut seen = BTreeSet::new();
        let (items, more) = feed(
            plan,
            |cursor| async move {
                let page = client
                    .recommended_novels(RecommendedNovelsRequest { cursor })
                    .await
                    .map_err(SchedulerError::from)?;
                Ok((page.items, page.next))
            },
            |novel: &Novel| {
                crate::novel_search::matches(novel, input.novel_filter.as_ref())
                    && seen.insert(novel.id)
            },
        )
        .await?;
        append(
            output,
            "novel",
            &items,
            more,
            input,
            plan,
            pixiv_record::from_novel,
        )?;
    }
    if matches!(input.kind.as_str(), "all" | "user") {
        let mut seen = BTreeSet::new();
        let (items, more) = feed(
            plan,
            |cursor| async move {
                let page = client
                    .recommended_users(RecommendedUsersRequest { cursor })
                    .await
                    .map_err(SchedulerError::from)?;
                Ok((page.items, page.next))
            },
            |preview: &UserPreview| {
                crate::user_search::matches(preview, input.user_filter.as_ref())
                    && seen.insert(preview.user.id)
            },
        )
        .await?;
        append(
            output,
            "user",
            &items,
            more,
            input,
            plan,
            pixiv_record::from_user_preview,
        )?;
    }
    Ok(())
}
async fn feed<I, F, U, M>(
    plan: &crate::search::Plan,
    fetch: F,
    mut include: M,
) -> Result<(Vec<I>, bool), SchedulerError>
where
    F: FnMut(Cursor) -> U,
    U: Future<Output = Result<(Vec<I>, Cursor), SchedulerError>>,
    M: FnMut(&I) -> bool,
{
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        fetch,
        |item| Ok(include(item)),
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => SchedulerError::Message(message),
    })?;
    Ok((page.items, page.result.has_more))
}
