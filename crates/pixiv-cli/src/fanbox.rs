mod help;
use crate::{CommandError, DetailOutput};
pub(crate) use help::download_flag_args;
pub use help::{command_route, help_route, is_fanbox_route};
use std::io::Read;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadKind {
    Creators,
    Posts,
    Tags,
    Home,
    Supporting,
    Post,
}
#[derive(Clone, Debug)]
pub struct ReadCommand {
    pub kind: ReadKind,
    pub sources: Vec<String>,
    pub creator_kind: String,
    pub json: Option<bool>,
    pub ndjson: bool,
    pub limit: Option<i64>,
    pub page: Option<i64>,
    pub proxy: Option<String>,
    pub no_proxy: Option<bool>,
}
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub limit: i64,
    pub skip: i64,
    pub one_batch: bool,
}
impl ReadCommand {
    pub fn parse(args: &[String]) -> Result<Self, CommandError> {
        let normalized = if command_route(args)
            .path
            .first()
            .is_some_and(|part| part == "fanbox")
        {
            args.to_vec()
        } else {
            std::iter::once("fanbox".to_owned())
                .chain(args.iter().cloned())
                .collect()
        };
        let route = command_route(&normalized);
        let leaf = route.path.last().map(String::as_str).unwrap_or("fanbox");
        let args = &route.args;
        let kind = match leaf {
            "creators" => ReadKind::Creators,
            "posts" => ReadKind::Posts,
            "tags" => ReadKind::Tags,
            "home" => ReadKind::Home,
            "supporting" => ReadKind::Supporting,
            "post" => ReadKind::Post,
            _ => {
                return Err(CommandError::MessageText(format!(
                    "unknown command \"{leaf}\" for \"pixiv fanbox\""
                )));
            }
        };
        let mut command = Self {
            kind,
            sources: vec![],
            creator_kind: "supporting".into(),
            json: None,
            ndjson: false,
            limit: None,
            page: None,
            proxy: None,
            no_proxy: None,
        };
        let mut index = 0;
        let mut positional = false;
        while index < args.len() {
            let argument = &args[index];
            index += 1;
            if argument == "--" && !positional {
                positional = true;
                continue;
            }
            if positional || !argument.starts_with('-') || argument == "-" {
                command.sources.push(argument.clone());
                continue;
            }
            let (flag, attached) = argument
                .split_once('=')
                .map_or((argument.as_str(), None), |(flag, value)| {
                    (flag, Some(value))
                });
            let boolean = matches!(flag, "--json" | "--ndjson" | "--no-proxy");
            let known = boolean || matches!(flag, "--proxy" | "--limit" | "--page" | "--kind");
            let supported = known
                && (!matches!(flag, "--limit" | "--page") || command.is_list())
                && (flag != "--kind" || kind == ReadKind::Creators);
            if !supported {
                return Err(CommandError::Usage(format!("unknown option '{flag}'")));
            }
            let value = if boolean {
                attached.unwrap_or("true")
            } else if let Some(value) = attached {
                value
            } else {
                let value = args.get(index).ok_or_else(|| {
                    CommandError::MessageText(format!("flag needs an argument: {flag}"))
                })?;
                index += 1;
                value
            };
            match flag {
                "--json" => command.json = Some(parse_bool(value, flag)?),
                "--ndjson" => command.ndjson = parse_bool(value, flag)?,
                "--no-proxy" => command.no_proxy = Some(parse_bool(value, flag)?),
                "--proxy" => command.proxy = Some(value.into()),
                "--kind" => command.creator_kind = value.into(),
                "--limit" => command.limit = Some(parse_integer(value, flag)?),
                "--page" => command.page = Some(parse_integer(value, flag)?),
                _ => unreachable!(),
            }
        }
        Ok(command)
    }
    pub fn is_list(&self) -> bool {
        !matches!(self.kind, ReadKind::Post | ReadKind::Tags)
    }
    pub fn usage(&self) -> &'static str {
        match self.kind {
            ReadKind::Creators => "usage: pixiv fanbox creators --kind supporting|following",
            ReadKind::Posts => "usage: pixiv fanbox posts SOURCE",
            ReadKind::Tags => "usage: pixiv fanbox tags CREATOR",
            ReadKind::Home => "usage: pixiv fanbox home",
            ReadKind::Supporting => "usage: pixiv fanbox supporting",
            ReadKind::Post => "usage: pixiv fanbox post POST_ID",
        }
    }
    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        let requires_source =
            matches!(self.kind, ReadKind::Posts | ReadKind::Tags | ReadKind::Post);
        if self.sources.len() > usize::from(requires_source) {
            return Err(CommandError::Message(self.usage()));
        }
        if requires_source && self.sources.is_empty() && !terminal {
            let mut value = String::new();
            input
                .read_to_string(&mut value)
                .map_err(|e| CommandError::Usage(format!("read stdin value: {e}")))?;
            let value = value
                .strip_suffix("\r\n")
                .or_else(|| value.strip_suffix('\n'))
                .or_else(|| value.strip_suffix('\r'))
                .unwrap_or(&value);
            if !value.is_empty() {
                self.sources.push(value.to_owned());
            }
        }
        if requires_source && self.sources.is_empty() {
            return Err(CommandError::Message(self.usage()));
        }
        Ok(())
    }
    pub fn plan(&self) -> Result<Plan, CommandError> {
        let limit = self.limit.unwrap_or(0);
        if limit < 0 {
            return Err(CommandError::Message(
                "limit must be zero or a positive integer",
            ));
        }
        if self.page.is_some_and(|page| page <= 0) {
            return Err(CommandError::Message("page must be a positive integer"));
        }
        let page = self.page.unwrap_or(0);
        if page > 0 && limit <= 0 {
            return Err(CommandError::Message(
                "--page requires --limit to be a positive integer",
            ));
        }
        let skip = if page > 0 {
            (page - 1).checked_mul(limit).ok_or(CommandError::Message(
                "page and limit overflow the logical result offset",
            ))?
        } else {
            0
        };
        Ok(Plan {
            limit,
            skip,
            one_batch: self.limit.is_none() && page == 0,
        })
    }
    pub fn output_mode(&self) -> Result<DetailOutput, CommandError> {
        if self.ndjson && self.json.is_some() {
            return Err(CommandError::Usage(
                "--ndjson cannot be used with --json".into(),
            ));
        }
        Ok(if self.ndjson {
            DetailOutput::Ndjson
        } else if self.json == Some(true) {
            DetailOutput::Json
        } else {
            DetailOutput::Human
        })
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        if self.kind == ReadKind::Creators
            && !matches!(self.creator_kind.as_str(), "supporting" | "following")
        {
            return Err(CommandError::Message(
                "kind must be one of: supporting, following",
            ));
        }
        if self.is_list() {
            self.plan()?;
        }
        self.output_mode()?;
        Ok(())
    }
    pub fn proxy_override(&self) -> Result<Option<&str>, CommandError> {
        if self.proxy.is_some() && self.no_proxy.is_some() {
            return Err(CommandError::Message(
                "use either --proxy or --no-proxy, not both",
            ));
        }
        Ok(if self.no_proxy == Some(true) {
            Some("")
        } else {
            self.proxy.as_deref()
        })
    }
}
pub(crate) fn parse_bool(value: &str, flag: &str) -> Result<bool, CommandError> {
    match value {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(CommandError::MessageText(format!(
            "invalid argument {value:?} for {flag:?} flag: strconv.ParseBool: parsing {value:?}: invalid syntax"
        ))),
    }
}
fn parse_integer(value: &str, flag: &str) -> Result<i64, CommandError> {
    value.parse().map_err(|error: std::num::ParseIntError| {
        CommandError::MessageText(format!(
            "invalid argument {value:?} for {flag:?} flag: strconv.ParseInt: parsing {value:?}: {}",
            if matches!(
                error.kind(),
                std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow
            ) {
                "value out of range"
            } else {
                "invalid syntax"
            }
        ))
    })
}

