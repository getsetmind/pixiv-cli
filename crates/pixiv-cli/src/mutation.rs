use crate::CommandError;
use clap::{Args, Subcommand};
use pixiv_app::{execution::Execution, lifecycle::Context};
use pixiv_sdk::{Client, Error, Reason, pixiv::*, transport::Transport};
use std::io::{BufRead, Write};

#[derive(Clone, Debug, Args)]
pub struct ActionInput {
    pub source: Option<String>,
    #[arg(long, default_value = "skip")]
    pub on_error: String,
    #[arg(long)]
    pub proxy: Option<String>,
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    pub no_proxy: Option<bool>,
}
#[derive(Clone, Debug, Args)]
pub struct BookmarkInput {
    #[command(flatten)]
    pub input: ActionInput,
    #[arg(long = "type", short = 't', default_value = "artwork")]
    pub entity: String,
}
#[derive(Clone, Debug, Subcommand)]
pub enum BookmarkCommand {
    #[command(args_override_self = true)]
    Add {
        #[command(flatten)]
        input: BookmarkInput,
        #[arg(long, default_value = "public")]
        restrict: String,
        #[arg(long = "tag", action = clap::ArgAction::Append)]
        tags: Vec<String>,
    },
    #[command(args_override_self = true)]
    Remove {
        #[command(flatten)]
        input: BookmarkInput,
    },
}
#[derive(Clone, Debug, Subcommand)]
pub enum FollowCommand {
    #[command(args_override_self = true)]
    Add {
        #[command(flatten)]
        input: ActionInput,
        #[arg(long, default_value = "public")]
        restrict: String,
    },
    #[command(args_override_self = true)]
    Remove {
        #[command(flatten)]
        input: ActionInput,
    },
}
#[derive(Clone, Debug)]
pub enum Mutation {
    Bookmark(BookmarkCommand),
    Follow(FollowCommand),
}
impl Mutation {
    fn input(&self) -> &ActionInput {
        match self {
            Self::Bookmark(
                BookmarkCommand::Add { input, .. } | BookmarkCommand::Remove { input },
            ) => &input.input,
            Self::Follow(FollowCommand::Add { input, .. } | FollowCommand::Remove { input }) => {
                input
            }
        }
    }
    pub fn proxy_override(&self) -> Result<Option<&str>, CommandError> {
        let input = self.input();
        if input.proxy.is_some() && input.no_proxy.is_some() {
            return Err(CommandError::Message(
                "use either --proxy or --no-proxy, not both",
            ));
        }
        if input.no_proxy == Some(true) {
            Ok(Some(""))
        } else {
            Ok(input.proxy.as_deref())
        }
    }
    fn operation(&self) -> &'static str {
        match self {
            Self::Bookmark(BookmarkCommand::Add { .. }) => "bookmark_add",
            Self::Bookmark(BookmarkCommand::Remove { .. }) => "bookmark_remove",
            Self::Follow(FollowCommand::Add { .. }) => "follow_add",
            Self::Follow(FollowCommand::Remove { .. }) => "follow_remove",
        }
    }
    fn accepts(&self, typ: &str) -> bool {
        match self {
            Self::Bookmark(
                BookmarkCommand::Add { input, .. } | BookmarkCommand::Remove { input },
            ) if input.entity == "novel" => typ == "novel",
            Self::Bookmark(_) => matches!(typ, "artwork" | "illust" | "manga" | "ugoira"),
            Self::Follow(_) => typ == "user",
        }
    }
    pub fn validate(&self, terminal: bool) -> Result<(), CommandError> {
        if self.input().source.is_none() && terminal {
            return Err(CommandError::Usage(format!(
                "usage: pixiv {} [options] [{}]",
                self.operation().replace('_', " "),
                if matches!(self, Self::Bookmark(_)) {
                    "ARTWORK_ID_OR_NOVEL_ID"
                } else {
                    "USER_ID"
                }
            )));
        }
        if let Self::Bookmark(
            BookmarkCommand::Add { input, .. } | BookmarkCommand::Remove { input },
        ) = self
            && !matches!(input.entity.as_str(), "artwork" | "novel")
        {
            return Err(CommandError::Usage(format!(
                "type {:?} is not supported by this command",
                input.entity
            )));
        }
        if let Self::Follow(FollowCommand::Add { restrict, .. }) = self
            && !matches!(restrict.as_str(), "" | "public" | "private")
        {
            return Err(Error::new(Reason::InvalidArgument, "follow add")
                .with_detail("restrict must be public or private")
                .into());
        }
        if !matches!(self.input().on_error.as_str(), "skip" | "fail-fast") {
            return Err(CommandError::Usage(
                "on-error must be one of: skip, fail-fast".into(),
            ));
        }
        Ok(())
    }
    fn source_id(&self, source: &str) -> Result<i64, CommandError> {
        if matches!(self, Self::Bookmark(_)) {
            return positive_id(source).map_err(|_| {
                CommandError::Usage("bookmark target ID must be a positive integer".into())
            });
        }
        let source = source.trim();
        let detail = if source.is_empty() {
            "input value is required"
        } else if let Ok(id) = source.parse::<i64>() {
            if id > 0 {
                return Ok(id);
            }
            "id must be a positive integer"
        } else if pixiv_sdk::reference::parse_url(source).is_ok() {
            "URL kind is not allowed for this command"
        } else {
            "input must be a positive ID or a supported Pixiv URL"
        };
        let operation = self.operation().replace('_', " ");
        Err(CommandError::MessageText(format!(
            "user_id: {}",
            Error::new(Reason::InvalidArgument, operation).with_detail(detail)
        )))
    }
    async fn invoke<T: Transport>(&self, client: &Client<T>, id: i64) -> pixiv_sdk::Result<()> {
        match self {
            Self::Bookmark(BookmarkCommand::Add {
                input,
                restrict,
                tags,
            }) => {
                if input.entity == "novel" {
                    client
                        .add_novel_bookmark(AddNovelBookmarkRequest {
                            novel_id: id,
                            restrict: restrict.clone(),
                            tags: tags.clone(),
                        })
                        .await
                } else {
                    client
                        .add_bookmark(AddBookmarkRequest {
                            artwork_id: id,
                            restrict: restrict.clone(),
                            tags: tags.clone(),
                        })
                        .await
                }
            }
            Self::Bookmark(BookmarkCommand::Remove { input }) => {
                if input.entity == "novel" {
                    client
                        .remove_novel_bookmark(RemoveNovelBookmarkRequest { novel_id: id })
                        .await
                } else {
                    client
                        .remove_bookmark(RemoveBookmarkRequest { artwork_id: id })
                        .await
                }
            }
            Self::Follow(FollowCommand::Add { restrict, .. }) => {
                client
                    .follow_user(FollowUserRequest {
                        user_id: id,
                        restrict: restrict.clone(),
                    })
                    .await
            }
            Self::Follow(FollowCommand::Remove { .. }) => {
                client
                    .unfollow_user(UnfollowUserRequest { user_id: id })
                    .await
            }
        }
    }
}
fn positive_id(source: &str) -> Result<i64, ()> {
    if source.is_empty() || source.trim() != source || source.starts_with('_') {
        return Err(());
    }
    source.parse::<i64>().ok().filter(|id| *id > 0).ok_or(())
}

