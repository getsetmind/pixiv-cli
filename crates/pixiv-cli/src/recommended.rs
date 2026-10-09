use crate::{CommandError, DetailOutput, json_spool::JsonSpool};
use pixiv_app::{
    execution::Execution, facade::UseOutcome, lifecycle::Context, scheduler::SchedulerError,
};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::{Artwork, Novel, UserPreview},
    transport::Transport,
};
use std::{
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(clap::Args, Clone, Debug, Default)]
pub struct RecommendedOptions {
    #[arg(num_args=0..)]
    pub query: Vec<String>,
    #[arg(long = "type", short = 't')]
    pub entity: Option<String>,
    #[arg(long)]
    pub content_type: Option<String>,
    #[arg(long, short = 'l', allow_hyphen_values = true)]
    pub limit: Option<i64>,
    #[arg(long, short = 'p', allow_hyphen_values = true)]
    pub page: Option<i64>,
    #[arg(long,short='j',num_args=0..=1,require_equals=true,default_missing_value="true")]
    pub json: Option<bool>,
    #[arg(long,action=clap::ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false")]
    pub ndjson: bool,
}
impl RecommendedOptions {
    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if self.query.len() > 1 {
            return Err(CommandError::Message(
                "usage: pixiv recommended [KIND] [options]",
            ));
        }
        Ok(())
    }
    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        self.validate_arguments()?;
        if self.query.is_empty() && !terminal {
            let mut bytes = Vec::new();
            input
                .read_to_end(&mut bytes)
                .map_err(|error| CommandError::Usage(format!("read stdin value: {error}")))?;
            if bytes.ends_with(b"\r\n") {
                bytes.truncate(bytes.len() - 2);
            } else if bytes.ends_with(b"\n") {
                bytes.pop();
            }
            if !bytes.is_empty() {
                self.query
                    .push(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        Ok(())
    }
    fn kind(&self) -> Result<&str, CommandError> {
        self.validate_arguments()?;
        if !self.query.is_empty() && self.entity.is_some() {
            return Err(CommandError::Usage(
                "KIND cannot be combined with --type".into(),
            ));
        }
        let kind = self
            .query
            .first()
            .map(String::as_str)
            .or(self.entity.as_deref())
            .unwrap_or("");
        if kind.is_empty() {
            return Err(CommandError::Message("recommended requires KIND or --type"));
        }
        if self.entity.is_some() && !matches!(kind, "artwork" | "novel" | "user" | "all") {
            return Err(CommandError::Usage(
                "type must be one of artwork, novel, user, all".into(),
            ));
        }
        if let Some(content) = self.content_type.as_deref() {
            if kind != "artwork" {
                return Err(CommandError::Usage(
                    "--content-type is only supported when --type artwork".into(),
                ));
            }
            if !matches!(content, "all" | "illust" | "manga") {
                return Err(CommandError::Usage(
                    "content-type must be one of all, illust, manga".into(),
                ));
            }
        }
        let kind = if kind == "artwork" {
            if self.content_type.as_deref() == Some("manga") {
                "manga"
            } else {
                "illust"
            }
        } else {
            kind
        };
        if !matches!(kind, "all" | "illust" | "manga" | "novel" | "user") {
            return Err(CommandError::Message(
                "recommendation kind must be one of: all, illust, manga, novel, user",
            ));
        }
        Ok(kind)
    }
    fn plan(&self) -> Result<crate::search::SearchPlan, CommandError> {
        self.kind()?;
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

pub async fn recommended<T: Transport, W: Write>(
    client: &Client<T>,
    options: &RecommendedOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    options.validate()?;
    if let Some(mut spool) = attempt(client, options, mode, out).await? {
        spool.commit(out)?;
    }
    Ok(())
}
pub async fn saved_recommended<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: RecommendedOptions,
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    options.validate()?;
    let output = Arc::new(Mutex::new(output));
    let callback_output = output.clone();
    let staged = Arc::new(Mutex::new(None));
    let retained = Arc::new(Mutex::new(None));
    let spool = staged.clone();
    let terminal = retained.clone();
    let result = execution
        .use_client(
            Some(context),
            0,
            proxy,
            Some(Arc::new(move |_, client| {
                let options = options.clone();
                let staged = staged.clone();
                let retained = retained.clone();
                let committed = Arc::new(AtomicBool::new(false));
                let mut writer = crate::search::SearchWriter {
                    output: callback_output.clone(),
                    committed: committed.clone(),
                };
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

#[derive(Clone, Copy)]
enum Feed {
    Artwork,
    Novel,
    User,
}
enum Item {
    Artwork(Box<Artwork>),
    Novel(Box<Novel>),
    User(Box<UserPreview>),
}
async fn pages<T: Transport, F: FnMut(Vec<Item>) -> Result<(), CommandError>>(
    client: &Client<T>,
    feed: Feed,
    plan: &crate::search::SearchPlan,
    consume: F,
) -> Result<(), CommandError> {
    pixiv_app::pagination::traverse_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip as i64,
            limit: plan.limit as i64,
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| async move {
            match feed {
                Feed::Artwork => {
                    let page = client
                        .recommended_artworks(pixiv_sdk::pixiv::RecommendedArtworksRequest {
                            cursor,
                        })
                        .await?;
                    Ok((
                        page.items
                            .into_iter()
                            .map(|v| Item::Artwork(Box::new(v)))
                            .collect(),
                        page.next,
                    ))
                }
                Feed::Novel => {
                    let page = client
                        .recommended_novels(pixiv_sdk::pixiv::RecommendedNovelsRequest { cursor })
                        .await?;
                    Ok((
                        page.items
                            .into_iter()
                            .map(|v| Item::Novel(Box::new(v)))
                            .collect(),
                        page.next,
                    ))
                }
                Feed::User => {
                    let page = client
                        .recommended_users(pixiv_sdk::pixiv::RecommendedUsersRequest { cursor })
                        .await?;
                    Ok((
                        page.items
                            .into_iter()
                            .map(|v| Item::User(Box::new(v)))
                            .collect(),
                        page.next,
                    ))
                }
            }
        },
        |_: &Item| Ok(true),
        None::<fn(Cursor, usize) -> Result<Cursor, CommandError>>,
        consume,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => CommandError::MessageText(message),
    })?;
    Ok(())
}
fn matches(item: &Item, filter: &str) -> bool {
    if let Item::Artwork(value) = item {
        filter == "all"
            || match value.kind {
                pixiv_sdk::models::ArtworkKind::Illust => filter == "illust",
                pixiv_sdk::models::ArtworkKind::Manga => filter == "manga",
                pixiv_sdk::models::ArtworkKind::Ugoira => filter == "ugoira",
                pixiv_sdk::models::ArtworkKind::Unknown => false,
            }
    } else {
        true
    }
}

fn human<W: Write>(item: &Item, out: &mut W) -> Result<(), CommandError> {
    match item {
        Item::Artwork(v) => {
            writeln!(out, "https://www.pixiv.net/artworks/{}", v.id)?;
            writeln!(
                out,
                "{} {} by {} bookmarks:{} views:{} tags:{}",
                v.id,
                crate::search::quote(&v.title),
                v.user.name,
                v.total_bookmarks,
                v.total_views,
                v.tags
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            )?;
        }
        Item::Novel(v) => writeln!(out, "{} {} — {}", v.id, v.title, v.user.name)?,
        Item::User(v) => writeln!(out, "{} {}", v.user.id, v.user.name)?,
    };
    Ok(())
}
fn ndjson<W: Write>(item: &Item, out: &mut W) -> Result<(), CommandError> {
    let record = match item {
        Item::Artwork(v) => pixiv_record::from_artwork(v),
        Item::Novel(v) => pixiv_record::from_novel(v),
        Item::User(v) => pixiv_record::from_user_preview(v),
    }
    .map_err(|e| CommandError::Message(e.message()))?;
    writeln!(
        out,
        "{}",
        crate::go_json_escape(serde_json::to_string(&record).map_err(std::io::Error::other)?)
    )?;
    Ok(())
}
fn compact(item: &Item, spool: &mut JsonSpool) -> Result<(), CommandError> {
    match item {
        Item::Artwork(v) => spool.append_compact(&pixiv_sdk::dto::ArtworkDto::from(v.as_ref())),
        Item::Novel(v) => spool.append_compact(&pixiv_sdk::dto::NovelDto::from(v.as_ref())),
        Item::User(v) => spool.append_compact(&pixiv_sdk::dto::UserPreviewDto::from(v.as_ref())),
    }
}
fn append(item: &Item, spool: &mut JsonSpool) -> Result<(), CommandError> {
    match item {
        Item::Artwork(v) => spool.append(std::slice::from_ref(v.as_ref())),
        Item::Novel(v) => spool.append_novels(std::slice::from_ref(v.as_ref())),
        Item::User(v) => spool.append_users(std::slice::from_ref(v.as_ref())),
    }
}
async fn attempt<T: Transport, W: Write>(
    client: &Client<T>,
    options: &RecommendedOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<Option<JsonSpool>, CommandError> {
    let kind = options.kind()?;
    let plan = options.plan()?;
    if kind == "all" {
        return all_attempt(client, &plan, mode, out).await.map(|_| None);
    }
    let (feed, key, heading) = match kind {
        "illust" => (Feed::Artwork, "illusts", "recommended illust"),
        "manga" => (Feed::Artwork, "manga", "recommended manga"),
        "novel" => (Feed::Novel, "novels", "recommended novels"),
        _ => (Feed::User, "user_previews", "recommended users"),
    };
    let filter = if kind == "manga" {
        "manga"
    } else {
        options.content_type.as_deref().unwrap_or("all")
    };
    let mut spool = if mode == DetailOutput::Json {
        Some(JsonSpool::with_key(key)?)
    } else {
        None
    };
    let mut heading_written = false;
    pages(client, feed, &plan, |items| {
        if mode == DetailOutput::Human && !heading_written {
            writeln!(out, "{heading}")?;
            heading_written = true;
        }
        for item in items.iter().filter(|item| matches(item, filter)) {
            if let Some(spool) = &mut spool {
                append(item, spool)?;
            } else if mode == DetailOutput::Ndjson {
                ndjson(item, out)?;
            } else {
                human(item, out)?;
            }
        }
        Ok(())
    })
    .await?;
    Ok(spool)
}
async fn all_attempt<T: Transport, W: Write>(
    client: &Client<T>,
    plan: &crate::search::SearchPlan,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let mut spool = if mode == DetailOutput::Ndjson {
        None
    } else {
        Some(JsonSpool::raw()?)
    };
    if mode == DetailOutput::Json {
        spool
            .as_mut()
            .expect("aggregate JSON spool exists")
            .write_all(b"{")?;
    }
    let mut visual = vec![];
    pages(client, Feed::Artwork, plan, |items| {
        visual.extend(items);
        Ok(())
    })
    .await?;
    for (index, (key, heading, filter)) in [
        ("illusts", "recommended illustrations", "illust"),
        ("manga", "recommended manga", "manga"),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(spool) = &mut spool {
            if mode == DetailOutput::Json {
                spool.section(key, index == 0)?;
            } else {
                writeln!(spool, "{heading}")?;
            }
        }
        for item in visual.iter().filter(|v| matches(v, filter)) {
            if let Some(spool) = &mut spool {
                if mode == DetailOutput::Json {
                    compact(item, spool)?;
                } else {
                    human(item, spool)?;
                }
            } else {
                ndjson(item, out)?;
            }
        }
    }
    for (feed, key, heading) in [
        (Feed::Novel, "novels", "recommended novels"),
        (Feed::User, "user_previews", "recommended users"),
    ] {
        if let Some(spool) = &mut spool {
            if mode == DetailOutput::Json {
                spool.section(key, false)?;
            } else {
                writeln!(spool, "{heading}")?;
            }
        }
        pages(client, feed, plan, |items| {
            for item in &items {
                if let Some(spool) = &mut spool {
                    if mode == DetailOutput::Json {
                        compact(item, spool)?;
                    } else {
                        human(item, spool)?;
                    }
                } else {
                    ndjson(item, out)?;
                }
            }
            Ok(())
        })
        .await?;
    }
    if let Some(mut spool) = spool {
        if mode == DetailOutput::Json {
            spool.write_all(b"\n  ]\n}\n")?;
        }
        spool.commit_raw(out)?;
    }
    Ok(())
}
