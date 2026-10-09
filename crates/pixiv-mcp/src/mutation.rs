use crate::{CallToolResult, TextContent};
use pixiv_sdk::{Client, pixiv::*, transport::Transport};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MutationInput {
    #[serde(default)]
    pub illust_id: i64,
    #[serde(default)]
    pub novel_id: i64,
    #[serde(default)]
    pub user_id: i64,
    #[serde(default)]
    pub restrict: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum MutationAction {
    AddBookmark,
    RemoveBookmark,
    AddNovelBookmark,
    RemoveNovelBookmark,
    FollowUser,
    UnfollowUser,
}
impl MutationAction {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "add_bookmark" => Some(Self::AddBookmark),
            "remove_bookmark" => Some(Self::RemoveBookmark),
            "add_novel_bookmark" => Some(Self::AddNovelBookmark),
            "remove_novel_bookmark" => Some(Self::RemoveNovelBookmark),
            "follow_user" => Some(Self::FollowUser),
            "unfollow_user" => Some(Self::UnfollowUser),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::AddBookmark => "add_bookmark",
            Self::RemoveBookmark => "remove_bookmark",
            Self::AddNovelBookmark => "add_novel_bookmark",
            Self::RemoveNovelBookmark => "remove_novel_bookmark",
            Self::FollowUser => "follow_user",
            Self::UnfollowUser => "unfollow_user",
        }
    }
    pub(crate) fn operation(self) -> &'static str {
        match self {
            Self::AddBookmark => "AddBookmark",
            Self::RemoveBookmark => "RemoveBookmark",
            Self::AddNovelBookmark => "AddNovelBookmark",
            Self::RemoveNovelBookmark => "RemoveNovelBookmark",
            Self::FollowUser => "FollowUser",
            Self::UnfollowUser => "UnfollowUser",
        }
    }
    fn target(self, input: &MutationInput) -> (&'static str, i64, String) {
        match self {
            Self::AddBookmark => (
                "illust_id",
                input.illust_id,
                format!("Bookmarked artwork {}.", input.illust_id),
            ),
            Self::RemoveBookmark => (
                "illust_id",
                input.illust_id,
                format!("Removed bookmark from artwork {}.", input.illust_id),
            ),
            Self::AddNovelBookmark => (
                "novel_id",
                input.novel_id,
                format!("Bookmarked novel {}.", input.novel_id),
            ),
            Self::RemoveNovelBookmark => (
                "novel_id",
                input.novel_id,
                format!("Removed bookmark from novel {}.", input.novel_id),
            ),
            Self::FollowUser => (
                "user_id",
                input.user_id,
                format!("Followed user {}.", input.user_id),
            ),
            Self::UnfollowUser => (
                "user_id",
                input.user_id,
                format!("Unfollowed user {}.", input.user_id),
            ),
        }
    }
}
pub fn mutation_tool(action: MutationAction) -> Value {
    let schema = match action {
        MutationAction::AddBookmark => include_str!("../schemas/add-bookmark.json"),
        MutationAction::RemoveBookmark => include_str!("../schemas/remove-bookmark.json"),
        MutationAction::AddNovelBookmark => include_str!("../schemas/add-novel-bookmark.json"),
        MutationAction::RemoveNovelBookmark => {
            include_str!("../schemas/remove-novel-bookmark.json")
        }
        MutationAction::FollowUser => include_str!("../schemas/follow-user.json"),
        MutationAction::UnfollowUser => include_str!("../schemas/unfollow-user.json"),
    };
    serde_json::from_str(schema).expect("mutation schema is valid JSON")
}
pub(crate) fn decode(
    action: MutationAction,
    arguments: Option<&Value>,
) -> Result<MutationInput, String> {
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
    crate::search::validate_schema_with_integer_binding(
        &mut arguments,
        &mutation_tool(action)["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        ("In", "int64"),
    )?;
    serde_json::from_value(arguments).map_err(|error| error.to_string())
}
async fn invoke<T: Transport>(
    client: &Client<T>,
    action: MutationAction,
    input: MutationInput,
) -> pixiv_sdk::Result<()> {
    match action {
        MutationAction::AddBookmark => {
            client
                .add_bookmark(AddBookmarkRequest {
                    artwork_id: input.illust_id,
                    restrict: input.restrict,
                    tags: input.tags,
                })
                .await
        }
        MutationAction::RemoveBookmark => {
            client
                .remove_bookmark(RemoveBookmarkRequest {
                    artwork_id: input.illust_id,
                })
                .await
        }
        MutationAction::AddNovelBookmark => {
            client
                .add_novel_bookmark(AddNovelBookmarkRequest {
                    novel_id: input.novel_id,
                    restrict: input.restrict,
                    tags: input.tags,
                })
                .await
        }
        MutationAction::RemoveNovelBookmark => {
            client
                .remove_novel_bookmark(RemoveNovelBookmarkRequest {
                    novel_id: input.novel_id,
                })
                .await
        }
        MutationAction::FollowUser => {
            client
                .follow_user(FollowUserRequest {
                    user_id: input.user_id,
                    restrict: input.restrict,
                })
                .await
        }
        MutationAction::UnfollowUser => {
            client
                .unfollow_user(UnfollowUserRequest {
                    user_id: input.user_id,
                })
                .await
        }
    }
}
pub async fn mutate<T: Transport>(
    client: &Client<T>,
    action: MutationAction,
    input: MutationInput,
) -> CallToolResult<Value> {
    let result = invoke(client, action, input.clone())
        .await
        .map_err(|error| error.to_string());
    result_for(action, &input, result)
}
pub(crate) async fn saved_mutate<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    action: MutationAction,
    input: MutationInput,
    proxy: Option<&str>,
) -> CallToolResult<Value> {
    let request = input.clone();
    let result = execution
        .write(context, 0, proxy, move |_, client| {
            let request = request.clone();
            async move { invoke(&client, action, request).await.map_err(Into::into) }
        })
        .await
        .map_err(|error| error.to_string());
    result_for(action, &input, result)
}
pub(crate) fn result_for(
    action: MutationAction,
    input: &MutationInput,
    result: Result<(), String>,
) -> CallToolResult<Value> {
    let (field, id, text) = action.target(input);
    let success = result.is_ok();
    let text = result.err().map_or(text, |error| format!("Error: {error}"));
    let mut structured = json!({"success":success,"action":action.name(),"text":text});
    if id != 0 {
        structured[field] = json!(id);
        crate::structured_wire_numbers(&mut structured[field]);
    }
    CallToolResult {
        content: vec![TextContent { kind: "text", text }],
        structured_content: structured,
        is_error: !success,
    }
}