use pixiv_app::{
    fanbox_facade::{Facade, OpenRequest},
    lifecycle::{Context, Lease},
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    cursor::{Cursor, Page},
    fanbox::{
        Client, CreatorPostsRequest, CreatorSummary, CreatorTagsRequest, CreatorsRequest,
        HomeRequest, Post, PostRequest, ReferenceKind, ResolveURLRequest, SupportingRequest,
        TaggedPostsRequest,
    },
};
use serde::Serialize;
use std::{future::Future, io::Write, pin::Pin, sync::Arc};

impl ReadCommand {
    pub async fn execute<S, W: Write>(
        &self,
        context: &Context,
        service: S,
        output: &mut W,
    ) -> Result<(), CommandError>
    where
        S: FnOnce() -> Result<Option<Arc<Facade>>, CommandError>,
    {
        self.validate()?;
        let facade = service()?.ok_or(CommandError::Message(
            "fanbox is not available: cannot open the local account store",
        ))?;
        let request = OpenRequest {
            proxy_override: self.proxy_override()?.map(str::to_owned),
        };
        let lease = OwnedLease(
            facade
                .open(Some(context), request)
                .await
                .map_err(CommandError::App)?,
        );
        let result = self
            .execute_with_client(context, lease.0.value(), output)
            .await;
        let closed = lease
            .0
            .close()
            .map_err(|error| CommandError::App(SchedulerError::Shared(error)));
        match (result, closed) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(closed)) => Err(CommandError::Joined(vec![error, closed])),
        }
    }
    pub async fn execute_with_client<W: Write>(
        &self,
        context: &Context,
        client: &Client,
        output: &mut W,
    ) -> Result<(), CommandError> {
        self.validate()?;
        let mode = self.output_mode()?;
        match self.kind {
            ReadKind::Post => {
                let post = client
                    .post(
                        Arc::new(context.clone()),
                        PostRequest {
                            post_id: self.source()?.to_owned(),
                        },
                    )
                    .await?;
                match mode {
                    DetailOutput::Human => print_post(output, &post),
                    DetailOutput::Json => write_json(output, &post_summary(&post), true)?,
                    DetailOutput::Ndjson => write_json(output, &post_summary(&post), false)?,
                }
                Ok(())
            }
            ReadKind::Tags => {
                let tags = client
                    .creator_tags(
                        Arc::new(context.clone()),
                        CreatorTagsRequest {
                            creator_id: self.source()?.to_owned(),
                        },
                    )
                    .await?;
                let tags = tags
                    .iter()
                    .map(|tag| TagOut {
                        name: tag.name.clone(),
                        url: tag.url.clone(),
                    })
                    .collect::<Vec<_>>();
                match mode {
                    DetailOutput::Human => {
                        for tag in tags {
                            let _ = output.write(format!("tag:{}\n", tag.name).as_bytes());
                        }
                    }
                    DetailOutput::Json => write_json(output, &TagsOut { tags }, true)?,
                    DetailOutput::Ndjson => {
                        for tag in tags {
                            write_json(output, &tag, false)?;
                        }
                    }
                }
                Ok(())
            }
            ReadKind::Creators => {
                let kind = self.creator_kind.clone();
                run_list(
                    context,
                    output,
                    self.plan()?,
                    ListOutput {
                        mode,
                        key: "creators",
                    },
                    |cursor| {
                        let kind = kind.clone();
                        Box::pin(
                            client.creators(
                                Arc::new(context.clone()),
                                CreatorsRequest { kind, cursor },
                            ),
                        )
                    },
                    CreatorSummary::to_dto,
                    print_creator,
                )
                .await
            }
            _ => {
                let source = match self.kind {
                    ReadKind::Home => PostSource::Home,
                    ReadKind::Supporting => PostSource::Supporting,
                    _ => resolve_posts(client, context, self.source()?).await?,
                };
                run_list(
                    context,
                    output,
                    self.plan()?,
                    ListOutput { mode, key: "posts" },
                    |cursor| Box::pin(fetch_posts(client, context, &source, cursor)),
                    Post::to_dto,
                    print_post,
                )
                .await
            }
        }
    }
    fn source(&self) -> Result<&str, CommandError> {
        self.sources
            .first()
            .map(String::as_str)
            .ok_or(CommandError::Message(self.usage()))
    }
}
pub(crate) struct OwnedLease(pub(crate) Lease<Arc<Client>, SchedulerError>);
impl Drop for OwnedLease {
    fn drop(&mut self) {
        let _ = self.0.close();
    }
}
#[derive(Serialize)]
struct TagOut {
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    url: String,
}
#[derive(Serialize)]
struct TagsOut {
    tags: Vec<TagOut>,
}
#[derive(Serialize)]
struct PostOut {
    id: String,
    title: String,
    published_at: String,
    creator_id: String,
    #[serde(skip_serializing_if = "is_zero")]
    fee_required: i64,
    is_restricted: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    is_pinned: bool,
    #[serde(skip_serializing_if = "is_zero")]
    comment_count: i64,
}
fn is_zero(value: &i64) -> bool {
    *value == 0
}
fn post_summary(post: &Post) -> PostOut {
    PostOut {
        id: post.id.clone(),
        title: post.title.clone(),
        published_at: if post.published_at.timestamp() == -62135596800
            && post.published_at.timestamp_subsec_nanos() == 0
        {
            String::new()
        } else {
            post.published_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        },
        creator_id: post.creator_id.clone(),
        fee_required: post.fee_required,
        is_restricted: post.is_restricted,
        is_pinned: post.is_pinned,
        comment_count: post.comment_count,
    }
}
fn print_post<W: Write>(output: &mut W, post: &Post) {
    let _ = output.write(
        format!(
            "id:{} title:{} published:{} restricted:{}\n",
            post.id,
            post.title,
            post_summary(post).published_at,
            if post.is_restricted { "yes" } else { "no" }
        )
        .as_bytes(),
    );
}
fn print_creator<W: Write>(output: &mut W, creator: &CreatorSummary) {
    let _ = output.write(format!("id:{}", creator.id).as_bytes());
    if !creator.name.is_empty() {
        let _ = output.write(format!(" name:{}", creator.name).as_bytes());
    }
    let _ = output.write(b"\n");
}
fn write_json<W: Write, T: Serialize>(
    output: &mut W,
    value: &T,
    pretty: bool,
) -> Result<(), CommandError> {
    let encoded = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    }
    .map_err(std::io::Error::other)?;
    let encoded = crate::go_json_escape(encoded);
    let _written = output.write(format!("{encoded}\n").as_bytes())?;
    Ok(())
}

