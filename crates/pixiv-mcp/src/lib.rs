use pixiv_sdk::{
    Client,
    reference::{REFERENCE_KIND_ARTWORK, parse_url},
    transport::Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
mod novel;
mod novel_search;
pub use novel_search::{NovelFilter, SearchNovelInput, search_novel, search_novel_tool};
mod ranking;
pub use novel::{NovelDetailInput, novel_detail, novel_detail_tool};
mod search;
mod trending;
pub use ranking::{IllustRankingInput, illust_ranking, illust_ranking_tool};
pub use trending::{TrendingTags, trending_tags_illust, trending_tags_illust_tool};
pub mod download;
pub mod reverse_search;
pub mod runtime;
pub mod stdio;
pub use search::{IllustFilter, SearchIllustInput, search_illust, search_illust_tool};

#[derive(Clone, Debug, Default, Deserialize)]
pub struct IllustReference {
    #[serde(default)]
    pub illust_id: i64,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult<T = Records> {
    pub content: Vec<TextContent>,
    pub structured_content: T,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}
#[derive(Debug, Serialize)]
pub struct TextContent {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}
#[derive(Debug, Serialize)]
pub struct Records {
    pub records: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagination: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<Value>,
}

pub fn illust_detail_tool() -> Value {
    json!({
        "name": "illust_detail",
        "description": "Get detailed information from exactly one artwork ID or supported Pixiv URL.",
        "inputSchema": {"type":"object", "additionalProperties":false, "properties":{
            "illust_id":{"type":"integer", "description":"artwork ID; provide exactly one of illust_id or url"},
            "url":{"type":"string", "description":"supported Pixiv artwork URL; provide exactly one of illust_id or url"}
        }},
        "outputSchema": {"type":"object", "additionalProperties":false, "required":["records"], "properties":{
            "records":{"type":"array", "items":{"type":"object", "additionalProperties":true, "required":["id","type","url"], "properties":{
                "id":{"type":"string"}, "type":{"type":"string"}, "url":{"type":"string"}
            }}}
        }}
    })
}

pub async fn illust_detail<T: Transport>(
    client: &Client<T>,
    input: IllustReference,
) -> CallToolResult {
    let id = match resolve_artwork(input) {
        Ok(id) => id,
        Err(error) => return failure(error),
    };
    let artwork = match client.artwork(id).await {
        Ok(artwork) => artwork,
        Err(error) => return failure(error.to_string()),
    };
    artwork_result(&artwork)
}

pub async fn saved_illust_detail<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: IllustReference,
) -> CallToolResult {
    saved_illust_detail_with_proxy(execution, context, input, None).await
}

pub(crate) async fn saved_illust_detail_with_proxy<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    input: IllustReference,
    proxy: Option<&str>,
) -> CallToolResult {
    let id = match resolve_artwork(input) {
        Ok(id) => id,
        Err(error) => return failure(error),
    };
    match execution
        .read(context, 0, proxy, move |_, client| async move {
            client.artwork(id).await.map_err(Into::into)
        })
        .await
    {
        Ok(artwork) => artwork_result(&artwork),
        Err(error) => failure(error.to_string()),
    }
}

fn artwork_result(artwork: &pixiv_sdk::models::Artwork) -> CallToolResult {
    record_result(pixiv_record::from_artwork(artwork))
}
pub(crate) fn record_result(record: Result<Value, pixiv_record::RecordError>) -> CallToolResult {
    match record {
        Ok(mut record) => {
            structured_wire_numbers(&mut record);
            CallToolResult {
                content: vec![TextContent {
                    kind: "text",
                    text: "Retrieved 1 records.".into(),
                }],
                structured_content: Records {
                    records: vec![record],
                    pagination: None,
                    filter: None,
                },
                is_error: false,
            }
        }
        Err(error) => failure(error.to_string()),
    }
}
fn structured_wire_numbers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            // Exact DTO numbers would differ from the Go MCP wrapper's float64 round trip.
            let decimal = number
                .as_f64()
                .expect("DTO number fits float64")
                .to_string();
            *value = serde_json::from_str(&decimal).expect("finite DTO number is JSON");
        }
        Value::Array(values) => values.iter_mut().for_each(structured_wire_numbers),
        Value::Object(values) => values.values_mut().for_each(structured_wire_numbers),
        _ => {}
    }
}
fn resolve_artwork(input: IllustReference) -> Result<i64, String> {
    let has_id = input.illust_id != 0;
    let has_url = !input.url.trim().is_empty();
    if has_id == has_url {
        return Err("provide exactly one of illust_id or url".into());
    }
    if has_id {
        if input.illust_id <= 0 {
            return Err("illust_id must be a positive integer".into());
        }
        return Ok(input.illust_id);
    }
    let reference = parse_url(&input.url).map_err(|error| error.to_string())?;
    if reference.kind != REFERENCE_KIND_ARTWORK {
        return Err("URL does not name a Pixiv artwork".into());
    }
    Ok(reference.id)
}
fn failure(error: String) -> CallToolResult {
    CallToolResult {
        content: vec![TextContent {
            kind: "text",
            text: format!("Error: {error}"),
        }],
        structured_content: Records {
            records: vec![],
            pagination: None,
            filter: None,
        },
        is_error: true,
    }
}

mod user;
pub use user::{UserDetailInput, user_detail, user_detail_tool};

mod user_search;
pub use user_search::{SearchUserInput, UserFilter, search_user, search_user_tool};

mod mutation;
pub use mutation::{MutationAction, MutationInput, mutate, mutation_tool};

mod novel_series;
pub use novel_series::{NovelSeriesInput, novel_series, novel_series_tool};
mod novel_content;
pub use novel_content::{NovelContentInput, novel_content, novel_content_tool};

mod illust_series;
pub use illust_series::{IllustSeriesInput, illust_series, illust_series_tool};

mod artwork_feed;
pub use artwork_feed::{
    IllustRecommendedInput, IllustRelatedInput, illust_recommended, illust_recommended_tool,
    illust_related, illust_related_tool,
};

mod recommended;
pub use recommended::{RecommendedInput, recommended, recommended_tool};

mod user_works;
pub use user_works::{
    UserArtworksInput, UserNovelsInput, user_artworks, user_artworks_tool, user_novels,
    user_novels_tool,
};

mod user_relationships;
pub use user_relationships::{
    UserRelationship, UserRelationshipInput, user_relationship, user_relationship_tool,
};

mod bookmark_lists;
pub use bookmark_lists::{BookmarkList, BookmarkListInput, bookmark_list, bookmark_list_tool};

mod bookmark_reads;
pub use bookmark_reads::{
    BookmarkDetailOutput, BookmarkRead, BookmarkReadInput, BookmarkReadOutput, BookmarkTagOutput,
    BookmarkTagsOutput, bookmark_read, bookmark_read_tool,
};

mod timeline;
pub use timeline::{Timeline, TimelineInput, timeline, timeline_tool};

mod mypixiv;
pub use mypixiv::{MyPixiv, MyPixivInput, my_pixiv, my_pixiv_tool};

mod comment_reads;
pub use comment_reads::{
    CommentRead, CommentReadInput, CommentReadOutput, comment_read, comment_read_tool,
};

mod comment_mutations;
pub use comment_mutations::{
    CommentMutation, CommentMutationInput, comment_mutation, comment_mutation_tool,
};

pub mod fanbox;
