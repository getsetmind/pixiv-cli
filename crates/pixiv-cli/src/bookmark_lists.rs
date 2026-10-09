use crate::{CommandError, DetailOutput, user_works::UserWorksOptions};
use pixiv_app::{
    execution::Execution, facade::UseOutcome, lifecycle::Context, scheduler::SchedulerError,
};
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::{Artwork, Novel},
    transport::Transport,
};
use std::{
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
};

#[derive(clap::Args, Clone, Debug)]
pub struct BookmarkListOptions {
    #[command(flatten)]
    pub listing: UserWorksOptions,
    #[arg(long = "type", short = 't', default_value = "artwork")]
    pub entity: String,
    #[arg(long, default_value = "public")]
    pub restrict: String,
    #[arg(long, default_value = "")]
    pub tag: String,
    #[arg(skip)]
    pub input_record: Option<String>,
    #[arg(skip)]
    pub record_pending: bool,
}
impl Default for BookmarkListOptions {
    fn default() -> Self {
        Self {
            listing: UserWorksOptions::default(),
            entity: "artwork".into(),
            restrict: "public".into(),
            tag: String::new(),
            input_record: None,
            record_pending: false,
        }
    }
}
#[derive(clap::Args, Clone, Debug)]
pub struct UserBookmarksOptions {
    #[command(flatten)]
    pub listing: UserWorksOptions,
    #[arg(long, default_value = "public")]
    pub restrict: String,
    #[arg(long, default_value = "")]
    pub tag: String,
}
impl Default for UserBookmarksOptions {
    fn default() -> Self {
        Self {
            listing: UserWorksOptions::default(),
            restrict: "public".into(),
            tag: String::new(),
        }
    }
}
#[derive(Clone, Debug)]
pub enum BookmarkLists {
    List(BookmarkListOptions),
    User(UserBookmarksOptions),
}
impl BookmarkLists {
    pub fn options(&self) -> &UserWorksOptions {
        match self {
            Self::List(value) => &value.listing,
            Self::User(value) => &value.listing,
        }
    }
    fn entity(&self) -> &str {
        match self {
            Self::List(value) => &value.entity,
            Self::User(_) => "artwork",
        }
    }
    fn restrict(&self) -> &str {
        match self {
            Self::List(value) => &value.restrict,
            Self::User(value) => &value.restrict,
        }
    }
    fn tag(&self) -> &str {
        match self {
            Self::List(value) => &value.tag,
            Self::User(value) => &value.tag,
        }
    }
    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if self.options().sources.len() > 1 {
            return Err(CommandError::Message(match self {
                Self::List(_) => "usage: pixiv bookmark list [options] [USER_ID_OR_URL]",
                Self::User(_) => "usage: pixiv user bookmarks [options] [USER_ID]",
            }));
        }
        Ok(())
    }
    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        self.validate_arguments()?;
        match self {
            Self::User(value) => {
                crate::user_works::resolve_optional_source(&mut value.listing, input, terminal)
            }
            Self::List(value) => crate::record_input::resolve_source(
                &mut value.listing.sources,
                &mut value.input_record,
                &mut value.record_pending,
                input,
                terminal,
            ),
        }
    }
    pub fn resolve_target<R: Read>(&mut self, input: &mut R) -> Result<(), CommandError> {
        if let Self::List(_) = self {
            self.validate_arguments()?;
            if !matches!(self.entity(), "artwork" | "novel" | "all") {
                return Err(CommandError::Message(
                    "bookmark list type must be one of artwork, novel, all",
                ));
            }
            self.plan()?;
        }
        if let Self::List(value) = self {
            crate::record_input::read_pending(
                &mut value.input_record,
                &mut value.record_pending,
                input,
                "read bookmark target record",
            )?;
        }
        Ok(())
    }
    fn plan(&self) -> Result<crate::search::SearchPlan, CommandError> {
        crate::search::SearchOptions {
            limit: self.options().limit,
            page: self.options().page,
            ..Default::default()
        }
        .plan()
    }
    fn user_id(&self) -> Result<i64, CommandError> {
        match self {
            Self::User(value) => value.listing.sources.first().map_or(Ok(0), |value| {
                value
                    .parse::<i64>()
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or(CommandError::Message("user_id must be a positive integer"))
            }),
            Self::List(value) => {
                if let Some(record) = &value.input_record {
                    return record_user(record, &value.entity);
                }
                value
                    .listing
                    .sources
                    .first()
                    .map_or(Ok(0), |source| text_user(source, &value.entity))
            }
        }
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        self.validate_arguments()?;
        match self {
            Self::List(_) => {
                if !matches!(self.entity(), "artwork" | "novel" | "all") {
                    return Err(CommandError::Message(
                        "bookmark list type must be one of artwork, novel, all",
                    ));
                }
                self.plan()?;
                self.user_id()?;
            }
            Self::User(_) => {
                self.user_id()?;
                self.plan()?;
            }
        }
        Ok(())
    }
    pub fn output_mode(
        &self,
        configured_json: bool,
        terminal: bool,
    ) -> Result<DetailOutput, CommandError> {
        let options = self.options();
        if options.ndjson && options.json.is_some() {
            return Err(CommandError::Usage(
                "--ndjson cannot be used with --json".into(),
            ));
        }
        Ok(if options.ndjson {
            DetailOutput::Ndjson
        } else if options.json.unwrap_or(configured_json) {
            DetailOutput::Json
        } else if options.json.is_none() && !terminal {
            DetailOutput::Ndjson
        } else {
            DetailOutput::Human
        })
    }
    fn listing(&self) -> Result<Listing, CommandError> {
        self.validate()?;
        let id = self.user_id()?;
        let identity = Arc::new(AtomicI64::new(id));
        let plan = self.plan()?;
        Ok(match self.entity() {
            "all" => Listing::All(self.clone()),
            "novel" => Listing::Novel(crate::novel_list::Listing {
                source: crate::novel_list::Source::Bookmarks(
                    pixiv_sdk::pixiv::UserNovelBookmarksRequest {
                        user_id: id,
                        restrict: self.restrict().into(),
                        tag: self.tag().into(),
                        cursor: Cursor::default(),
                    },
                    identity,
                ),
                heading: format!("novel bookmarks by {id}"),
                plan,
            }),
            _ => Listing::Artwork(crate::artwork_list::Listing {
                source: crate::artwork_list::Source::Bookmarks(
                    pixiv_sdk::pixiv::UserArtworkBookmarksRequest {
                        user_id: id,
                        restrict: self.restrict().into(),
                        tag: self.tag().into(),
                        cursor: Cursor::default(),
                    },
                    identity,
                    matches!(self, Self::User(_)),
                ),
                heading: String::new(),
                plan,
            }),
        })
    }
}
pub(crate) fn current_target<T: Transport>(
    client: &Client<T>,
    identity: &AtomicI64,
    user_alias: bool,
) -> Result<i64, CommandError> {
    if user_alias {
        return crate::user_works::current_target(client, identity);
    }
    let mut id = identity.load(Ordering::Acquire);
    if id == 0 {
        id = client.user_id();
        if id <= 0 {
            return Err(CommandError::Message("cannot determine current user id"));
        }
        identity.store(id, Ordering::Release);
    }
    Ok(id)
}
fn text_user(value: &str, entity: &str) -> Result<i64, CommandError> {
    crate::record_input::bookmark_target(Some(value), None, entity, "bookmark list", false)
        .map(|value| value.0)
}
fn record_user(value: &str, entity: &str) -> Result<i64, CommandError> {
    crate::record_input::bookmark_target(None, Some(value), entity, "bookmark list", false)
        .map(|value| value.0)
}
enum Listing {
    Artwork(crate::artwork_list::Listing),
    Novel(crate::novel_list::Listing),
    All(BookmarkLists),
}

