use crate::{
    CommandError, DetailOutput,
    json_spool::JsonSpool,
    search::{SearchInput, SearchOptions},
};
use pixiv_app::{
    execution::Execution, facade::UseOutcome, lifecycle::Context, scheduler::SchedulerError,
};
use pixiv_sdk::{Client, cursor::Cursor, pixiv::SearchUsersRequest, transport::Transport};
use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub async fn saved_user_search<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    listing_input: (&SearchInput, &SearchOptions, &str),
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    let request = listing_input
        .0
        .user_request(listing_input.1, listing_input.2)?;
    let plan = listing_input.1.plan()?;
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
                let request = request.clone();
                let plan = plan.clone();
                let committed = Arc::new(AtomicBool::new(false));
                let mut writer = crate::search::SearchWriter {
                    output: callback_output.clone(),
                    committed: committed.clone(),
                };
                let staged = staged.clone();
                let retained = retained.clone();
                Box::pin(async move {
                    let error = match attempt(&client, request, &plan, mode, &mut writer).await {
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

pub async fn user_search<T: Transport, W: Write>(
    client: &Client<T>,
    input: &SearchInput,
    options: &SearchOptions,
    word: &str,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let request = input.user_request(options, word)?;
    if let Some(mut spool) = attempt(client, request, &options.plan()?, mode, out).await? {
        spool.commit(out)?;
    }
    Ok(())
}

async fn attempt<T: Transport, W: Write>(
    client: &Client<T>,
    request: SearchUsersRequest,
    plan: &crate::search::SearchPlan,
    mode: DetailOutput,
    out: &mut W,
) -> Result<Option<JsonSpool>, CommandError> {
    let mut spool = if mode == DetailOutput::Json {
        Some(JsonSpool::with_key("user_previews")?)
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
        request.cursor.clone(),
        |cursor| {
            let mut request = request.clone();
            request.cursor = cursor;
            async move {
                let page = client
                    .search_users(request)
                    .await
                    .map_err(CommandError::from)?;
                Ok((page.items, page.next))
            }
        },
        |_: &pixiv_sdk::models::UserPreview| Ok(true),
        None::<fn(Cursor, usize) -> Result<Cursor, CommandError>>,
        |items| {
            if let Some(spool) = &mut spool {
                return spool.append_users(&items);
            }
            if mode == DetailOutput::Ndjson {
                for item in &items {
                    let record = pixiv_record::from_user_preview(item)
                        .map_err(|error| CommandError::Message(error.message()))?;
                    let encoded = serde_json::to_string(&record).map_err(std::io::Error::other)?;
                    writeln!(out, "{}", crate::go_json_escape(encoded))?;
                }
                return Ok(());
            }
            if !heading {
                writeln!(out, "users for {}", crate::search::quote(&request.word))?;
                heading = true;
            }
            for item in &items {
                let mut line = format!("{} {}", item.user.id, crate::safe_line(&item.user.name));
                if !item.user.account.is_empty() {
                    line.push_str(&format!(" (@{})", crate::safe_line(&item.user.account)));
                }
                if !item.user.comment.is_empty() {
                    line.push_str(&format!(" — {}", crate::safe_line(&item.user.comment)));
                }
                writeln!(out, "{line}")?;
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
