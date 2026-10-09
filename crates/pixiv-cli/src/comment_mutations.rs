use crate::CommandError;
use clap::{Args, Subcommand};
use pixiv_app::{execution::Execution, lifecycle::Context};
use pixiv_sdk::{Client, Error, Reason, pixiv::*, transport::Transport};
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
};

#[derive(Args, Clone, Debug, Default)]
pub struct Input {
    #[arg(num_args=0..)]
    pub sources: Vec<String>,
    #[arg(long = "type", short = 't')]
    pub entity: Option<String>,
    #[arg(long,short='j',num_args=0..=1,require_equals=true,default_missing_value="true",value_parser=crate::timeline::timeline_boolean)]
    pub json: Option<bool>,
    #[arg(long)]
    pub proxy: Option<String>,
    #[arg(long,num_args=0..=1,require_equals=true,default_missing_value="true",value_parser=crate::timeline::timeline_boolean)]
    pub no_proxy: Option<bool>,
}
#[derive(Subcommand, Clone, Debug)]
pub enum CommentMutation {
    #[command(args_override_self = true)]
    Create {
        #[command(flatten)]
        input: Input,
        #[arg(long, default_value = "")]
        comment: String,
    },
    #[command(args_override_self = true)]
    Delete {
        #[command(flatten)]
        input: Input,
    },
    #[command(args_override_self = true)]
    Reply {
        #[command(flatten)]
        input: Input,
        #[arg(long, default_value = "")]
        comment: String,
        #[arg(long, default_value = "0", allow_hyphen_values = true, value_parser=crate::timeline::timeline_integer)]
        parent_comment_id: i64,
    },
    #[command(args_override_self = true)]
    Stamp {
        #[command(flatten)]
        input: Input,
        #[arg(long, default_value = "")]
        comment: String,
        #[arg(long, default_value = "0", allow_hyphen_values = true, value_parser=crate::timeline::timeline_integer)]
        stamp_id: i64,
    },
}
impl CommentMutation {
    pub fn input(&self) -> &Input {
        match self {
            Self::Create { input, .. }
            | Self::Delete { input }
            | Self::Reply { input, .. }
            | Self::Stamp { input, .. } => input,
        }
    }
    fn input_mut(&mut self) -> &mut Input {
        match self {
            Self::Create { input, .. }
            | Self::Delete { input }
            | Self::Reply { input, .. }
            | Self::Stamp { input, .. } => input,
        }
    }
    pub fn operation(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Delete { .. } => "delete",
            Self::Reply { .. } => "reply",
            Self::Stamp { .. } => "stamp",
        }
    }
    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        if self.input().sources.is_empty() && !terminal {
            let mut bytes = vec![];
            input
                .read_to_end(&mut bytes)
                .map_err(|error| CommandError::Usage(format!("read stdin value: {error}")))?;
            if bytes.ends_with(b"\r\n") {
                bytes.truncate(bytes.len() - 2);
            } else if bytes.ends_with(b"\n") {
                bytes.pop();
            }
            if !bytes.is_empty() {
                self.input_mut()
                    .sources
                    .push(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        if self.input().sources.len() != 1 {
            return Err(CommandError::MessageText(format!(
                "usage: pixiv comment {} [options] {}",
                self.operation(),
                if matches!(self, Self::Delete { .. }) {
                    "COMMENT_ID"
                } else {
                    "ID"
                }
            )));
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<i64, CommandError> {
        let entity = self
            .input()
            .entity
            .as_deref()
            .ok_or(CommandError::Message("--type is required for comment"))?;
        if !matches!(entity, "artwork" | "novel") {
            return Err(CommandError::Message("type must be one of artwork, novel"));
        }
        if let Self::Create { comment, .. } | Self::Reply { comment, .. } = self
            && comment.is_empty()
        {
            return Err(CommandError::Message("--comment is required"));
        }
        if let Self::Reply {
            parent_comment_id, ..
        } = self
            && *parent_comment_id <= 0
        {
            return Err(CommandError::Message(
                "--parent-comment-id must be positive",
            ));
        }
        if let Self::Stamp { stamp_id, .. } = self
            && *stamp_id <= 0
        {
            return Err(CommandError::Message("--stamp-id must be positive"));
        }
        let value = self.input().sources.first().map(|s| s.trim()).unwrap_or("");
        let invalid = |detail| {
            CommandError::Sdk(
                Error::new(
                    Reason::InvalidArgument,
                    format!("comment {}", self.operation()),
                )
                .with_detail(detail),
            )
        };
        if value.is_empty() {
            return Err(invalid("input value is required"));
        }
        if let Ok(id) = value.parse::<i64>() {
            return if id > 0 {
                Ok(id)
            } else {
                Err(invalid("id must be a positive integer"))
            };
        }
        pixiv_sdk::reference::parse_url(value)
            .map_err(|_| invalid("input must be a positive ID or a supported Pixiv URL"))?;
        Err(invalid("URL kind is not allowed for this command"))
    }
    pub fn proxy_override(&self) -> Result<Option<&str>, CommandError> {
        let input = self.input();
        if input.proxy.is_some() && input.no_proxy.is_some() {
            return Err(CommandError::Message(
                "use either --proxy or --no-proxy, not both",
            ));
        }
        Ok(if input.no_proxy == Some(true) {
            Some("")
        } else {
            input.proxy.as_deref()
        })
    }
    async fn invoke<T: Transport>(
        &self,
        client: &Client<T>,
        id: i64,
    ) -> pixiv_sdk::Result<Option<i64>> {
        let novel = self.input().entity.as_deref() == Some("novel");
        let result = match self {
            Self::Create { comment, .. } => {
                if novel {
                    client
                        .post_novel_comment(PostNovelCommentRequest {
                            novel_id: id,
                            comment: comment.clone(),
                        })
                        .await?
                } else {
                    client
                        .post_artwork_comment(PostArtworkCommentRequest {
                            artwork_id: id,
                            comment: comment.clone(),
                        })
                        .await?
                }
            }
            Self::Reply {
                comment,
                parent_comment_id,
                ..
            } => {
                if novel {
                    client
                        .reply_novel_comment(ReplyNovelCommentRequest {
                            novel_id: id,
                            comment: comment.clone(),
                            parent_comment_id: *parent_comment_id,
                        })
                        .await?
                } else {
                    client
                        .reply_artwork_comment(ReplyArtworkCommentRequest {
                            artwork_id: id,
                            comment: comment.clone(),
                            parent_comment_id: *parent_comment_id,
                        })
                        .await?
                }
            }
            Self::Stamp {
                comment, stamp_id, ..
            } => {
                if novel {
                    client
                        .stamp_novel_comment(StampNovelCommentRequest {
                            novel_id: id,
                            comment: comment.clone(),
                            stamp_id: *stamp_id,
                        })
                        .await?
                } else {
                    client
                        .stamp_artwork_comment(StampArtworkCommentRequest {
                            artwork_id: id,
                            comment: comment.clone(),
                            stamp_id: *stamp_id,
                        })
                        .await?
                }
            }
            Self::Delete { .. } => {
                if novel {
                    client
                        .delete_novel_comment(DeleteNovelCommentRequest { comment_id: id })
                        .await?
                } else {
                    client
                        .delete_artwork_comment(DeleteArtworkCommentRequest { comment_id: id })
                        .await?
                };
                return Ok(None);
            }
        };
        Ok(Some(result.comment_id))
    }
}
fn output<W: Write>(out: &mut W, result: Option<i64>, json: bool) -> Result<(), CommandError> {
    let body = match (result, json) {
        (Some(id), true) => format!("{{\n  \"comment_id\": {id}\n}}\n"),
        (None, true) => "{\n  \"deleted\": true\n}\n".into(),
        (Some(id), false) => format!("comment id: {id}\n"),
        (None, false) => "comment deleted\n".into(),
    };
    let _ = out.write(body.as_bytes())?;
    Ok(())
}
pub async fn mutation<T: Transport, W: Write>(
    client: &Client<T>,
    action: &CommentMutation,
    json: bool,
    out: &mut W,
) -> Result<(), CommandError> {
    let id = action.validate()?;
    action.proxy_override()?;
    let result = action.invoke(client, id).await?;
    output(out, result, json)
}
pub async fn saved_mutation<T: Transport + 'static, W: Write>(
    execution: &Execution<T>,
    context: &Context,
    action: CommentMutation,
    json: bool,
    out: &mut W,
) -> Result<(), CommandError> {
    let id = action.validate()?;
    let proxy = action.proxy_override()?.map(str::to_owned);
    let result = Arc::new(Mutex::new(None));
    let retained = result.clone();
    execution
        .write(context, 0, proxy.as_deref(), move |_, client| {
            let action = action.clone();
            let retained = retained.clone();
            async move {
                let value = action.invoke(&client, id).await?;
                *retained.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
                Ok(())
            }
        })
        .await?;
    let value = result
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .expect("successful comment mutation produced a result");
    output(out, value, json)
}

pub fn argument_error(error: &clap::Error) -> Option<CommandError> {
    if let Some(error) = crate::timeline::argument_error(error) {
        return Some(error);
    }
    if !matches!(
        error.kind(),
        clap::error::ErrorKind::ValueValidation | clap::error::ErrorKind::InvalidValue
    ) {
        return None;
    }
    let argument = error.get(clap::error::ContextKind::InvalidArg)?.to_string();
    let name = argument.split([' ', '[', '<']).next()?;
    if !matches!(name, "--parent-comment-id" | "--stamp-id") {
        return None;
    }
    let value = error
        .get(clap::error::ContextKind::InvalidValue)?
        .to_string();
    if value.is_empty() && error.kind() == clap::error::ErrorKind::InvalidValue {
        return Some(CommandError::MessageText(format!(
            "flag needs an argument: {name}"
        )));
    }
    let detail = crate::timeline::timeline_integer(&value).err()?;
    Some(CommandError::MessageText(format!(
        "invalid argument {} for {} flag: strconv.ParseInt: parsing {}: {detail}",
        crate::search::quote(&value),
        crate::search::quote(name),
        crate::search::quote(&value)
    )))
}
