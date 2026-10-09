use crate::{CommandError, DetailOutput, json_spool::JsonSpool};
use pixiv_app::{
    execution::Execution, facade::UseOutcome, lifecycle::Context, scheduler::SchedulerError,
};
use pixiv_sdk::{Client, cursor::Cursor, transport::Transport};
use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone)]
pub(crate) enum Source {
    Ranking(pixiv_sdk::pixiv::NovelRankingRequest),
    Following(pixiv_sdk::pixiv::FollowingNovelsRequest),
    Latest(pixiv_sdk::pixiv::LatestNovelsRequest),
    MyPixiv(pixiv_sdk::pixiv::MyPixivNovelsRequest),
    MyPixivUser(pixiv_sdk::pixiv::UserNovelsRequest),
    Bookmarks(
        pixiv_sdk::pixiv::UserNovelBookmarksRequest,
        Arc<std::sync::atomic::AtomicI64>,
    ),
    Series(pixiv_sdk::pixiv::NovelSeriesRequest),
    Search(pixiv_sdk::pixiv::SearchNovelsRequest),
    User(
        pixiv_sdk::pixiv::UserNovelsRequest,
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
    if let Source::Series(request) = &listing.source {
        return series_attempt(client, request, &listing.plan, mode, out).await;
    }
    let plan = &listing.plan;
    let mut spool = if mode == DetailOutput::Json {
        Some(JsonSpool::with_key("novels")?)
    } else {
        None
    };
    let mut heading = false;
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
                    Source::Series(_) => unreachable!("series uses collected presentation"),
                    Source::User(mut request, identity) => {
                        request.user_id = crate::user_works::current_target(client, &identity)?;
                        request.cursor = cursor;
                        client.user_novels(request).await
                    }
                    Source::Bookmarks(mut request, identity) => {
                        request.user_id =
                            crate::bookmark_lists::current_target(client, &identity, false)?;
                        request.cursor = cursor;
                        client.user_novel_bookmarks(request).await
                    }
                    Source::Following(mut request) => {
                        request.cursor = cursor;
                        client.following_novels(request).await
                    }
                    Source::MyPixiv(mut request) => {
                        request.cursor = cursor;
                        client.my_pixiv_novels(request).await
                    }
                    Source::MyPixivUser(mut request) => {
                        request.cursor = cursor;
                        client.user_novels(request).await
                    }
                    Source::Latest(mut request) => {
                        request.cursor = cursor;
                        client.latest_novels(request).await
                    }
                    Source::Ranking(mut request) => {
                        request.cursor = cursor;
                        client.novel_ranking(request).await
                    }
                    Source::Search(mut request) => {
                        request.cursor = cursor;
                        client.search_novels(request).await
                    }
                }
                .map_err(CommandError::from)?;
                Ok((page.items, page.next))
            }
        },
        |_: &pixiv_sdk::models::Novel| Ok(true),
        None::<fn(Cursor, usize) -> Result<Cursor, CommandError>>,
        |items| {
            if let Some(spool) = &mut spool {
                return spool.append_novels(&items);
            }
            if mode == DetailOutput::Ndjson {
                for item in &items {
                    let record = pixiv_record::from_novel(item)
                        .map_err(|error| CommandError::Message(error.message()))?;
                    let encoded = serde_json::to_string(&record).map_err(std::io::Error::other)?;
                    writeln!(out, "{}", crate::go_json_escape(encoded))?;
                }
                return Ok(());
            }
            if !heading {
                writeln!(out, "{}", listing.heading)?;
                heading = true;
            }
            for item in &items {
                if matches!(listing.source, Source::User(..)) {
                    writeln!(
                        out,
                        "{} {} — {}",
                        item.id,
                        crate::safe_line(&item.title),
                        crate::safe_line(&item.user.name)
                    )?;
                } else {
                    writeln!(out, "{} {} — {}", item.id, item.title, item.user.name)?;
                }
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

async fn series_attempt<T: Transport, W: Write>(
    client: &Client<T>,
    request: &pixiv_sdk::pixiv::NovelSeriesRequest,
    plan: &crate::search::SearchPlan,
    mode: DetailOutput,
    out: &mut W,
) -> Result<Option<JsonSpool>, CommandError> {
    let metadata = Mutex::new(pixiv_sdk::models::NovelSeries::default());
    let mut novels = vec![];
    pixiv_app::pagination::traverse_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip as i64,
            limit: plan.limit as i64,
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| {
            let mut request = request.clone();
            request.cursor = cursor;
            let metadata = &metadata;
            async move {
                let result = client
                    .novel_series(request)
                    .await
                    .map_err(CommandError::from)?;
                let mut metadata = metadata.lock().unwrap_or_else(|error| error.into_inner());
                if metadata.id == 0 {
                    *metadata = result.series;
                }
                Ok((result.novels.items, result.novels.next))
            }
        },
        |_: &pixiv_sdk::models::Novel| Ok(true),
        None::<fn(Cursor, usize) -> Result<Cursor, CommandError>>,
        |items| {
            novels.extend(items);
            Ok(())
        },
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => CommandError::MessageText(message),
    })?;
    let metadata = metadata
        .into_inner()
        .unwrap_or_else(|error| error.into_inner());
    if mode == DetailOutput::Json {
        let mut spool = JsonSpool::with_key("novels")?;
        spool.append_novels(&novels)?;
        spool.add_field("series", &pixiv_sdk::dto::NovelSeriesDto::from(&metadata))?;
        spool.commit(out)?;
        return Ok(None);
    }
    if mode == DetailOutput::Ndjson {
        for item in &novels {
            let record = pixiv_record::from_novel(item)
                .map_err(|error| CommandError::Message(error.message()))?;
            writeln!(
                out,
                "{}",
                crate::go_json_escape(
                    serde_json::to_string(&record).map_err(std::io::Error::other)?
                )
            )?;
        }
    } else {
        writeln!(out, "series {}: {}", metadata.id, metadata.title)?;
        for item in &novels {
            writeln!(out, "{} {} — {}", item.id, item.title, item.user.name)?;
        }
    }
    Ok(None)
}
