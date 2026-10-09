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
