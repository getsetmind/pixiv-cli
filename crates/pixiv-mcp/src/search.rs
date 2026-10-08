use crate::{CallToolResult, Records, TextContent};
use chrono::{NaiveDate, Utc};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client, Error, Reason,
    models::{Artwork, ArtworkKind},
    pixiv::SearchArtworksRequest,
    transport::Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct IllustFilter {
    pub id: Option<i64>,
    pub r#type: String,
    pub tags: Vec<String>,
    pub min_views: Option<i64>,
    pub min_pages: Option<i64>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SearchIllustInput {
    pub word: String,
    pub search_target: String,
    pub sort: String,
    pub duration: String,
    pub start_date: String,
    pub end_date: String,
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub content_type: String,
    pub ai_mode: String,
    pub aspect_ratio: String,
    pub resolution: String,
    pub tool: String,
    pub bookmark_min: Option<i64>,
    pub bookmark_max: Option<i64>,
    pub bookmark_strategy: String,
    pub illust_filter: Option<IllustFilter>,
}

pub fn search_illust_tool() -> Value {
    let string = |description| json!({"type":"string","description":description});
    let enumeration = |description, values: &[&str]| json!({"type":"string","description":description,"enum":values});
    let mut output = crate::illust_detail_tool()["outputSchema"].clone();
    let properties = output["properties"]
        .as_object_mut()
        .expect("record schema is an object");
    for key in ["access_control", "content", "pagination", "series"] {
        properties.insert(
            key.into(),
            json!({"type":"object","additionalProperties":true}),
        );
    }
    for key in ["bookmark_tags", "comments"] {
        properties.insert(
            key.into(),
            json!({"type":"array","items":{"type":"object","additionalProperties":true}}),
        );
    }
    properties.insert("bookmarked".into(), json!({"type":"boolean"}));
    properties.insert("restrict".into(), json!({"type":"string"}));
    properties.insert(
        "tags".into(),
        json!({"type":"array","items":{"type":"string"}}),
    );
    properties.insert("total".into(), json!({"type":"integer"}));
    properties.insert("filter".into(),json!({"type":"object","additionalProperties":false,"properties":{"min":{"type":"integer"},"max":{"type":"integer"},"membership":{"type":"string"},"strategy":{"type":"string"},"completeness":{"type":"string"}}}));
    json!({"name":"search_illust","description":"Search for illustrations using keywords with filters.","outputSchema":output,
        "inputSchema":{"type":"object","additionalProperties":false,"required":["word"],"properties":{
            "word":string("Illustration search keyword."),
            "search_target":enumeration("Pixiv search target.", &["partial_match_for_tags","exact_match_for_tags","title_and_caption","keyword"]),
            "sort":string("Pixiv result order."),
            "duration":enumeration("Pixiv quick date range; cannot be combined with start_date or end_date.", &["within_last_day","within_last_week","within_last_month","within_half_year","within_year"]),
            "start_date":{"type":"string","pattern":"^[0-9]{4}-[0-9]{2}-[0-9]{2}$","description":"Inclusive start date in YYYY-MM-DD; may be used with end_date."},
            "end_date":{"type":"string","pattern":"^[0-9]{4}-[0-9]{2}-[0-9]{2}$","description":"Inclusive end date in YYYY-MM-DD; may be used with start_date."},
            "page":{"type":"integer","description":"1-based logical page; requires a positive limit."},
            "limit":{"type":"integer","description":"Maximum logical results; 0 returns all; omit for one logical batch."},
            "content_type":enumeration("Artwork content type filter.",&["all","illust-and-ugoira","illust","manga","ugoira"]),
            "ai_mode":enumeration("AI-generated artwork filter.",&["all","exclude","only"]),
            "aspect_ratio":enumeration("Artwork aspect ratio filter.",&["all","landscape","portrait","square"]),
            "resolution":enumeration("Artwork resolution tier filter.",&["all","high","medium","low"]),
            "tool":string("Exact drawing tool name from the versioned pixiv-cli drawing-tool catalog."),
            "bookmark_min":{"type":"integer","minimum":0,"description":"Inclusive minimum public bookmark count; requires App OAuth."},
            "bookmark_max":{"type":"integer","minimum":0,"description":"Inclusive maximum public bookmark count; requires App OAuth."},
            "bookmark_strategy":enumeration("Bookmark count strategy; server requires verified evidence and otherwise fails explicitly.",&["auto","local","best_effort","server"]),
            "illust_filter":{"type":"object","additionalProperties":false,"properties":{"id":{"type":"integer","minimum":1},"type":{"type":"string","enum":["illust","manga","ugoira"]},"tags":{"type":"array","items":{"type":"string"}},"min_views":{"type":"integer","minimum":0},"min_pages":{"type":"integer","minimum":0}}}
        }}
    })
}

pub(crate) fn decode(arguments: Option<&Value>) -> Result<SearchIllustInput, String> {
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
    validate_schema(
        &mut arguments,
        &search_illust_tool()["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
    )?;
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}
fn display(value: &Value) -> String {
    match value {
        Value::Null => "<invalid reflect.Value>".into(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}
fn date_shape(value: &str) -> bool {
    value.len() == 10
        && value.bytes().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
}
fn validate_schema(
    value: &mut Value,
    schema: &Value,
    path: &str,
    context: &str,
) -> Result<(), String> {
    let expected = schema["type"].as_str().unwrap_or_default();
    let actual = crate::stdio::value_type(value);
    if actual != expected {
        return Err(format!(
            "{context}: type: {} has type {actual:?}, want {expected:?}",
            display(value)
        ));
    }
    if let Some(allowed) = schema["enum"].as_array()
        && !allowed.contains(value)
    {
        return Err(format!(
            "{context}: enum: {} does not equal any of: [{}]",
            display(value),
            allowed
                .iter()
                .map(|v| v.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    if let Some(minimum) = schema["minimum"].as_i64()
        && value.as_f64().is_some_and(|number| number < minimum as f64)
    {
        return Err(format!(
            "{context}: minimum: {}/1 is less than {:.6}",
            display(value),
            minimum as f64
        ));
    }
    if let Some(pattern) = schema["pattern"].as_str()
        && !date_shape(value.as_str().unwrap_or_default())
    {
        return Err(format!(
            "{context}: pattern: {:?} does not match regular expression {pattern:?}",
            value.as_str().unwrap_or_default()
        ));
    }
    if let Some(map) = value.as_object_mut() {
        let missing = schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|key| !map.contains_key(*key))
            .map(|key| format!("{key:?}"))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!(
                "{context}: required: missing properties: [{}]",
                missing.join(" ")
            ));
        }
        let properties = schema["properties"]
            .as_object()
            .expect("object schema has properties");
        let extra = map
            .keys()
            .filter(|key| !properties.contains_key(*key))
            .map(|key| format!("{key:?}"))
            .collect::<Vec<_>>();
        if !extra.is_empty() {
            return Err(format!(
                "{context}: unexpected additional properties [{}]",
                extra.join(" ")
            ));
        }
        for (key, value) in map {
            let nested = format!("{path}/properties/{key}");
            validate_schema(
                value,
                &properties[key],
                &nested,
                &format!("{context}: validating {nested}"),
            )?;
        }
    } else if let Some(values) = value.as_array_mut() {
        for value in values {
            let nested = format!("{path}/items");
            validate_schema(
                value,
                &schema["items"],
                &nested,
                &format!("{context}: validating {nested}"),
            )?;
        }
    } else if expected == "integer" {
        let decimal = value.as_f64().expect("integer fits float64").to_string();
        let integer: i64 = decimal.parse().map_err(|_|format!("invalid params: json: cannot unmarshal number {decimal} into Go struct field searchIllustIn.{} of type int",path.strip_prefix("/properties/").unwrap_or(path)))?;
        *value = json!(integer);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Plan {
    page: i64,
    limit: i64,
    skip: i64,
    one_batch: bool,
}
fn plan(input: &SearchIllustInput) -> Result<Plan, String> {
    if input.page.is_some_and(|page| page <= 0) {
        return Err("page must be a positive integer".into());
    }
    if input.limit.is_some_and(|limit| limit < 0) {
        return Err("limit must be zero or a positive integer".into());
    }
    if let Some(page) = input.page {
        let limit = input
            .limit
            .filter(|limit| *limit > 0)
            .ok_or("page requires limit to be a positive integer")?;
        let skip = (page - 1)
            .checked_mul(limit)
            .ok_or("page and limit overflow the logical result offset")?;
        return Ok(Plan {
            page,
            limit,
            skip,
            one_batch: false,
        });
    }
    Ok(Plan {
        page: 1,
        limit: input.limit.unwrap_or(-1),
        skip: 0,
        one_batch: input.limit.is_none(),
    })
}
fn pagination(plan: &Plan, limit: Option<i64>, returned: usize, more: bool) -> Value {
    json!({"page":plan.page,"limit":limit,"returned":returned,"has_more":more,"next_page":if more && limit.is_some_and(|limit|limit > 0) { plan.page.checked_add(1) } else { None }})
}
pub(crate) fn failure(message: String) -> CallToolResult {
    let mut result = crate::failure(message);
    result.structured_content.pagination =
        Some(json!({"page":1,"limit":null,"returned":0,"has_more":false,"next_page":null}));
    result
}
fn valid_date(value: &str) -> bool {
    date_shape(value) && NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}
fn validate(input: &mut SearchIllustInput) -> Result<Plan, String> {
    if input.start_date.is_empty()
        && input.end_date.is_empty()
        && let Some(range) =
            pixiv_app::dates::quick_date_range(&input.duration, Utc::now().fixed_offset())
                .map_err(|error| error.to_string())?
    {
        input.start_date = range.start_date;
        input.end_date = range.end_date;
        input.duration.clear();
    }
    if !input.duration.is_empty() && (!input.start_date.is_empty() || !input.end_date.is_empty()) {
        return Err("duration cannot be combined with start_date or end_date".into());
    }
    if (!input.start_date.is_empty() && !valid_date(&input.start_date))
        || (!input.end_date.is_empty() && !valid_date(&input.end_date))
    {
        return Err("start_date and end_date must use YYYY-MM-DD".into());
    }
    if !input.start_date.is_empty()
        && !input.end_date.is_empty()
        && input.start_date > input.end_date
    {
        return Err("start_date cannot be later than end_date".into());
    }
    if input
        .bookmark_min
        .zip(input.bookmark_max)
        .is_some_and(|(min, max)| min > max)
    {
        return Err("bookmark_min cannot be greater than bookmark_max".into());
    }
    let plan = plan(input)?;
    if let Some(filter) = &input.illust_filter {
        if filter.id.is_some_and(|id| id <= 0) {
            return Err("illust_filter.id must be positive".into());
        }
        if !matches!(filter.r#type.as_str(), "" | "illust" | "manga" | "ugoira") {
            return Err("illust_filter.type must be one of: illust, manga, ugoira".into());
        }
        if filter.min_views.is_some_and(|value| value < 0) {
            return Err("illust_filter.min_views must be zero or positive".into());
        }
        if filter.min_pages.is_some_and(|value| value < 0) {
            return Err("illust_filter.min_pages must be zero or positive".into());
        }
        if input.bookmark_min.is_some() || input.bookmark_max.is_some() {
            return Err("bookmark range cannot be combined with illust_filter".into());
        }
    }
    Ok(plan)
}
fn kind(artwork: &Artwork) -> &'static str {
    match artwork.kind {
        ArtworkKind::Illust => "illust",
        ArtworkKind::Manga => "manga",
        ArtworkKind::Ugoira => "ugoira",
        ArtworkKind::Unknown => "unknown",
    }
}
fn matches(
    artwork: &Artwork,
    filter: &IllustFilter,
    local_type: &pixiv_app::search_filter::ArtworkFilter,
) -> bool {
    filter.id.is_none_or(|id| artwork.id == id)
        && local_type.matches(artwork.x_restrict, kind(artwork))
        && filter
            .min_views
            .is_none_or(|minimum| artwork.total_views >= minimum)
        && filter
            .min_pages
            .is_none_or(|minimum| artwork.page_count >= minimum)
        && filter
            .tags
            .iter()
            .all(|tag| artwork.tags.iter().any(|actual| actual.name == *tag))
}
pub async fn search_illust<T: Transport>(
    client: &Client<T>,
    mut input: SearchIllustInput,
) -> CallToolResult {
    let plan = match validate(&mut input) {
        Ok(plan) => plan,
        Err(error) => return failure(error),
    };
    search_result(collect(client, &input, &plan).await, &input, &plan)
}

pub(crate) async fn saved_search_illust<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    mut input: SearchIllustInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let plan = match validate(&mut input) {
        Ok(plan) => plan,
        Err(error) => return failure(error),
    };
    let requested = input.clone();
    let result = execution
        .read(context, 0, proxy, move |_, client| {
            let input = requested.clone();
            async move { collect(&client, &input, &plan).await }
        })
        .await;
    search_result(result, &input, &plan)
}

fn search_result(
    result: Result<(Vec<Artwork>, bool, Option<Value>), SchedulerError>,
    input: &SearchIllustInput,
    plan: &Plan,
) -> CallToolResult {
    match result {
        Ok((items, more, filter)) => {
            let mut records = vec![];
            for item in &items {
                match pixiv_record::from_artwork(item) {
                    Ok(mut record) => {
                        crate::structured_wire_numbers(&mut record);
                        records.push(record);
                    }
                    Err(error) => return failure(error.to_string()),
                }
            }
            let mut pagination = pagination(plan, input.limit, records.len(), more);
            crate::structured_wire_numbers(&mut pagination);
            CallToolResult {
                content: vec![TextContent {
                    kind: "text",
                    text: format!("Retrieved {} records.", records.len()),
                }],
                structured_content: Records {
                    records,
                    pagination: Some(pagination),
                    filter,
                },
                is_error: false,
            }
        }
        Err(error) => failure(error.to_string()),
    }
}
async fn collect<T: Transport>(
    client: &Client<T>,
    input: &SearchIllustInput,
    plan: &Plan,
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
    let has_range = input.bookmark_min.is_some() || input.bookmark_max.is_some();
    let bookmark_branch = has_range || !input.bookmark_strategy.is_empty();
    for (value, detail) in [
        (
            input.bookmark_min,
            "bookmark minimum must be zero or positive",
        ),
        (
            input.bookmark_max,
            "bookmark maximum must be zero or positive",
        ),
    ] {
        if value.is_some_and(|value| value < 0) {
            return Err(Error::new(Reason::InvalidArgument, "SearchArtworks")
                .with_detail(detail)
                .into());
        }
    }
    let strategy =
        match input.bookmark_strategy.as_str() {
            "" | "auto" if has_range => "local",
            "" | "auto" => "auto",
            "local" => "local",
            "best_effort" => "best_effort",
            "server" => return Err(Error::new(Reason::UpstreamUnavailable, "SearchArtworks")
                .with_detail(
                    "server bookmark strategy requires verified premium membership and evidence",
                )
                .into()),
            _ => {
                return Err(Error::new(Reason::InvalidArgument, "SearchArtworks")
                    .with_detail("unknown bookmark filter strategy")
                    .into());
            }
        };
    let mut request = SearchArtworksRequest {
        word: input.word.clone(),
        target: input.search_target.clone(),
        sort: input.sort.clone(),
        duration: input.duration.clone(),
        start_date: input.start_date.clone(),
        end_date: input.end_date.clone(),
        content_type: input.content_type.clone(),
        ai_mode: input.ai_mode.clone(),
        aspect_ratio: input.aspect_ratio.clone(),
        resolution: input.resolution.clone(),
        tool: input.tool.clone(),
        bookmark_min: input.bookmark_min,
        bookmark_max: input.bookmark_max,
        ..Default::default()
    };
    if has_range {
        request.cursor_context = pixiv_app::search_filter::bookmark_context(
            input.bookmark_min,
            input.bookmark_max,
            strategy,
        );
        if strategy == "local" {
            request.bookmark_min = None;
            request.bookmark_max = None;
        }
    }
    let mut seen = BTreeSet::new();
    let checkpoint = bookmark_branch.then_some(|cursor, consumed| {
        let mut query = request.clone();
        query.cursor = cursor;
        client
            .checkpoint_search_artworks(query, consumed as i64)
            .map_err(SchedulerError::from)
    });
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        request.cursor.clone(),
        |cursor| {
            let mut query = request.clone();
            query.cursor = cursor;
            async move {
                let page = client
                    .search_artworks(query)
                    .await
                    .map_err(SchedulerError::from)?;
                Ok((page.items, page.next))
            }
        },
        |artwork: &Artwork| {
            if has_range {
                if artwork.total_bookmarks < 0 {
                    return Err(
                        Error::new(Reason::MalformedUpstreamResponse, "SearchArtworks")
                            .with_detail("artwork bookmark count is negative")
                            .into(),
                    );
                }
                Ok(!input
                    .bookmark_min
                    .is_some_and(|min| artwork.total_bookmarks < min)
                    && !input
                        .bookmark_max
                        .is_some_and(|max| artwork.total_bookmarks > max))
            } else if !bookmark_branch {
                Ok(!input
                    .illust_filter
                    .as_ref()
                    .is_some_and(|filter| !matches(artwork, filter, &local_type))
                    && seen.insert(format!("{}:{}", kind(artwork), artwork.id)))
            } else {
                Ok(true)
            }
        },
        checkpoint,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => SchedulerError::Message(message),
    })?;
    let items = page.items;
    let more = page.result.has_more;
    let filter = if has_range {
        let mut filter = json!({"membership":"unknown","strategy":strategy,"completeness":if more { "partial" } else { "complete_for_source" }});
        if let Some(min) = input.bookmark_min {
            filter["min"] = json!(min);
        }
        if let Some(max) = input.bookmark_max {
            filter["max"] = json!(max);
        }
        crate::structured_wire_numbers(&mut filter);
        Some(filter)
    } else {
        None
    };
    Ok((items, more, filter))
}
