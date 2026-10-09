use crate::{
    CommandError, DetailOutput,
    novel_list::{Listing, Source},
    search::{SearchInput, SearchOptions},
};
use pixiv_sdk::{Client, transport::Transport};
use std::io::Write;
fn listing(
    input: &SearchInput,
    options: &SearchOptions,
    word: &str,
) -> Result<Listing, CommandError> {
    Ok(Listing {
        source: Source::Search(input.novel_request(options, word)?),
        heading: format!("novels for {}", crate::search::quote(word)),
        plan: options.plan()?,
    })
}
pub async fn novel_search<T: Transport, W: Write>(
    client: &Client<T>,
    input: &SearchInput,
    options: &SearchOptions,
    word: &str,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    if let Some(mut spool) =
        crate::novel_list::attempt(client, &listing(input, options, word)?, mode, out).await?
    {
        spool.commit(out)?;
    }
    Ok(())
}
pub async fn saved_novel_search<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    listing_input: (&SearchInput, &SearchOptions, &str),
    proxy: Option<&str>,
    mode: DetailOutput,
    out: W,
) -> Result<(), CommandError> {
    crate::novel_list::saved(
        execution,
        context,
        listing(listing_input.0, listing_input.1, listing_input.2)?,
        proxy,
        mode,
        out,
    )
    .await
}

#[derive(clap::Args)]
pub struct NovelSearchOptions {
    #[arg(num_args = 0..)]
    pub query: Vec<String>,
    #[arg(long, short = 'j', num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    pub json: Option<bool>,
    #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", default_value = "false")]
    pub ndjson: bool,
    #[arg(long, default_value = "tag-partial", allow_hyphen_values = true)]
    pub search_by: String,
    #[arg(long, default_value = "date_desc", allow_hyphen_values = true)]
    pub sort: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    pub period: String,
    #[arg(long, short = 'l', allow_hyphen_values = true)]
    pub limit: Option<i64>,
    #[arg(long, short = 'p', allow_hyphen_values = true)]
    pub page: Option<i64>,
}
impl NovelSearchOptions {
    pub fn into_search(self) -> (SearchInput, SearchOptions) {
        (
            SearchInput::for_novel(self.query, self.json, self.ndjson),
            SearchOptions {
                search_by: self.search_by,
                sort: self.sort,
                limit: self.limit,
                page: self.page,
                dates: crate::search::SearchDateOptions {
                    period: self.period,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
    }
}
