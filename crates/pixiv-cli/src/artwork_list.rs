use crate::{CommandError, DetailOutput, json_spool::JsonSpool};
use pixiv_app::{
    execution::Execution, facade::UseOutcome, lifecycle::Context, scheduler::SchedulerError,
};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::Artwork,
    pixiv::{ArtworkRankingRequest, ArtworkSeriesRequest},
    transport::Transport,
};
use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone)]
pub(crate) enum Source {
    Ranking(ArtworkRankingRequest),
    Series(ArtworkSeriesRequest),
}
#[derive(Clone)]
pub(crate) struct Listing {
    pub(crate) source: Source,
    pub(crate) heading: String,
    pub(crate) plan: crate::search::SearchPlan,
}
pub(crate) async fn saved<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    listing: Listing,
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    let output = Arc::new(Mutex::new(output));
    let callback_output = output.clone();
    let spool = Arc::new(Mutex::new(None));
    let staged = spool.clone();
    let terminal = Arc::new(Mutex::new(None));
    let retained = terminal.clone();
    let result = execution
        .use_client(
            Some(context),
            0,
            proxy,
            Some(Arc::new(move |_, client| {
                let listing = listing.clone();
                let committed = Arc::new(AtomicBool::new(false));
                let mut writer = crate::search::SearchWriter {
                    output: callback_output.clone(),
                    committed: committed.clone(),
                };
                let staged = staged.clone();
                let retained = retained.clone();
                Box::pin(async move {
                    let error = match attempt(&client, &listing, mode, &mut writer).await {
                        Ok(value) => {
                            *staged.lock().unwrap_or_else(|e| e.into_inner()) = value;
                            None
                        }
                        Err(CommandError::Sdk(error)) => Some(SchedulerError::from(error)),
                        Err(CommandError::App(error)) => Some(error),
                        Err(error) => {
                            let message = error.to_string();
                            *retained.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
                            Some(SchedulerError::Message(message))
                        }
                    };
                    UseOutcome {
                        committed: committed.load(Ordering::Acquire),
                        error,
                    }
                })
            })),
        )
        .await;
    if let Some(error) = terminal.lock().unwrap_or_else(|e| e.into_inner()).take() {
        return Err(error);
    }
    result?;
    if let Some(mut spool) = spool.lock().unwrap_or_else(|e| e.into_inner()).take() {
        spool.commit(&mut *output.lock().unwrap_or_else(|e| e.into_inner()))?;
    }
    Ok(())
}

pub(crate) async fn attempt<T: Transport, W: Write>(
    client: &Client<T>,
    listing: &Listing,
    mode: DetailOutput,
    out: &mut W,
) -> Result<Option<JsonSpool>, CommandError> {
    let plan = listing.plan.clone();
    let mut spool = if mode == DetailOutput::Json {
        Some(JsonSpool::new()?)
    } else {
        None
    };
    let mut heading = false;
    let mut position = plan.skip;
    pixiv_app::pagination::traverse_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip as i64,
            limit: plan.limit as i64,
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| {
            let source = listing.source.clone();
            async move {
                let page = match source {
                    Source::Ranking(mut request) => {
                        request.cursor = cursor;
                        client.artwork_ranking(request).await
                    }
                    Source::Series(mut request) => {
                        request.cursor = cursor;
                        client.artwork_series(request).await
                    }
                }
                .map_err(CommandError::from)?;
                Ok((page.items, page.next))
            }
        },
        |_: &Artwork| Ok(true),
        None::<fn(Cursor, usize) -> Result<Cursor, CommandError>>,
        |items| {
            if let Some(spool) = &mut spool {
                return spool.append(&items);
            }
            if mode == DetailOutput::Ndjson {
                return crate::search::present_search("", &items, mode, &mut false, out);
            }
            if !heading {
                writeln!(out, "{}", listing.heading)?;
                heading = true;
            }
            for item in &items {
                position += 1;
                writeln!(
                    out,
                    "#{position} https://www.pixiv.net/artworks/{}",
                    item.id
                )?;
                let tags = item
                    .tags
                    .iter()
                    .map(|tag| tag.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(
                    out,
                    "{} {} by {} bookmarks:{} views:{} tags:{}",
                    item.id,
                    crate::search::quote(&item.title),
                    item.user.name,
                    item.total_bookmarks,
                    item.total_views,
                    tags
                )?;
            }
            Ok(())
        },
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => CommandError::MessageText(message),
    })?;
    Ok(spool)
}
