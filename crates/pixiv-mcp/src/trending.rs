use crate::{CallToolResult, TextContent};
use pixiv_sdk::{
    Client, models::TrendingTag, pixiv::TrendingArtworkTagsRequest, transport::Transport,
};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct TrendingTags {
    pub tags: Vec<Value>,
    pub text: String,
}

pub fn trending_tags_illust_tool() -> Value {
    serde_json::from_str(include_str!("../schemas/trending-tags-illust.json"))
        .expect("trending tags tool schema is valid JSON")
}

pub async fn trending_tags_illust<T: Transport>(
    client: &Client<T>,
) -> CallToolResult<TrendingTags> {
    match client
        .trending_artwork_tags(TrendingArtworkTagsRequest::default())
        .await
    {
        Ok(tags) => present(&tags),
        Err(error) => failure(error.to_string()),
    }
}

pub(crate) async fn saved_trending_tags_illust<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    proxy: Option<&str>,
) -> CallToolResult<TrendingTags> {
    match execution
        .read(context, 0, proxy, |_, client| async move {
            client
                .trending_artwork_tags(TrendingArtworkTagsRequest::default())
                .await
                .map_err(Into::into)
        })
        .await
    {
        Ok(tags) => present(&tags),
        Err(error) => failure(error.to_string()),
    }
}

fn present(tags: &[TrendingTag]) -> CallToolResult<TrendingTags> {
    let text = if tags.is_empty() {
        "No trending tags found.".into()
    } else {
        let lines = tags
            .iter()
            .map(|tag| {
                let translated = if tag.translated_name.is_empty() {
                    "none"
                } else {
                    &tag.translated_name
                };
                format!("- {} (translation: {translated})", tag.tag)
            })
            .collect::<Vec<_>>();
        format!("Trending tags:\n{}", lines.join("\n"))
    };
    let tags = tags
        .iter()
        .map(|tag| {
            let mut value = serde_json::to_value(pixiv_sdk::dto::TrendingTagDto::from(tag))
                .expect("trending tag DTO is serializable");
            crate::structured_wire_numbers(&mut value);
            value
        })
        .collect();
    result(tags, text, false)
}

pub(crate) fn failure(error: String) -> CallToolResult<TrendingTags> {
    result(vec![], format!("Error: {error}"), true)
}

fn result(tags: Vec<Value>, text: String, is_error: bool) -> CallToolResult<TrendingTags> {
    CallToolResult {
        content: vec![TextContent {
            kind: "text",
            text: text.clone(),
        }],
        structured_content: TrendingTags { tags, text },
        is_error,
    }
}

pub(crate) fn decode(arguments: Option<&Value>) -> Result<(), String> {
    let Some(arguments) = arguments.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let Some(arguments) = arguments.as_object() else {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            crate::stdio::value_type(arguments)
        ));
    };
    if arguments.is_empty() {
        return Ok(());
    }
    let extra = arguments
        .keys()
        .map(|key| format!("{key:?}"))
        .collect::<Vec<_>>()
        .join(" ");
    Err(format!(
        "invalid params: validating \"arguments\": validating root: unexpected additional properties [{extra}]"
    ))
}
