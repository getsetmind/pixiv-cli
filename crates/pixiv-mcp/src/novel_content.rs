use crate::{CallToolResult, TextContent};
use pixiv_sdk::{Client, pixiv::NovelContentRequest, transport::Transport};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Clone, Debug, Default, Deserialize)]
pub struct NovelContentInput {
    pub novel_id: i64,
}
pub fn novel_content_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/novel-content.json"))
        .expect("novel content schema is valid JSON")
}
pub(crate) fn decode(arguments: Option<&Value>) -> Result<NovelContentInput, String> {
    crate::novel_series::decode_input(arguments, &novel_content_tool()["inputSchema"])
}
pub(crate) fn failure(message: String) -> CallToolResult<Value> {
    CallToolResult {
        content: vec![TextContent {
            kind: "text",
            text: format!("Error: {message}"),
        }],
        structured_content: json!({"content":{"novel_id":0,"title":"","caption":"","blocks":[]}}),
        is_error: true,
    }
}
fn result(value: Result<pixiv_sdk::models::NovelContent, String>) -> CallToolResult<Value> {
    match value {
        Ok(content) => {
            let mut dto = serde_json::to_value(pixiv_sdk::dto::NovelContentDto::from(&content))
                .expect("DTO serializes");
            crate::structured_wire_numbers(&mut dto);
            CallToolResult {
                content: vec![TextContent {
                    kind: "text",
                    text: format!("Retrieved novel content for {}.", content.novel_id),
                }],
                structured_content: json!({"content":dto}),
                is_error: false,
            }
        }
        Err(error) => failure(error),
    }
}
pub async fn novel_content<T: Transport>(
    client: &Client<T>,
    input: NovelContentInput,
) -> CallToolResult<Value> {
    if input.novel_id <= 0 {
        return failure("novel_id must be a positive integer".into());
    }
    result(
        client
            .novel_content(NovelContentRequest {
                novel_id: input.novel_id,
            })
            .await
            .map_err(|error| error.to_string()),
    )
}
pub(crate) async fn saved_novel_content<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: NovelContentInput,
    proxy: Option<&str>,
) -> CallToolResult<Value> {
    if input.novel_id <= 0 {
        return failure("novel_id must be a positive integer".into());
    }
    result(
        execution
            .read(context, 0, proxy, move |_, client| async move {
                client
                    .novel_content(NovelContentRequest {
                        novel_id: input.novel_id,
                    })
                    .await
                    .map_err(Into::into)
            })
            .await
            .map_err(|error| error.to_string()),
    )
}