pub async fn mutation<T: Transport, R: BufRead, W: Write>(
    client: &Client<T>,
    action: Mutation,
    input: &mut R,
    terminal: bool,
    diagnostics: &mut W,
) -> Result<(), CommandError> {
    action.validate(terminal)?;
    run(action.clone(), None, input, diagnostics, |id| {
        let action = action.clone();
        async move { action.invoke(client, id).await.map_err(Into::into) }
    })
    .await
}
pub async fn saved_mutation<T: Transport + 'static, R: BufRead, W: Write>(
    execution: &Execution<T>,
    context: &Context,
    action: Mutation,
    proxy: Option<&str>,
    input: &mut R,
    terminal: bool,
    diagnostics: &mut W,
) -> Result<(), CommandError> {
    action.validate(terminal)?;
    run(action.clone(), Some(context), input, diagnostics, |id| {
        let action = action.clone();
        async move {
            execution
                .write(context, 0, proxy, move |_, client| {
                    let action = action.clone();
                    async move { action.invoke(&client, id).await.map_err(Into::into) }
                })
                .await
                .map_err(Into::into)
        }
    })
    .await
}
async fn run<R: BufRead, W: Write, F, U>(
    action: Mutation,
    context: Option<&Context>,
    input: &mut R,
    diagnostics: &mut W,
    mut invoke: F,
) -> Result<(), CommandError>
where
    F: FnMut(i64) -> U,
    U: std::future::Future<Output = Result<(), CommandError>>,
{
    if let Some(source) = &action.input().source {
        return invoke(action.source_id(source)?).await;
    }
    check_context(context)?;
    let mut prefix = Vec::new();
    let record_mode = loop {
        let bytes = input
            .fill_buf()
            .map_err(|error| CommandError::Usage(format!("read stdin input: {error}")))?;
        let Some(&byte) = bytes.first() else {
            break false;
        };
        input.consume(1);
        prefix.push(byte);
        if !matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
            break byte == b'{';
        }
    };
    if !record_mode {
        std::io::Read::read_to_end(input, &mut prefix)
            .map_err(|error| CommandError::Usage(format!("read stdin input: {error}")))?;
        if prefix.ends_with(b"\r\n") {
            prefix.truncate(prefix.len() - 2);
        } else if prefix.ends_with(b"\n") {
            prefix.pop();
        }
        if prefix.is_empty() {
            return Ok(());
        }
        let source = String::from_utf8_lossy(&prefix);
        return invoke(action.source_id(&source)?).await;
    }
    let mut input = std::io::Read::chain(std::io::Cursor::new(prefix), input);
    let mut line = Vec::new();
    let mut number = 0;
    let mut failed = false;
    loop {
        check_context(context)?;
        line.clear();
        let count = input
            .read_until(b'\n', &mut line)
            .map_err(|error| CommandError::MessageText(format!("read NDJSON input: {error}")))?;
        if count == 0 {
            break;
        }
        number += 1;
        let (code, message) = match parse_record(&line) {
            Err(message) => ("invalid_record", Some(message)),
            Ok((_, typ)) if !action.accepts(&typ) => ("unsupported_type", Some(format!("record type {typ:?} is not supported by {}", action.operation()))),
            Ok((id, _)) => match positive_id(&id) {
                Err(_) => ("invalid_id", Some("record id must be a positive integer: record id must be a positive integer".into())),
                Ok(id) => ("action_failed", invoke(id).await.err().map(|error| error.to_string())),
            },
        };
        if let Some(message) = message {
            check_context(context)?;
            failed = true;
            let mut diagnostic = serde_json::json!({"kind":"record_error","operation":action.operation(),"line":number,"code":code,"message":message});
            let mut decoder = serde_json::Deserializer::from_slice(&line);
            if let Ok(raw) =
                <&serde_json::value::RawValue as serde::Deserialize>::deserialize(&mut decoder)
                && let Ok(fields) = serde_json::from_str::<
                    std::collections::BTreeMap<String, Box<serde_json::value::RawValue>>,
                >(raw.get())
            {
                if let Some(raw) = fields.get("id") {
                    let id = if raw.get().starts_with('"') {
                        serde_json::from_str::<String>(raw.get()).ok()
                    } else if raw
                        .get()
                        .starts_with(|ch: char| ch.is_ascii_digit() || ch == '-')
                    {
                        Some(raw.get().to_owned())
                    } else {
                        None
                    };
                    if let Some(id) = id.filter(|id| !id.is_empty()) {
                        diagnostic["id"] = id.into();
                    }
                }
                if let Some(typ) = fields
                    .get("type")
                    .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
                    .filter(|typ| !typ.is_empty())
                {
                    diagnostic["type"] = typ.into();
                }
            }
            writeln!(
                diagnostics,
                "{}",
                crate::go_json_escape(diagnostic.to_string())
            )?;
            if action.input().on_error == "fail-fast" {
                break;
            }
        }
    }
    if failed {
        Err(CommandError::Pipeline)
    } else {
        Ok(())
    }
}
fn parse_record(line: &[u8]) -> Result<(String, String), String> {
    let value: &serde_json::value::RawValue =
        serde_json::from_slice(line).map_err(|_| "invalid record JSON object".to_owned())?;
    let fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_str(value.get())
            .map_err(|_| "invalid record JSON object: record must be a JSON object".to_owned())?;
    let id = match fields.get("id") {
        None => return Err("record id is required".into()),
        Some(raw) if raw.get().starts_with('"') => {
            let id: String =
                serde_json::from_str(raw.get()).map_err(|_| "invalid record JSON object")?;
            if id.is_empty() {
                return Err("record id must be a non-empty string".into());
            }
            id
        }
        Some(raw) => {
            let id = raw.get();
            if !id.starts_with(['1', '2', '3', '4', '5', '6', '7', '8', '9'])
                || !id.bytes().all(|b| b.is_ascii_digit())
            {
                return Err("record id must be a non-empty string or positive integer".into());
            }
            id.to_owned()
        }
    };
    let required = |name: &str| -> Result<String, String> {
        let value = fields
            .get(name)
            .ok_or_else(|| format!("record {name} is required"))?;
        let value: String = serde_json::from_str(value.get())
            .map_err(|_| format!("record {name} must be a string"))?;
        if value.is_empty() {
            return Err(format!("record {name} must be a non-empty string"));
        }
        Ok(value)
    };
    let typ = required("type")?;
    required("url")?;
    Ok((id, typ))
}

