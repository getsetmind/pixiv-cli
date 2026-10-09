use crate::CommandError;
use pixiv_app::{execution::Execution, lifecycle::Context, scheduler::SchedulerError};
use pixiv_sdk::{
    Client, models::TrendingTag, pixiv::TrendingArtworkTagsRequest, transport::Transport,
};
use std::io::Write;

#[derive(serde::Serialize)]
struct Output<'a> {
    tags: Vec<pixiv_sdk::dto::TrendingTagDto<'a>>,
}

pub async fn trending_tags<T: Transport, W: Write>(
    client: &Client<T>,
    json: bool,
    out: &mut W,
) -> Result<(), CommandError> {
    let tags = client
        .trending_artwork_tags(TrendingArtworkTagsRequest::default())
        .await?;
    present(&tags, json, out)
}

pub async fn saved_trending_tags<T: Transport + 'static, W: Write>(
    execution: &Execution<T>,
    context: &Context,
    proxy: Option<&str>,
    json: bool,
    out: &mut W,
) -> Result<(), CommandError> {
    let tags = execution
        .read(context, 0, proxy, |_, client| async move {
            client
                .trending_artwork_tags(TrendingArtworkTagsRequest::default())
                .await
                .map_err(SchedulerError::from)
        })
        .await?;
    present(&tags, json, out)
}

fn present<W: Write>(tags: &[TrendingTag], json: bool, out: &mut W) -> Result<(), CommandError> {
    if json {
        let body = serde_json::to_string_pretty(&Output {
            tags: tags
                .iter()
                .map(pixiv_sdk::dto::TrendingTagDto::from)
                .collect(),
        })
        .map_err(std::io::Error::other)?;
        writeln!(out, "{}", crate::go_json_escape(body))?;
    } else {
        for tag in tags {
            let translated = if tag.translated_name.is_empty() {
                "none"
            } else {
                &tag.translated_name
            };
            writeln!(
                out,
                "{} (translation: {})",
                crate::safe_line(&tag.tag),
                crate::safe_line(translated)
            )?;
        }
    }
    Ok(())
}
