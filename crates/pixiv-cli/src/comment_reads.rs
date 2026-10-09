use crate::{CommandError, DetailOutput, user_works::UserWorksOptions};
use pixiv_app::{
    execution::Execution, facade::UseOutcome, lifecycle::Context, scheduler::SchedulerError,
};
use pixiv_sdk::{Client, cursor::Cursor, transport::Transport};
use std::{
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(clap::Args, Clone, Debug, Default)]
pub struct CommentOptions {
    #[command(flatten)]
    pub listing: UserWorksOptions,
    #[arg(long = "type", short = 't')]
    pub entity: Option<String>,
}
#[derive(clap::Args, Clone, Debug, Default)]
pub struct StampsOptions {
    #[arg(long,short='j',num_args=0..=1,require_equals=true,default_missing_value="true")]
    pub json: Option<bool>,
    #[arg(long,action=clap::ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false")]
    pub ndjson: bool,
    #[arg(num_args=0..)]
    pub sources: Vec<String>,
}
impl CommentOptions {
    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        if self.listing.sources.len() > 1 {
            return self.arguments();
        }
        crate::user_works::resolve_optional_source(&mut self.listing, input, terminal)?;
        self.arguments()
    }
    fn arguments(&self) -> Result<(), CommandError> {
        if self.listing.sources.len() != 1 {
            return Err(CommandError::Message("usage: pixiv comment [options] ID"));
        }
        Ok(())
    }
    fn target(&self) -> Result<(i64, String), CommandError> {
        self.arguments()?;
        let entity = self
            .entity
            .as_deref()
            .ok_or(CommandError::Message("--type is required for comment"))?;
        if !matches!(entity, "artwork" | "novel") {
            return Err(CommandError::Message("type must be one of artwork, novel"));
        }
        let value = self.listing.sources[0].trim();
        let invalid = |detail| {
            CommandError::Sdk(
                pixiv_sdk::Error::new(pixiv_sdk::Reason::InvalidArgument, "comment read")
                    .with_detail(detail),
            )
        };
        if value.is_empty() {
            return Err(invalid("input value is required"));
        }
        if let Ok(id) = value.parse::<i64>() {
            return if id > 0 {
                Ok((id, entity.into()))
            } else {
                Err(invalid("id must be a positive integer"))
            };
        }
        pixiv_sdk::reference::parse_url(value)
            .map_err(|_| invalid("input must be a positive ID or a supported Pixiv URL"))?;
        Err(invalid("URL kind is not allowed for this command"))
    }
    fn plan(&self) -> Result<crate::search::SearchPlan, CommandError> {
        crate::search::SearchOptions {
            limit: self.listing.limit,
            page: self.listing.page,
            ..Default::default()
        }
        .plan()
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        self.target()?;
        self.plan()?;
        Ok(())
    }
    pub fn output_mode(
        &self,
        configured: bool,
        terminal: bool,
    ) -> Result<DetailOutput, CommandError> {
        output_mode(self.listing.json, self.listing.ndjson, configured, terminal)
    }
}
impl StampsOptions {
    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if !self.sources.is_empty() {
            return Err(CommandError::Message(
                "usage: pixiv comment stamps [options]",
            ));
        }
        Ok(())
    }
    pub fn output_mode(
        &self,
        configured: bool,
        terminal: bool,
    ) -> Result<DetailOutput, CommandError> {
        output_mode(self.json, self.ndjson, configured, terminal)
    }
}
fn output_mode(
    json: Option<bool>,
    ndjson: bool,
    configured: bool,
    terminal: bool,
) -> Result<DetailOutput, CommandError> {
    if ndjson && json.is_some() {
        return Err(CommandError::Usage(
            "--ndjson cannot be used with --json".into(),
        ));
    }
    Ok(if ndjson {
        DetailOutput::Ndjson
    } else if json.unwrap_or(configured) {
        DetailOutput::Json
    } else if json.is_none() && !terminal {
        DetailOutput::Ndjson
    } else {
        DetailOutput::Human
    })
}
fn write_json<W: Write, V: serde::Serialize>(
    out: &mut W,
    value: &V,
    pretty: bool,
) -> Result<(), CommandError> {
    let body = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    }
    .map_err(std::io::Error::other)?;
    out.write_all(crate::go_json_escape(body).as_bytes())?;
    out.write_all(b"\n")?;
    Ok(())
}
async fn attempt<T: Transport, W: Write>(
    client: &Client<T>,
    options: &CommentOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<bool, CommandError> {
    let (id, entity) = options.target()?;
    let plan = options.plan()?;
    let metadata = Arc::new(Mutex::new((None, None)));
    let retained = metadata.clone();
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip as i64,
            limit: plan.limit as i64,
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| {
            let entity = entity.clone();
            let retained = retained.clone();
            async move {
                let page = if entity == "novel" {
                    client
                        .novel_comments(pixiv_sdk::NovelCommentsRequest {
                            novel_id: id,
                            cursor,
                        })
                        .await?
                } else {
                    client
                        .artwork_comments(pixiv_sdk::ArtworkCommentsRequest {
                            artwork_id: id,
                            cursor,
                        })
                        .await?
                };
                let mut metadata = retained.lock().unwrap_or_else(|e| e.into_inner());
                if metadata.0.is_none() {
                    metadata.0 = page.total;
                }
                if metadata.1.is_none() {
                    metadata.1 = page.access_control;
                }
                Ok::<_, CommandError>((page.items, page.next))
            }
        },
        |_: &pixiv_sdk::Comment| Ok(true),
        None::<fn(Cursor, usize) -> Result<Cursor, CommandError>>,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => CommandError::MessageText(message),
    })?;
    if mode == DetailOutput::Ndjson {
        for item in &page.items {
            write_json(out, &pixiv_sdk::to_comment_dto(item), false)?;
        }
        return Ok(!page.items.is_empty());
    }
    if mode == DetailOutput::Json {
        #[derive(serde::Serialize)]
        struct Output<'a> {
            comments: Vec<pixiv_sdk::CommentDto<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            total: Option<i64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            access_control: Option<pixiv_sdk::CommentAccessControlDto>,
        }
        let metadata = metadata.lock().unwrap_or_else(|e| e.into_inner());
        write_json(
            out,
            &Output {
                comments: page.items.iter().map(pixiv_sdk::to_comment_dto).collect(),
                total: metadata.0,
                access_control: metadata
                    .1
                    .as_ref()
                    .map(pixiv_sdk::to_comment_access_control_dto),
            },
            true,
        )?;
    } else {
        let _ = out.write(format!("{entity} comments for {id}\n").as_bytes())?;
        for item in &page.items {
            let _ =
                out.write(format!("{} {}: {}\n", item.id, item.user.name, item.body).as_bytes())?;
        }
    }
    Ok(true)
}
pub async fn comments<T: Transport, W: Write>(
    client: &Client<T>,
    options: &CommentOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    options.validate()?;
    attempt(client, options, mode, out).await.map(|_| ())
}
pub async fn saved_comments<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: CommentOptions,
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    options.validate()?;
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
                    let error = match attempt(&client, &options, mode, &mut writer).await {
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
fn stamp_output<W: Write>(
    items: &[pixiv_sdk::Stamp],
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    if mode == DetailOutput::Json {
        #[derive(serde::Serialize)]
        struct Output<'a> {
            stamps: Vec<pixiv_sdk::StampDto<'a>>,
        }
        write_json(
            out,
            &Output {
                stamps: items.iter().map(pixiv_sdk::to_stamp_dto).collect(),
            },
            true,
        )?;
    } else {
        for item in items {
            if mode == DetailOutput::Ndjson {
                write_json(out, &pixiv_sdk::to_stamp_dto(item), false)?;
            } else {
                let _ = out
                    .write(format!("{} {}\n", item.id, item.image.resource.reference).as_bytes())?;
            }
        }
    }
    Ok(())
}
pub async fn stamps<T: Transport, W: Write>(
    client: &Client<T>,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let items = client.stamps(pixiv_sdk::StampsRequest {}).await?;
    stamp_output(&items, mode, out)
}
pub async fn saved_stamps<T: Transport + 'static, W: Write>(
    execution: &Execution<T>,
    context: &Context,
    proxy: Option<&str>,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let items = execution
        .read(context, 0, proxy, move |_, client| async move {
            client
                .stamps(pixiv_sdk::StampsRequest {})
                .await
                .map_err(Into::into)
        })
        .await?;
    stamp_output(&items, mode, out)
}