fn check_context(context: Option<&Context>) -> Result<(), CommandError> {
    if let Some(error) = context.and_then(Context::error) {
        return Err(CommandError::State(Box::new(error)));
    }
    Ok(())
}

pub fn argument_error(error: &clap::Error) -> Option<CommandError> {
    if error.kind() != clap::error::ErrorKind::UnknownArgument {
        return None;
    }
    let name = error.get(clap::error::ContextKind::InvalidArg)?.to_string();
    let name = name.split('=').next()?;
    name.starts_with('-')
        .then(|| CommandError::Usage(format!("unknown option '{name}'")))
}

pub async fn saved_mutation_with_factory<T: Transport + 'static, R: BufRead, W: Write, F>(
    context: &Context,
    action: Mutation,
    input: &mut R,
    terminal: bool,
    diagnostics: &mut W,
    mut open: F,
) -> Result<(), CommandError>
where
    F: FnMut() -> Result<std::sync::Arc<Execution<T>>, CommandError>,
{
    action.validate(terminal)?;
    run(action.clone(), Some(context), input, diagnostics, |id| {
        let action = action.clone();
        let execution = action
            .proxy_override()
            .map(|proxy| proxy.map(str::to_owned))
            .and_then(|proxy| open().map(|execution| (execution, proxy)));
        async move {
            let (execution, proxy) = execution?;
            execution
                .write(context, 0, proxy.as_deref(), move |_, client| {
                    let action = action.clone();
                    async move { action.invoke(&client, id).await.map_err(Into::into) }
                })
                .await
                .map_err(Into::into)
        }
    })
    .await
}
