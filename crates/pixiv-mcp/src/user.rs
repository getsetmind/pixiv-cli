use crate::{CallToolResult, failure, record_result};
use pixiv_sdk::{Client, pixiv::UserRequest, transport::Transport};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UserDetailInput {
    pub user_id: i64,
}
pub fn user_detail_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/user-detail.json"))
        .expect("user detail schema is valid JSON")
}

pub(crate) fn decode(arguments: Option<&Value>) -> Result<UserDetailInput, String> {
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
        &user_detail_tool()["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        ("In", "int64"),
    )?;
    let user_id = arguments["user_id"].as_i64().expect("schema binds int64");
    Ok(UserDetailInput { user_id })
}
pub async fn user_detail<T: Transport>(
    client: &Client<T>,
    input: UserDetailInput,
) -> CallToolResult {
    if input.user_id <= 0 {
        return failure("user_id must be a positive integer".into());
    }
    match client
        .user(UserRequest {
            user_id: input.user_id,
        })
        .await
    {
        Ok(user) => record_result(pixiv_record::from_user_detail(&user)),
        Err(error) => failure(error.to_string()),
    }
}
pub(crate) async fn saved_user_detail<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: UserDetailInput,
    proxy: Option<&str>,
) -> CallToolResult {
    if input.user_id <= 0 {
        return failure("user_id must be a positive integer".into());
    }
    match execution
        .read(context, 0, proxy, move |_, client| async move {
            client
                .user(UserRequest {
                    user_id: input.user_id,
                })
                .await
                .map_err(Into::into)
        })
        .await
    {
        Ok(user) => record_result(pixiv_record::from_user_detail(&user)),
        Err(error) => failure(error.to_string()),
    }
}
