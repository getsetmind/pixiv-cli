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
    Following(
        pixiv_sdk::pixiv::FollowingArtworksRequest,
        pixiv_app::search_filter::ArtworkFilter,
    ),
    Latest(pixiv_sdk::pixiv::LatestArtworksRequest),
    MyPixiv(pixiv_sdk::pixiv::MyPixivArtworksRequest),
    MyPixivUser(pixiv_sdk::pixiv::UserArtworksRequest),
    Bookmarks(
        pixiv_sdk::pixiv::UserArtworkBookmarksRequest,
        Arc<std::sync::atomic::AtomicI64>,
        bool,
    ),
    Series(ArtworkSeriesRequest),
    User(
        pixiv_sdk::pixiv::UserArtworksRequest,
        Arc<std::sync::atomic::AtomicI64>,
    ),
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
                    Source::Bookmarks(mut request, identity, user_alias) => {
                        request.user_id =
                            crate::bookmark_lists::current_target(client, &identity, user_alias)?;
                        request.cursor = cursor;
                        client.user_artwork_bookmarks(request).await
                    }
                    Source::Following(mut request, filter) => {
                        request.cursor = cursor;
                        client.following_artworks(request).await.map(|mut page| {
                            page.items.retain(|item| {
                                filter.matches(
                                    item.x_restrict,
                                    match item.kind {
                                        pixiv_sdk::models::ArtworkKind::Illust => "illust",
                                        pixiv_sdk::models::ArtworkKind::Manga => "manga",
                                        pixiv_sdk::models::ArtworkKind::Ugoira => "ugoira",
                                        pixiv_sdk::models::ArtworkKind::Unknown => "unknown",
                                    },
                                )
                            });
                            page
                        })
                    }
                    Source::MyPixiv(mut request) => {
                        request.cursor = cursor;
                        client.my_pixiv_artworks(request).await
                    }
                    Source::MyPixivUser(mut request) => {
                        request.cursor = cursor;
                        client.user_artworks(request).await
                    }
                    Source::Latest(mut request) => {
                        request.cursor = cursor;
                        client.latest_artworks(request).await
                    }
                    Source::Ranking(mut request) => {
                        request.cursor = cursor;
                        client.artwork_ranking(request).await
                    }
                    Source::User(mut request, identity) => {
                        request.user_id = crate::user_works::current_target(client, &identity)?;
                        request.cursor = cursor;
                        client.user_artworks(request).await
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
                let heading_text = match &listing.source {
                    Source::Bookmarks(_, identity, _) => {
                        format!("bookmarks by {}", identity.load(Ordering::Acquire))
                    }
                    Source::User(_, identity) => {
                        format!("artworks by {}", identity.load(Ordering::Acquire))
                    }
                    _ => listing.heading.clone(),
                };
                writeln!(out, "{heading_text}")?;
                heading = true;
            }
            for item in &items {
                position += 1;
                let user = matches!(
                    listing.source,
                    Source::User(..) | Source::Bookmarks(_, _, true)
                );
                let plain = matches!(
                    listing.source,
                    Source::Bookmarks(..)
                        | Source::Following(..)
                        | Source::Latest(..)
                        | Source::MyPixiv(..)
                        | Source::MyPixivUser(..)
                );
                if user || plain {
                    writeln!(out, "https://www.pixiv.net/artworks/{}", item.id)?;
                } else {
                    writeln!(
                        out,
                        "#{position} https://www.pixiv.net/artworks/{}",
                        item.id
                    )?;
                }
                let tags = item
                    .tags
                    .iter()
                    .map(|tag| {
                        if user {
                            crate::safe_line(&tag.name)
                        } else {
                            tag.name.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(
                    out,
                    "{} {} by {} bookmarks:{} views:{} tags:{}",
                    item.id,
                    crate::search::quote(&item.title),
                    if user {
                        crate::safe_line(&item.user.name)
                    } else {
                        item.user.name.clone()
                    },
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