pub(crate) enum PostSource {
    Home,
    Supporting,
    Creator(String),
    Tag(String, String),
    Single(String),
}
pub(crate) async fn resolve_posts(
    client: &Client,
    context: &Context,
    source: &str,
) -> Result<PostSource, CommandError> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let reference = client.resolve_url(
            Arc::new(context.clone()),
            ResolveURLRequest {
                raw_url: source.into(),
            },
        )?;
        Ok(match reference.kind {
            ReferenceKind::Post => PostSource::Single(reference.post_id),
            ReferenceKind::Tag => PostSource::Tag(reference.creator_id, reference.tag),
            _ => PostSource::Creator(reference.creator_id),
        })
    } else if !source.is_empty() && source.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(PostSource::Single(source.into()))
    } else {
        Ok(PostSource::Creator(source.into()))
    }
}
pub(crate) async fn fetch_posts(
    client: &Client,
    context: &Context,
    source: &PostSource,
    cursor: Cursor,
) -> pixiv_sdk::Result<Page<Post>> {
    let context = Arc::new(context.clone());
    match source {
        PostSource::Home => client.home(context, HomeRequest { cursor }).await,
        PostSource::Supporting => {
            client
                .supporting(context, SupportingRequest { cursor })
                .await
        }
        PostSource::Creator(creator_id) => {
            client
                .creator_posts(
                    context,
                    CreatorPostsRequest {
                        creator_id: creator_id.clone(),
                        cursor,
                    },
                )
                .await
        }
        PostSource::Tag(creator_id, tag) => {
            client
                .tagged_posts(
                    context,
                    TaggedPostsRequest {
                        creator_id: creator_id.clone(),
                        tag: tag.clone(),
                        cursor,
                    },
                )
                .await
        }
        PostSource::Single(post_id) => {
            if cursor.is_zero() {
                client
                    .post(
                        context,
                        PostRequest {
                            post_id: post_id.clone(),
                        },
                    )
                    .await
                    .map(|post| Page {
                        items: vec![post],
                        next: Cursor::default(),
                    })
            } else {
                Ok(Page::default())
            }
        }
    }
}
type FetchFuture<'a, T> = Pin<Box<dyn Future<Output = pixiv_sdk::Result<Page<T>>> + 'a>>;
struct ListOutput<'a> {
    mode: DetailOutput,
    key: &'a str,
}

