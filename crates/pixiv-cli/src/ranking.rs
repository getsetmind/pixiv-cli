use crate::{CommandError, DetailOutput, json_spool::JsonSpool};
use pixiv_app::{
    execution::Execution, facade::UseOutcome, lifecycle::Context, scheduler::SchedulerError,
};
use pixiv_sdk::{
    Client, cursor::Cursor, models::Artwork, pixiv::ArtworkRankingRequest, transport::Transport,
};
use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(clap::Args, Clone, Debug)]
pub struct RankingOptions {
    #[arg(long = "type", short = 't', default_value = "artwork")]
    pub entity: String,
    #[arg(long, default_value = "day")]
    pub mode: String,
    #[arg(long)]
    pub date: Option<String>,
    #[arg(long, short = 'l', allow_hyphen_values = true)]
    pub limit: Option<i64>,
    #[arg(long, short = 'p', allow_hyphen_values = true)]
    pub page: Option<i64>,
    #[arg(long,short='j',num_args=0..=1,require_equals=true,default_missing_value="true")]
    pub json: Option<bool>,
    #[arg(long,action=clap::ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false")]
    pub ndjson: bool,
    #[arg(num_args=0..)]
    pub query: Vec<String>,
}
impl RankingOptions {
    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if self.query.is_empty() {
            Ok(())
        } else {
            Err(CommandError::Message("usage: pixiv ranking [options]"))
        }
    }
    fn plan(&self) -> Result<crate::search::SearchPlan, CommandError> {
        self.validate_arguments()?;
        if !matches!(self.entity.as_str(), "artwork" | "novel") {
            return Err(CommandError::Message("type must be one of: artwork, novel"));
        }
        if self.entity == "novel" && self.date.is_some() {
            return Err(CommandError::Message(
                "--date is only supported when --type artwork",
            ));
        }
        if self.entity == "novel" {
            return Err(CommandError::Message(
                "novel ranking is not implemented yet",
            ));
        }
        crate::search::SearchOptions {
            limit: self.limit,
            page: self.page,
            ..Default::default()
        }
        .plan()
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        self.plan().map(|_| ())
    }
    pub fn output_mode(
        &self,
        configured_json: bool,
        terminal: bool,
    ) -> Result<DetailOutput, CommandError> {
        if self.ndjson && self.json.is_some() {
            return Err(CommandError::Usage(
                "--ndjson cannot be used with --json".into(),
            ));
        }
        Ok(if self.ndjson {
            DetailOutput::Ndjson
        } else if self.json.unwrap_or(configured_json) {
            DetailOutput::Json
        } else if self.json.is_none() && !terminal {
            DetailOutput::Ndjson
        } else {
            DetailOutput::Human
        })
    }
}

pub async fn artwork_ranking<T: Transport, W: Write>(
    client: &Client<T>,
    options: &RankingOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    if let Some(mut spool) = attempt(client, options, mode, out).await? {
        spool.commit(out)?;
    }
    Ok(())
}

pub async fn saved_artwork_ranking<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: RankingOptions,
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    options.validate()?;
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
                let options = options.clone();
                let committed = Arc::new(AtomicBool::new(false));
                let mut writer = crate::search::SearchWriter {
                    output: callback_output.clone(),
                    committed: committed.clone(),
                };
                let staged = staged.clone();
                let retained = retained.clone();
                Box::pin(async move {
                    let error = match attempt(&client, &options, mode, &mut writer).await {
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

async fn attempt<T: Transport, W: Write>(
    client: &Client<T>,
    options: &RankingOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<Option<JsonSpool>, CommandError> {
    let plan = options.plan()?;
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
            let request = ArtworkRankingRequest {
                mode: options.mode.clone(),
                date: options.date.clone().unwrap_or_default(),
                cursor,
            };
            async move {
                let page = client
                    .artwork_ranking(request)
                    .await
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
                writeln!(out, "{} ranking", options.mode)?;
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
