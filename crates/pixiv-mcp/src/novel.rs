use crate::{CallToolResult, failure, record_result};
use pixiv_sdk::{Client, pixiv::NovelRequest, transport::Transport};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Clone, Debug, Default, Deserialize)]
pub struct NovelDetailInput {
    pub novel_id: i64,
}
pub fn novel_detail_tool() -> Value {
    let mut tool = crate::illust_detail_tool();
    tool["name"] = json!("novel_detail");
    tool["description"] = json!("Get detailed information for one Pixiv novel.");
    tool["inputSchema"] = json!({"type":"object","additionalProperties":false,"required":["novel_id"],"properties":{"novel_id":{"type":"integer","description":"positive Pixiv novel ID"}}});
    tool
}
pub(crate) fn decode(arguments: Option<&Value>) -> Result<NovelDetailInput, String> {
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
        &novel_detail_tool()["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        ("In", "int64"),
    )?;
    let novel_id = arguments["novel_id"].as_i64().expect("schema binds int64");
    Ok(NovelDetailInput { novel_id })
}
pub async fn novel_detail<T: Transport>(
    client: &Client<T>,
    input: NovelDetailInput,
) -> CallToolResult {
    if input.novel_id <= 0 {
        return failure("novel_id must be a positive integer".into());
    }
    match client
        .novel(NovelRequest {
            novel_id: input.novel_id,
        })
        .await
    {
        Ok(novel) => record_result(pixiv_record::from_novel(&novel)),
        Err(error) => failure(error.to_string()),
    }
}
pub(crate) async fn saved_novel_detail<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: NovelDetailInput,
    proxy: Option<&str>,
) -> CallToolResult {
    if input.novel_id <= 0 {
        return failure("novel_id must be a positive integer".into());
    }
    match execution
        .read(context, 0, proxy, move |_, client| async move {
            client
                .novel(NovelRequest {
                    novel_id: input.novel_id,
                })
                .await
                .map_err(Into::into)
        })
        .await
    {
        Ok(novel) => record_result(pixiv_record::from_novel(&novel)),
        Err(error) => failure(error.to_string()),
    }
}