pub async fn bookmark_lists<T: Transport, W: Write>(
    client: &Client<T>,
    options: &BookmarkLists,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let spool = match options.listing()? {
        Listing::Artwork(value) => crate::artwork_list::attempt(client, &value, mode, out).await?,
        Listing::Novel(value) => crate::novel_list::attempt(client, &value, mode, out).await?,
        Listing::All(value) => {
            all_attempt(client, &value, mode, out).await?;
            None
        }
    };
    if let Some(mut spool) = spool {
        spool.commit(out)?;
    }
    Ok(())
}
pub async fn saved_bookmark_lists<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: BookmarkLists,
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    match options.listing()? {
        Listing::Artwork(value) => {
            crate::artwork_list::saved(execution, context, value, proxy, mode, output).await
        }
        Listing::Novel(value) => {
            crate::novel_list::saved(execution, context, value, proxy, mode, output).await
        }
        Listing::All(options) => {
            let output = Arc::new(Mutex::new(output));
            let terminal = Arc::new(Mutex::new(None));
            let retained = terminal.clone();
            let result = execution
                .use_client(
                    Some(context),
                    0,
                    proxy,
                    Some(Arc::new(move |_, client| {
                        let options = options.clone();
                        let retained = retained.clone();
                        let committed = Arc::new(AtomicBool::new(false));
                        let mut writer = crate::search::SearchWriter {
                            output: output.clone(),
                            committed: committed.clone(),
                        };
                        Box::pin(async move {
                            let error = match all_attempt(&client, &options, mode, &mut writer)
                                .await
                            {
                                Ok(()) => {
                                    committed.store(true, Ordering::Release);
                                    None
                                }
                                Err(CommandError::Sdk(error)) => Some(SchedulerError::from(error)),
                                Err(CommandError::App(error)) => Some(error),
                                Err(error) => {
                                    let message = error.to_string();
                                    *retained.lock().unwrap_or_else(|error| error.into_inner()) =
                                        Some(error);
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
            if let Some(error) = terminal
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                return Err(error);
            }
            result?;
            Ok(())
        }
    }
}
#[derive(Clone, Default)]
pub(crate) struct StreamCursor {
    pub(crate) upstream: Cursor,
    pub(crate) consumed: usize,
    text: String,
}
impl StreamCursor {
    pub(crate) fn upstream(upstream: Cursor) -> Self {
        let text = if upstream.is_zero() {
            String::new()
        } else {
            format!("{}#0", upstream.as_str())
        };
        Self {
            upstream,
            consumed: 0,
            text,
        }
    }
}
impl pixiv_app::pagination::Cursor for StreamCursor {
    fn is_zero(&self) -> bool {
        self.upstream.is_zero() && self.consumed == 0
    }
    fn text(&self) -> &str {
        &self.text
    }
}
enum Item {
    Artwork(Box<Artwork>),
    Novel(Box<Novel>),
}
impl Item {
    fn record(&self) -> Result<serde_json::Value, CommandError> {
        match self {
            Self::Artwork(value) => pixiv_record::from_artwork(value),
            Self::Novel(value) => pixiv_record::from_novel(value),
        }
        .map_err(|error| CommandError::Message(error.message()))
    }
    fn human<W: Write>(&self, out: &mut W) -> Result<(), CommandError> {
        match self {
            Self::Artwork(value) => {
                writeln!(out, "https://www.pixiv.net/artworks/{}", value.id)?;
                writeln!(
                    out,
                    "{} {} by {} bookmarks:{} views:{} tags:{}",
                    value.id,
                    crate::search::quote(&value.title),
                    value.user.name,
                    value.total_bookmarks,
                    value.total_views,
                    value
                        .tags
                        .iter()
                        .map(|tag| tag.name.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                )?;
            }
            Self::Novel(value) => {
                writeln!(out, "{} {} — {}", value.id, value.title, value.user.name)?
            }
        }
        Ok(())
    }
}
async fn all_attempt<T: Transport, W: Write>(
    client: &Client<T>,
    options: &BookmarkLists,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let requested = options.user_id()?;
    let id = current_target(client, &AtomicI64::new(requested), false)?;
    let plan = options.plan()?;
    let mut streams = (0..2)
        .map(|index| pixiv_app::pagination::Stream {
            fetch: move |cursor: StreamCursor| async move {
                let (items, next) = if index == 0 {
                    let page = client
                        .user_artwork_bookmarks(pixiv_sdk::pixiv::UserArtworkBookmarksRequest {
                            user_id: id,
                            restrict: options.restrict().into(),
                            tag: options.tag().into(),
                            cursor: cursor.upstream,
                        })
                        .await?;
                    (
                        page.items
                            .into_iter()
                            .map(|value| Item::Artwork(Box::new(value)))
                            .collect::<Vec<_>>(),
                        page.next,
                    )
                } else {
                    let page = client
                        .user_novel_bookmarks(pixiv_sdk::pixiv::UserNovelBookmarksRequest {
                            user_id: id,
                            restrict: options.restrict().into(),
                            tag: options.tag().into(),
                            cursor: cursor.upstream,
                        })
                        .await?;
                    (
                        page.items
                            .into_iter()
                            .map(|value| Item::Novel(Box::new(value)))
                            .collect::<Vec<_>>(),
                        page.next,
                    )
                };
                if cursor.consumed > items.len() {
                    return Err(CommandError::Message(if index == 0 {
                        "bookmark artwork checkpoint exceeds upstream batch"
                    } else {
                        "bookmark novel checkpoint exceeds upstream batch"
                    }));
                }
                Ok((
                    items.into_iter().skip(cursor.consumed).collect(),
                    StreamCursor::upstream(next),
                ))
            },
            include: |_: &Item| Ok(true),
            checkpoint,
        })
        .collect::<Vec<_>>();
    let page = pixiv_app::pagination::collect_streams(
        pixiv_app::pagination::Plan {
            skip: plan.skip as i64,
            limit: plan.limit as i64,
            one_batch: plan.one_batch,
        },
        &mut streams,
        pixiv_app::pagination::StreamState::default(),
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => CommandError::MessageText(message),
    })?;
    let mut staged = vec![];
    match mode {
        DetailOutput::Json => {
            let records = page
                .items
                .iter()
                .map(Item::record)
                .collect::<Result<Vec<_>, _>>()?;
            let body = serde_json::to_string_pretty(&serde_json::json!({"records":records}))
                .map_err(std::io::Error::other)?;
            writeln!(staged, "{}", crate::go_json_escape(body))?;
        }
        DetailOutput::Ndjson => {
            for item in &page.items {
                let body = serde_json::to_string(&item.record()?).map_err(std::io::Error::other)?;
                writeln!(staged, "{}", crate::go_json_escape(body))?;
            }
        }
        DetailOutput::Human => {
            for item in &page.items {
                item.human(&mut staged)?;
            }
        }
    }
    std::io::copy(&mut staged.as_slice(), out)?;
    Ok(())
}

pub(crate) fn checkpoint(
    cursor: StreamCursor,
    position: usize,
) -> Result<StreamCursor, CommandError> {
    if position == 0 {
        return Err(CommandError::Message(
            "bookmark checkpoint position must be positive",
        ));
    }
    let consumed = cursor.consumed + position;
    let text = format!("{}#{consumed}", cursor.upstream.as_str());
    Ok(StreamCursor {
        upstream: cursor.upstream,
        consumed,
        text,
    })
}
