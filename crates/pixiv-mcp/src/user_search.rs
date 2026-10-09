use crate::{CallToolResult, SearchIllustInput};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client, Error, Reason, cursor::Cursor, models::UserPreview, pixiv::SearchUsersRequest,
    transport::Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UserFilter {
    pub id: Option<i64>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SearchUserInput {
    pub word: String,
    pub user_filter: Option<UserFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn search_user_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/search-user.json"))
        .expect("user search schema is valid JSON")
}
pub(crate) fn decode(arguments: Option<&Value>) -> Result<SearchUserInput, String> {
    let mut arguments = arguments
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            if arguments.is_number() {
                "number"
            } else {
                crate::stdio::value_type(&arguments)
            }
        ));
    }
    crate::search::validate_schema_with_bindings(
        &mut arguments,
        &search_user_tool()["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        &|path| {
            (
                "searchUserIn",
                if path == "/properties/user_filter/properties/id" {
                    "int64"
                } else {
                    "int"
                },
            )
        },
    )?;
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}
fn validate(input: &SearchUserInput) -> Result<crate::search::Plan, String> {
    if input.word.trim().is_empty() {
        return Err(Error::new(Reason::InvalidArgument, "SearchUsers")
            .with_detail("search word is required")
            .to_string());
    }
    let mut common = SearchIllustInput {
        page: input.page,
        limit: input.limit,
        ..Default::default()
    };
    let plan = crate::search::validate(&mut common)?;
    if input
        .user_filter
        .as_ref()
        .is_some_and(|filter| filter.id.is_some_and(|id| id <= 0))
    {
        return Err("user_filter.id must be positive".into());
    }
    Ok(plan)
}
pub async fn search_user<T: Transport>(
    client: &Client<T>,
    input: SearchUserInput,
) -> CallToolResult {
    let plan = match validate(&input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    result(collect(client, &input, &plan).await, &input, &plan)
}
pub(crate) async fn saved_search_user<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: SearchUserInput,
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
    result: Result<(Vec<UserPreview>, bool), SchedulerError>,
    input: &SearchUserInput,
    plan: &crate::search::Plan,
) -> CallToolResult {
    match result {
        Ok((items, more)) => {
            let mut records = Vec::with_capacity(items.len());
            for item in &items {
                let mut record = match pixiv_record::from_user_preview(item) {
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
    input: &SearchUserInput,
    plan: &crate::search::Plan,
) -> Result<(Vec<UserPreview>, bool), SchedulerError> {
    let mut seen = BTreeSet::new();
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| {
            let request = SearchUsersRequest {
                word: input.word.clone(),
                cursor,
            };
            async move {
                let page = client
                    .search_users(request)
                    .await
                    .map_err(SchedulerError::from)?;
                Ok((page.items, page.next))
            }
        },
        |preview: &UserPreview| {
            let matches = input
                .user_filter
                .as_ref()
                .is_none_or(|filter| filter.id.is_none_or(|id| preview.user.id == id));
            Ok(matches && seen.insert(preview.user.id))
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
