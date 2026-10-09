use crate::{CallToolResult, SearchIllustInput, UserFilter};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::UserPreview,
    pixiv::{
        RelatedUsersRequest, UserBlockedUsersRequest, UserFollowersRequest, UserFollowingRequest,
    },
    transport::Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserRelationship {
    Following,
    Followers,
    Related,
    Blocked,
}
impl UserRelationship {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "user_following" => Some(Self::Following),
            "user_followers" => Some(Self::Followers),
            "related_users" => Some(Self::Related),
            "blocked_users" => Some(Self::Blocked),
            _ => None,
        }
    }
    pub(crate) fn operation(self) -> &'static str {
        match self {
            Self::Following => "UserFollowing",
            Self::Followers => "UserFollowers",
            Self::Related => "RelatedUsers",
            Self::Blocked => "UserBlockedUsers",
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UserRelationshipInput {
    pub user_id: i64,
    pub restrict: String,
    pub user_filter: Option<UserFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn user_relationship_tool(kind: UserRelationship) -> Value {
    serde_json::from_str(match kind {
        UserRelationship::Following => include_str!("../schemas/user-following.json"),
        UserRelationship::Followers => include_str!("../schemas/user-followers.json"),
        UserRelationship::Related => include_str!("../schemas/related-users.json"),
        UserRelationship::Blocked => include_str!("../schemas/blocked-users.json"),
    })
    .expect("user relationship schema is valid JSON")
}
pub(crate) fn decode(
    kind: UserRelationship,
    arguments: Option<&Value>,
) -> Result<UserRelationshipInput, String> {
    let mut arguments = arguments
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            match &arguments {
                Value::Number(_) => "number",
                Value::Bool(_) => "bool",
                v => crate::stdio::value_type(v),
            }
        ));
    }
    crate::search::validate_schema_with_bindings(
        &mut arguments,
        &user_relationship_tool(kind)["inputSchema"],
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
    serde_json::from_value(arguments).map_err(|e| format!("invalid params: {e}"))
}
fn plan(input: &UserRelationshipInput) -> Result<crate::search::Plan, String> {
    crate::search::validate(&mut SearchIllustInput {
        page: input.page,
        limit: input.limit,
        ..Default::default()
    })
}
fn before_identity(
    kind: UserRelationship,
    input: &UserRelationshipInput,
) -> Result<Option<crate::search::Plan>, String> {
    match kind {
        UserRelationship::Following => {
            let plan = plan(input)?;
            crate::user_search::validate_filter(input.user_filter.as_ref())?;
            Ok(Some(plan))
        }
        UserRelationship::Related => {
            crate::user_search::validate_filter(input.user_filter.as_ref())?;
            Ok(None)
        }
        _ => Ok(None),
    }
}
fn resolve<T: Transport>(client: &Client<T>, requested: i64) -> Result<i64, SchedulerError> {
    if requested > 0 {
        Ok(requested)
    } else if requested < 0 {
        Err(SchedulerError::Message(
            "user_id must be positive when provided".into(),
        ))
    } else if client.user_id() > 0 {
        Ok(client.user_id())
    } else {
        Err(SchedulerError::Message(
            "cannot determine current user id".into(),
        ))
    }
}
pub async fn user_relationship<T: Transport>(
    client: &Client<T>,
    kind: UserRelationship,
    input: UserRelationshipInput,
) -> CallToolResult {
    let prior = match before_identity(kind, &input) {
        Ok(p) => p,
        Err(e) => return crate::search::failure(e),
    };
    let id = match resolve(client, input.user_id) {
        Ok(id) => id,
        Err(e) => return crate::search::failure(e.to_string()),
    };
    let plan = match prior.map(Ok).unwrap_or_else(|| plan(&input)) {
        Ok(p) => p,
        Err(e) => return crate::search::failure(e),
    };
    result(
        collect(client, kind, id, &input, &plan).await,
        input.limit,
        &plan,
    )
}
pub(crate) async fn saved_relationship<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    kind: UserRelationship,
    input: UserRelationshipInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let prior = match before_identity(kind, &input) {
        Ok(p) => p,
        Err(e) => return crate::search::failure(e),
    };
    let id = if input.user_id > 0 {
        Ok(input.user_id)
    } else if input.user_id < 0 {
        Err(SchedulerError::Message(
            "user_id must be positive when provided".into(),
        ))
    } else {
        execution
            .read(
                context,
                0,
                proxy,
                |_, client| async move { resolve(&client, 0) },
            )
            .await
    };
    let id = match id {
        Ok(id) => id,
        Err(e) => return crate::search::failure(e.to_string()),
    };
    let plan = match prior.map(Ok).unwrap_or_else(|| plan(&input)) {
        Ok(p) => p,
        Err(e) => return crate::search::failure(e),
    };
    let limit = input.limit;
    let collected = execution
        .read(context, 0, proxy, move |_, client| {
            let input = input.clone();
            async move { collect(&client, kind, id, &input, &plan).await }
        })
        .await;
    result(collected, limit, &plan)
}
pub(crate) fn result(
    output: Result<(Vec<UserPreview>, bool), SchedulerError>,
    limit: Option<i64>,
    plan: &crate::search::Plan,
) -> CallToolResult {
    match output {
        Ok((items, more)) => {
            let mut records = Vec::with_capacity(items.len());
            for item in &items {
                let mut record = match pixiv_record::from_user_preview(item) {
                    Ok(record) => record,
                    Err(e) => return crate::search::failure(e.to_string()),
                };
                crate::structured_wire_numbers(&mut record);
                records.push(record);
            }
            crate::search::list_result(records, more, None, limit, plan)
        }
        Err(e) => crate::search::failure(e.to_string()),
    }
}
async fn collect<T: Transport>(
    client: &Client<T>,
    kind: UserRelationship,
    id: i64,
    input: &UserRelationshipInput,
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
        |cursor| async move {
            let page = match kind {
                UserRelationship::Following => {
                    client
                        .user_following(UserFollowingRequest {
                            user_id: id,
                            restrict: input.restrict.clone(),
                            cursor,
                        })
                        .await
                }
                UserRelationship::Followers => {
                    client
                        .user_followers(UserFollowersRequest {
                            user_id: id,
                            restrict: input.restrict.clone(),
                            cursor,
                        })
                        .await
                }
                UserRelationship::Related => {
                    client
                        .related_users(RelatedUsersRequest {
                            user_id: id,
                            cursor,
                        })
                        .await
                }
                UserRelationship::Blocked => {
                    client
                        .user_blocked_users(UserBlockedUsersRequest {
                            user_id: id,
                            cursor,
                        })
                        .await
                }
            }
            .map_err(SchedulerError::from)?;
            Ok((page.items, page.next))
        },
        |preview: &UserPreview| {
            Ok(crate::user_search::matches(
                preview,
                if matches!(
                    kind,
                    UserRelationship::Following | UserRelationship::Related
                ) {
                    input.user_filter.as_ref()
                } else {
                    None
                },
            ) && seen.insert(preview.user.id))
        },
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(|e| match e.cause {
        pixiv_app::pagination::Cause::Source(e) => e,
        pixiv_app::pagination::Cause::Message(m) => SchedulerError::Message(m),
    })?;
    Ok((page.items, page.result.has_more))
}
