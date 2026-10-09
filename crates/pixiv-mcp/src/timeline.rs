use crate::{CallToolResult, IllustFilter, NovelFilter, SearchIllustInput};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::{Artwork, Novel},
    pixiv::{
        FollowingArtworksRequest, FollowingNovelsRequest, LatestArtworksRequest,
        LatestNovelsRequest,
    },
    transport::Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Timeline {
    IllustFollowing,
    NovelFollowing,
    IllustLatest,
    NovelLatest,
}
impl Timeline {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "timeline_illust_following" => Some(Self::IllustFollowing),
            "timeline_novel_following" => Some(Self::NovelFollowing),
            "timeline_illust_latest" => Some(Self::IllustLatest),
            "timeline_novel_latest" => Some(Self::NovelLatest),
            _ => None,
        }
    }
    pub fn operation(self) -> &'static str {
        match self {
            Self::IllustFollowing => "FollowingArtworks",
            Self::NovelFollowing => "FollowingNovels",
            Self::IllustLatest => "LatestArtworks",
            Self::NovelLatest => "LatestNovels",
        }
    }
    fn artworks(self) -> bool {
        matches!(self, Self::IllustFollowing | Self::IllustLatest)
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TimelineInput {
    pub restrict: String,
    pub content_type: String,
    pub illust_filter: Option<IllustFilter>,
    pub novel_filter: Option<NovelFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn timeline_tool(kind: Timeline) -> Value {
    serde_json::from_str(match kind {
        Timeline::IllustFollowing => include_str!("../schemas/timeline-illust-following.json"),
        Timeline::NovelFollowing => include_str!("../schemas/timeline-novel-following.json"),
        Timeline::IllustLatest => include_str!("../schemas/timeline-illust-latest.json"),
        Timeline::NovelLatest => include_str!("../schemas/timeline-novel-latest.json"),
    })
    .expect("timeline schema is valid JSON")
}
pub(crate) fn decode(kind: Timeline, arguments: Option<&Value>) -> Result<TimelineInput, String> {
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
    crate::search::validate_schema_with_bindings(
        &mut arguments,
        &timeline_tool(kind)["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        &|path| {
            (
                if kind == Timeline::IllustFollowing {
                    "followIn"
                } else {
                    "In"
                },
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
fn common(input: &TimelineInput) -> SearchIllustInput {
    SearchIllustInput {
        illust_filter: input.illust_filter.clone(),
        page: input.page,
        limit: input.limit,
        ..Default::default()
    }
}
fn validate(kind: Timeline, input: &mut TimelineInput) -> Result<crate::search::Plan, String> {
    if kind == Timeline::IllustLatest && !matches!(input.content_type.as_str(), "illust" | "manga")
    {
        return Err("content_type must be one of: illust, manga".into());
    }
    if input.restrict.is_empty() {
        input.restrict = "public".into();
    }
    let plan = crate::search::validate(&mut common(input))?;
    if !kind.artworks() {
        crate::novel_search::validate_filter(input.novel_filter.as_ref())?;
    }
    Ok(plan)
}
pub async fn timeline<T: Transport>(
    client: &Client<T>,
    kind: Timeline,
    mut input: TimelineInput,
) -> CallToolResult {
    let plan = match validate(kind, &mut input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    if kind.artworks() {
        crate::search::search_result(
            collect_artworks(client, kind, &input, &plan).await,
            &common(&input),
            &plan,
        )
    } else {
        novel_result(
            collect_novels(client, kind, &input, &plan).await,
            input.limit,
            &plan,
        )
    }
}
pub(crate) async fn saved_timeline<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    kind: Timeline,
    mut input: TimelineInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let plan = match validate(kind, &mut input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    let requested = input.clone();
    if kind.artworks() {
        let output = execution
            .read(context, 0, proxy, move |_, client| {
                let input = requested.clone();
                async move { collect_artworks(&client, kind, &input, &plan).await }
            })
            .await;
        crate::search::search_result(output, &common(&input), &plan)
    } else {
        let output = execution
            .read(context, 0, proxy, move |_, client| {
                let input = requested.clone();
                async move { collect_novels(&client, kind, &input, &plan).await }
            })
            .await;
        novel_result(output, input.limit, &plan)
    }
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
    kind: Timeline,
    input: &TimelineInput,
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
        |cursor| async move {
            let page = match kind {
                Timeline::IllustFollowing => {
                    client
                        .following_artworks(FollowingArtworksRequest {
                            restrict: input.restrict.clone(),
                            cursor,
                        })
                        .await
                }
                Timeline::IllustLatest => {
                    client
                        .latest_artworks(LatestArtworksRequest {
                            content_type: input.content_type.clone(),
                            cursor,
                        })
                        .await
                }
                _ => unreachable!("artwork timeline"),
            }
            .map_err(SchedulerError::from)?;
            Ok((page.items, page.next))
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
    kind: Timeline,
    input: &TimelineInput,
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
            let page = match kind {
                Timeline::NovelFollowing => {
                    client
                        .following_novels(FollowingNovelsRequest {
                            restrict: input.restrict.clone(),
                            cursor,
                        })
                        .await
                }
                Timeline::NovelLatest => client.latest_novels(LatestNovelsRequest { cursor }).await,
                _ => unreachable!("novel timeline"),
            }
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