async fn run_list<'a, T, D, W, F, M, P>(
    context: &Context,
    output: &mut W,
    plan: Plan,
    format: ListOutput<'_>,
    mut fetch: F,
    map: M,
    print: P,
) -> Result<(), CommandError>
where
    W: Write,
    D: Serialize,
    F: FnMut(Cursor) -> FetchFuture<'a, T>,
    M: Fn(&T) -> D,
    P: Fn(&mut W, &T),
{
    let ListOutput { mode, key } = format;
    let mut spool = if mode == DetailOutput::Json {
        Some(crate::json_spool::JsonSpool::with_key(key)?)
    } else {
        None
    };
    let mut cursor = Cursor::default();
    let mut seen = std::collections::HashSet::new();
    let mut returned = 0_i64;
    let mut skip = plan.skip;
    loop {
        if let Some(error) = context.error() {
            return Err(CommandError::State(Box::new(error)));
        }
        if !seen.insert(cursor.as_str().to_owned()) {
            return Err(CommandError::Message("pagination cursor repeated"));
        }
        let page = fetch(cursor).await?;
        let start = usize::try_from(skip.min(page.items.len() as i64)).unwrap_or(0);
        skip -= start as i64;
        let mut items = &page.items[start..];
        if plan.limit > 0 {
            items = &items[..items.len().min((plan.limit - returned) as usize)];
        }
        match mode {
            DetailOutput::Human => {
                for item in items {
                    print(output, item);
                }
            }
            DetailOutput::Ndjson => {
                for item in items {
                    write_json(output, &map(item), false)?;
                }
            }
            DetailOutput::Json => spool
                .as_mut()
                .expect("JSON output owns a spool")
                .append_dtos(items.iter().map(&map))?,
        }
        returned += items.len() as i64;
        if plan.limit > 0 && returned >= plan.limit
            || plan.one_batch && (returned > 0 || page.next.is_zero())
            || page.next.is_zero()
        {
            break;
        }
        cursor = page.next;
    }
    if let Some(mut spool) = spool {
        spool.commit_fanbox(output)?;
    }
    Ok(())
}

pub fn prepare_root<R: Read, W: std::io::Write, C>(
    context: &pixiv_app::lifecycle::Context,
    args: &[String],
    input: &mut R,
    terminal: bool,
    hooks: &dyn crate::startup::StartupHooks,
    diagnostics: &mut W,
    runtime: C,
) -> Result<ReadCommand, CommandError>
where
    C: FnOnce() -> Result<(), CommandError>,
{
    let mut command = ReadCommand::parse(args)?;
    command.resolve_source(input, terminal)?;
    crate::startup::run_startup(context, hooks, diagnostics)?;
    runtime()?;
    Ok(command)
}
