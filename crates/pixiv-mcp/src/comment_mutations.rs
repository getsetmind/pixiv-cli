use crate::{CallToolResult, TextContent};
use pixiv_sdk::{Client, pixiv::*, transport::Transport};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct CommentMutationInput {
    pub illust_id: i64,
    pub novel_id: i64,
    pub comment_id: i64,
    pub parent_comment_id: i64,
    pub stamp_id: i64,
    pub comment: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommentMutation {
    CreateArtwork,
    ReplyArtwork,
    DeleteArtwork,
    StampArtwork,
    CreateNovel,
    ReplyNovel,
    DeleteNovel,
    StampNovel,
}
impl CommentMutation {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "create_artwork_comment" => Some(Self::CreateArtwork),
            "reply_artwork_comment" => Some(Self::ReplyArtwork),
            "delete_artwork_comment" => Some(Self::DeleteArtwork),
            "stamp_artwork_comment" => Some(Self::StampArtwork),
            "create_novel_comment" => Some(Self::CreateNovel),
            "reply_novel_comment" => Some(Self::ReplyNovel),
            "delete_novel_comment" => Some(Self::DeleteNovel),
            "stamp_novel_comment" => Some(Self::StampNovel),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::CreateArtwork => "create_artwork_comment",
            Self::ReplyArtwork => "reply_artwork_comment",
            Self::DeleteArtwork => "delete_artwork_comment",
            Self::StampArtwork => "stamp_artwork_comment",
            Self::CreateNovel => "create_novel_comment",
            Self::ReplyNovel => "reply_novel_comment",
            Self::DeleteNovel => "delete_novel_comment",
            Self::StampNovel => "stamp_novel_comment",
        }
    }
    pub(crate) fn operation(self) -> &'static str {
        match self {
            Self::CreateArtwork => "PostArtworkComment",
            Self::ReplyArtwork => "ReplyArtworkComment",
            Self::DeleteArtwork => "DeleteArtworkComment",
            Self::StampArtwork => "StampArtworkComment",
            Self::CreateNovel => "PostNovelComment",
            Self::ReplyNovel => "ReplyNovelComment",
            Self::DeleteNovel => "DeleteNovelComment",
            Self::StampNovel => "StampNovelComment",
        }
    }
    fn target(self, input: &CommentMutationInput) -> (&'static str, i64, String) {
        match self {
            Self::CreateArtwork => (
                "illust_id",
                input.illust_id,
                format!("Created comment on artwork {}.", input.illust_id),
            ),
            Self::ReplyArtwork => (
                "illust_id",
                input.illust_id,
                format!(
                    "Replied to comment {} on artwork {}.",
                    input.parent_comment_id, input.illust_id
                ),
            ),
            Self::DeleteArtwork => (
                "comment_id",
                input.comment_id,
                format!("Deleted comment {}.", input.comment_id),
            ),
            Self::StampArtwork => (
                "illust_id",
                input.illust_id,
                format!("Added stamp comment to artwork {}.", input.illust_id),
            ),
            Self::CreateNovel => (
                "novel_id",
                input.novel_id,
                format!("Created comment on novel {}.", input.novel_id),
            ),
            Self::ReplyNovel => (
                "novel_id",
                input.novel_id,
                format!(
                    "Replied to comment {} on novel {}.",
                    input.parent_comment_id, input.novel_id
                ),
            ),
            Self::DeleteNovel => (
                "comment_id",
                input.comment_id,
                format!("Deleted comment {}.", input.comment_id),
            ),
            Self::StampNovel => (
                "novel_id",
                input.novel_id,
                format!("Added stamp comment to novel {}.", input.novel_id),
            ),
        }
    }
}
pub fn comment_mutation_tool(action: CommentMutation) -> Value {
    serde_json::from_str(match action {
        CommentMutation::CreateArtwork => include_str!("../schemas/create-artwork-comment.json"),
        CommentMutation::ReplyArtwork => include_str!("../schemas/reply-artwork-comment.json"),
        CommentMutation::DeleteArtwork => include_str!("../schemas/delete-artwork-comment.json"),
        CommentMutation::StampArtwork => include_str!("../schemas/stamp-artwork-comment.json"),
        CommentMutation::CreateNovel => include_str!("../schemas/create-novel-comment.json"),
        CommentMutation::ReplyNovel => include_str!("../schemas/reply-novel-comment.json"),
        CommentMutation::DeleteNovel => include_str!("../schemas/delete-novel-comment.json"),
        CommentMutation::StampNovel => include_str!("../schemas/stamp-novel-comment.json"),
    })
    .expect("comment mutation schema is valid JSON")
}
pub(crate) fn decode(
    action: CommentMutation,
    arguments: Option<&Value>,
) -> Result<CommentMutationInput, String> {
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
        &comment_mutation_tool(action)["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        ("In", "int64"),
    )?;
    serde_json::from_value(arguments).map_err(|error| error.to_string())
}
async fn invoke<T: Transport>(
    client: &Client<T>,
    action: CommentMutation,
    input: CommentMutationInput,
) -> pixiv_sdk::Result<i64> {
    match action {
        CommentMutation::CreateArtwork => client
            .post_artwork_comment(PostArtworkCommentRequest {
                artwork_id: input.illust_id,
                comment: input.comment,
            })
            .await
            .map(|result| result.comment_id),
        CommentMutation::ReplyArtwork => client
            .reply_artwork_comment(ReplyArtworkCommentRequest {
                artwork_id: input.illust_id,
                comment: input.comment,
                parent_comment_id: input.parent_comment_id,
            })
            .await
            .map(|result| result.comment_id),
        CommentMutation::DeleteArtwork => client
            .delete_artwork_comment(DeleteArtworkCommentRequest {
                comment_id: input.comment_id,
            })
            .await
            .map(|()| input.comment_id),
        CommentMutation::StampArtwork => client
            .stamp_artwork_comment(StampArtworkCommentRequest {
                artwork_id: input.illust_id,
                comment: input.comment,
                stamp_id: input.stamp_id,
            })
            .await
            .map(|result| result.comment_id),
        CommentMutation::CreateNovel => client
            .post_novel_comment(PostNovelCommentRequest {
                novel_id: input.novel_id,
                comment: input.comment,
            })
            .await
            .map(|result| result.comment_id),
        CommentMutation::ReplyNovel => client
            .reply_novel_comment(ReplyNovelCommentRequest {
                novel_id: input.novel_id,
                comment: input.comment,
                parent_comment_id: input.parent_comment_id,
            })
            .await
            .map(|result| result.comment_id),
        CommentMutation::DeleteNovel => client
            .delete_novel_comment(DeleteNovelCommentRequest {
                comment_id: input.comment_id,
            })
            .await
            .map(|()| input.comment_id),
        CommentMutation::StampNovel => client
            .stamp_novel_comment(StampNovelCommentRequest {
                novel_id: input.novel_id,
                comment: input.comment,
                stamp_id: input.stamp_id,
            })
            .await
            .map(|result| result.comment_id),
    }
}
pub async fn comment_mutation<T: Transport>(
    client: &Client<T>,
    action: CommentMutation,
    input: CommentMutationInput,
) -> CallToolResult<Value> {
    match invoke(client, action, input.clone()).await {
        Ok(id) => result_for(action, &input, id, Ok(())),
        Err(error) => result_for(action, &input, 0, Err(error.to_string())),
    }
}
pub(crate) async fn saved_mutate<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    action: CommentMutation,
    input: CommentMutationInput,
    proxy: Option<&str>,
) -> CallToolResult<Value> {
    let request = input.clone();
    let returned_id = Arc::new(Mutex::new(0));
    let captured_id = returned_id.clone();
    let result = execution
        .write(context, 0, proxy, move |_, client| {
            let request = request.clone();
            let captured_id = captured_id.clone();
            async move {
                let id = invoke(&client, action, request).await?;
                *captured_id
                    .lock()
                    .expect("comment result lock is not poisoned") = id;
                Ok(())
            }
        })
        .await
        .map_err(|error| error.to_string());
    let id = *returned_id
        .lock()
        .expect("comment result lock is not poisoned");
    result_for(action, &input, id, result)
}
pub(crate) fn result_for(
    action: CommentMutation,
    input: &CommentMutationInput,
    returned_id: i64,
    result: Result<(), String>,
) -> CallToolResult<Value> {
    let (field, id, text) = action.target(input);
    let success = result.is_ok();
    let text = result.err().map_or(text, |error| format!("Error: {error}"));
    let mut structured = json!({"success":success,"action":action.name(),"text":text});
    if id != 0 {
        structured[field] = json!(id);
    }
    if returned_id != 0 {
        structured["comment_id"] = json!(returned_id);
    }
    crate::structured_wire_numbers(&mut structured);
    CallToolResult {
        content: vec![TextContent { kind: "text", text }],
        structured_content: structured,
        is_error: !success,
    }
}
