use crate::{CommandError, DetailOutput, user_works::UserWorksOptions};
use pixiv_app::{
    execution::Execution, facade::UseOutcome, lifecycle::Context, scheduler::SchedulerError,
};
use pixiv_sdk::{Client, cursor::Cursor, transport::Transport};
use std::{
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
};

#[derive(clap::Args, Clone, Debug)]
pub struct BookmarkDetailOptions {
    #[arg(num_args=0..)]
    pub sources: Vec<String>,
    #[arg(long = "type", short = 't', default_value = "artwork")]
    pub entity: String,
    #[arg(long,short='j',num_args=0..=1,require_equals=true,default_missing_value="true")]
    pub json: Option<bool>,
    #[arg(skip)]
    pub input_record: Option<String>,
    #[arg(skip)]
    pub record_pending: bool,
}
impl Default for BookmarkDetailOptions {
    fn default() -> Self {
        Self {
            sources: vec![],
            entity: "artwork".into(),
            json: None,
            input_record: None,
            record_pending: false,
        }
    }
}
impl BookmarkDetailOptions {
    fn arguments(&self) -> Result<(), CommandError> {
        if self.sources.len() > 1 || (self.sources.is_empty() && self.input_record.is_none()) {
            return Err(CommandError::Message(
                "usage: pixiv bookmark detail [options] ARTWORK_ID_OR_NOVEL_ID_OR_URL",
            ));
        }
        Ok(())
    }
    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        crate::record_input::resolve_source(
            &mut self.sources,
            &mut self.input_record,
            &mut self.record_pending,
            input,
            terminal,
        )?;
        self.arguments()
    }
    pub fn resolve_target<R: Read>(&mut self, input: &mut R) -> Result<(), CommandError> {
        self.arguments()?;
        crate::record_input::read_pending(
            &mut self.input_record,
            &mut self.record_pending,
            input,
            "read bookmark detail record",
        )
    }
    fn target(&self) -> Result<(i64, String), CommandError> {
        self.arguments()?;
        crate::record_input::bookmark_target(
            self.sources.first().map(String::as_str),
            self.input_record.as_deref(),
            &self.entity,
            "bookmark detail",
            true,
        )
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        self.target().map(|_| ())
    }
    pub fn output_mode(
        &self,
        configured_json: bool,
        _terminal: bool,
    ) -> Result<DetailOutput, CommandError> {
        Ok(if self.json.unwrap_or(configured_json) {
            DetailOutput::Json
        } else {
            DetailOutput::Human
        })
    }
}
#[derive(clap::Args, Clone, Debug)]
pub struct BookmarkTagsOptions {
    #[command(flatten)]
    pub listing: UserWorksOptions,
    #[arg(long = "type", short = 't', default_value = "artwork")]
    pub entity: String,
    #[arg(long, default_value = "public")]
    pub restrict: String,
    #[arg(skip)]
    pub input_record: Option<String>,
    #[arg(skip)]
    pub record_pending: bool,
}
impl Default for BookmarkTagsOptions {
    fn default() -> Self {
        Self {
            listing: UserWorksOptions::default(),
            entity: "artwork".into(),
            restrict: "public".into(),
            input_record: None,
            record_pending: false,
        }
    }
}
impl BookmarkTagsOptions {
    fn arguments(&self) -> Result<(), CommandError> {
        if self.listing.sources.len() > 1 {
            return Err(CommandError::Message(
                "usage: pixiv bookmark tags [options] [USER_ID_OR_URL]",
            ));
        }
        Ok(())
    }
    fn plan(&self) -> Result<crate::search::SearchPlan, CommandError> {
        crate::search::SearchOptions {
            limit: self.listing.limit,
            page: self.listing.page,
            ..Default::default()
        }
        .plan()
    }
    fn kind(&self) -> Result<(), CommandError> {
        if !matches!(self.entity.as_str(), "artwork" | "novel" | "all") {
            return Err(CommandError::Message(
                "bookmark tags type must be one of artwork, novel, all",
            ));
        }
        Ok(())
    }
    fn user_id(&self) -> Result<i64, CommandError> {
        if self.input_record.is_none() && self.listing.sources.is_empty() {
            return Ok(0);
        }
        crate::record_input::bookmark_target(
            self.listing.sources.first().map(String::as_str),
            self.input_record.as_deref(),
            &self.entity,
            "bookmark tags",
            false,
        )
        .map(|v| v.0)
    }
    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        self.arguments()?;
        crate::record_input::resolve_source(
            &mut self.listing.sources,
            &mut self.input_record,
            &mut self.record_pending,
            input,
            terminal,
        )
    }
    pub fn resolve_target<R: Read>(&mut self, input: &mut R) -> Result<(), CommandError> {
        self.arguments()?;
        self.kind()?;
        self.plan()?;
        crate::record_input::read_pending(
            &mut self.input_record,
            &mut self.record_pending,
            input,
            "read bookmark target record",
        )
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        self.arguments()?;
        self.kind()?;
        self.plan()?;
        self.user_id()?;
        Ok(())
    }
    pub fn output_mode(
        &self,
        configured_json: bool,
        terminal: bool,
    ) -> Result<DetailOutput, CommandError> {
        crate::bookmark_lists::BookmarkLists::List(crate::bookmark_lists::BookmarkListOptions {
            listing: self.listing.clone(),
            ..Default::default()
        })
        .output_mode(configured_json, terminal)
    }
}
async fn detail_value<T: Transport>(
    client: &Client<T>,
    id: i64,
    entity: &str,
) -> Result<pixiv_sdk::models::ArtworkBookmarkDetail, pixiv_sdk::Error> {
    if entity == "novel" {
        client
            .novel_bookmark(pixiv_sdk::pixiv::NovelBookmarkRequest { novel_id: id })
            .await
    } else {
        client
            .artwork_bookmark(pixiv_sdk::pixiv::ArtworkBookmarkRequest { artwork_id: id })
            .await
    }
}
fn detail_output<W: Write>(
    value: &pixiv_sdk::models::ArtworkBookmarkDetail,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    if mode == DetailOutput::Json {
        let body =
            serde_json::to_string_pretty(&pixiv_sdk::dto::ArtworkBookmarkDetailDto::from(value))
                .map_err(std::io::Error::other)?;
        out.write_all(format!("{}\n", crate::go_json_escape(body)).as_bytes())?;
    } else if value.restrict.is_empty() {
        out.write_all(b"bookmarked: no\n")?;
    } else {
        out.write_all(format!("bookmarked: yes\nrestrict: {}\n", value.restrict).as_bytes())?;
        out.write_all(format!("tags: {}\n", value.tags.join(",")).as_bytes())?;
    }
    Ok(())
}
pub async fn bookmark_detail<T: Transport, W: Write>(
    client: &Client<T>,
    options: &BookmarkDetailOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let (id, entity) = options.target()?;
    let value = detail_value(client, id, &entity).await?;
    detail_output(&value, mode, out)
}
pub async fn saved_bookmark_detail<T: Transport + 'static, W: Write>(
    execution: &Execution<T>,
    context: &Context,
    options: BookmarkDetailOptions,
    proxy: Option<&str>,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let (id, entity) = options.target()?;
    let value = execution
        .read(context, 0, proxy, move |_, client| {
            let entity = entity.clone();
            async move { detail_value(&client, id, &entity).await.map_err(Into::into) }
        })
        .await?;
    detail_output(&value, mode, out)
}
async fn tag_page<T: Transport>(
    client: &Client<T>,
    id: i64,
    entity: &str,
    restrict: &str,
    cursor: Cursor,
) -> Result<pixiv_sdk::cursor::Page<pixiv_sdk::models::BookmarkTag>, CommandError> {
    let request = pixiv_sdk::pixiv::UserNovelBookmarkTagsRequest {
        user_id: id,
        restrict: restrict.into(),
        cursor,
    };
    Ok(if entity == "novel" {
        client.user_novel_bookmark_tags(request).await?
    } else {
        client.user_artwork_bookmark_tags(request).await?
    })
}
fn page_failure(error: pixiv_app::pagination::Failure<CommandError>) -> CommandError {
    match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => CommandError::MessageText(message),
    }
}
#[derive(serde::Serialize)]
struct TagItem {
    name: String,
    count: i64,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    entity: Option<&'static str>,
}
impl TagItem {
    fn from(value: pixiv_sdk::models::BookmarkTag, entity: Option<&'static str>) -> Self {
        Self {
            name: value.name,
            count: value.count,
            entity,
        }
    }
}
fn tag_output<W: Write>(
    items: &[TagItem],
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    if mode == DetailOutput::Json {
        #[derive(serde::Serialize)]
        struct Tags<'a> {
            bookmark_tags: &'a [TagItem],
        }
        let body = serde_json::to_string_pretty(&Tags {
            bookmark_tags: items,
        })
        .map_err(std::io::Error::other)?;
        out.write_all(format!("{}\n", crate::go_json_escape(body)).as_bytes())?;
    } else {
        for item in items {
            if mode == DetailOutput::Ndjson {
                let body = serde_json::to_string(item).map_err(std::io::Error::other)?;
                out.write_all(crate::go_json_escape(body).as_bytes())?;
                out.write_all(b"\n")?;
            } else {
                let prefix = item
                    .entity
                    .map(|kind| format!("{kind}: "))
                    .unwrap_or_default();
                out.write_all(format!("{prefix}{} ({})\n", item.name, item.count).as_bytes())?;
            }
        }
    }
    Ok(())
}
async fn tags_attempt<T: Transport, W: Write>(
    client: &Client<T>,
    options: &BookmarkTagsOptions,
    identity: &AtomicI64,
    mode: DetailOutput,
    out: &mut W,
) -> Result<bool, CommandError> {
    let plan = options.plan()?;
    let plan = pixiv_app::pagination::Plan {
        skip: plan.skip as i64,
        limit: plan.limit as i64,
        one_batch: plan.one_batch,
    };
    if options.entity != "all" {
        let page = pixiv_app::pagination::collect_pages(
            plan,
            Cursor::default(),
            |cursor| async move {
                let id = crate::bookmark_lists::current_target(client, identity, false)?;
                let page = tag_page(client, id, &options.entity, &options.restrict, cursor).await?;
                Ok((page.items, page.next))
            },
            |_: &pixiv_sdk::models::BookmarkTag| Ok(true),
            None::<fn(Cursor, usize) -> Result<Cursor, CommandError>>,
        )
        .await
        .map_err(page_failure)?;
        let items = page
            .items
            .into_iter()
            .map(|tag| TagItem::from(tag, None))
            .collect::<Vec<_>>();
        tag_output(&items, mode, out)?;
        return Ok(mode != DetailOutput::Ndjson || !items.is_empty());
    }
    let id =
        crate::bookmark_lists::current_target(client, &AtomicI64::new(options.user_id()?), false)?;
    let mut streams = (0..2)
        .map(|index| pixiv_app::pagination::Stream {
            fetch: move |cursor: crate::bookmark_lists::StreamCursor| async move {
                let entity = if index == 0 { "artwork" } else { "novel" };
                let page = tag_page(client, id, entity, &options.restrict, cursor.upstream).await?;
                if cursor.consumed > page.items.len() {
                    return Err(CommandError::Message(if index == 0 {
                        "bookmark artwork tag checkpoint exceeds upstream batch"
                    } else {
                        "bookmark novel tag checkpoint exceeds upstream batch"
                    }));
                }
                Ok((
                    page.items
                        .into_iter()
                        .skip(cursor.consumed)
                        .map(|tag| TagItem::from(tag, Some(entity)))
                        .collect(),
                    crate::bookmark_lists::StreamCursor::upstream(page.next),
                ))
            },
            include: |_: &TagItem| Ok(true),
            checkpoint: crate::bookmark_lists::checkpoint,
        })
        .collect::<Vec<_>>();
    let page = pixiv_app::pagination::collect_streams(
        plan,
        &mut streams,
        pixiv_app::pagination::StreamState::default(),
    )
    .await
    .map_err(page_failure)?;
    let mut staged = vec![];
    tag_output(&page.items, mode, &mut staged)?;
    std::io::copy(&mut staged.as_slice(), out)?;
    Ok(true)
}
pub async fn bookmark_tags<T: Transport, W: Write>(
    client: &Client<T>,
    options: &BookmarkTagsOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    options.validate()?;
    tags_attempt(
        client,
        options,
        &AtomicI64::new(options.user_id()?),
        mode,
        out,
    )
    .await
    .map(|_| ())
}
pub async fn saved_bookmark_tags<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: BookmarkTagsOptions,
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    options.validate()?;
    let identity = Arc::new(AtomicI64::new(options.user_id()?));
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
                let identity = identity.clone();
                let retained = retained.clone();
                let committed = Arc::new(AtomicBool::new(false));
                let mut writer = crate::search::SearchWriter {
                    output: output.clone(),
                    committed: committed.clone(),
                };
                Box::pin(async move {
                    let error =
                        match tags_attempt(&client, &options, &identity, mode, &mut writer).await {
                            Ok(value) => {
                                committed.store(value, Ordering::Release);
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
    Ok(())
}
