use crate::{CommandError, DetailOutput, user_works::UserWorksOptions};
use pixiv_app::{execution::Execution, lifecycle::Context};
use pixiv_sdk::{Client, Error, Reason, cursor::Cursor, transport::Transport};
use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};
#[derive(clap::Args, Clone, Debug)]
pub struct UserFollowingOptions {
    #[command(flatten)]
    pub listing: UserWorksOptions,
    #[arg(long, default_value = "public")]
    pub restrict: String,
}
#[derive(Clone, Debug)]
pub enum UserRelationships {
    Following(UserFollowingOptions),
    Followers(UserFollowingOptions),
    Related(UserWorksOptions),
    Blocked(UserWorksOptions),
    MyPixiv(UserWorksOptions),
}
impl UserRelationships {
    pub fn options(&self) -> &UserWorksOptions {
        match self {
            Self::Following(o) | Self::Followers(o) => &o.listing,
            Self::Related(o) | Self::Blocked(o) | Self::MyPixiv(o) => o,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Following(_) => "following",
            Self::Followers(_) => "followers",
            Self::Related(_) => "related",
            Self::Blocked(_) => "blocked",
            Self::MyPixiv(_) => "mypixiv",
        }
    }
    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        if matches!(self, Self::MyPixiv(_)) {
            return self.validate_arguments();
        }
        if self.options().sources.len() > 1 {
            self.validate_arguments()?;
        }
        let options = match self {
            Self::Following(options) | Self::Followers(options) => &mut options.listing,
            Self::Related(options) | Self::Blocked(options) | Self::MyPixiv(options) => options,
        };
        crate::user_works::resolve_optional_source(options, input, terminal)?;
        self.validate_arguments()
    }
    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if (matches!(self, Self::MyPixiv(_)) && !self.options().sources.is_empty())
            || self.options().sources.len() > 1
            || (matches!(self, Self::Related(_)) && self.options().sources.is_empty())
        {
            return Err(CommandError::Message(match self {
                Self::Following(_) => "usage: pixiv user following [options] [USER_ID]",
                Self::Followers(_) => "usage: pixiv user followers [options] [USER_ID]",
                Self::Related(_) => "usage: pixiv user related [options] USER_ID",
                Self::Blocked(_) => "usage: pixiv user blocked [options] [USER_ID]",
                Self::MyPixiv(_) => "usage: pixiv mypixiv users",
            }));
        }
        Ok(())
    }
    fn user_id(&self) -> Result<i64, CommandError> {
        if matches!(self, Self::MyPixiv(_)) {
            return Ok(0);
        }
        crate::user_works::resolved_user_id(
            self.options().sources.first(),
            &format!("user {}", self.name()),
        )
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        self.validate_arguments()?;
        if let Self::Following(options) | Self::Followers(options) = self
            && !matches!(options.restrict.as_str(), "" | "public" | "private")
        {
            return Err(Error::new(Reason::InvalidArgument, "user relationships")
                .with_detail("restrict must be public or private")
                .into());
        }
        self.user_id()?;
        self.plan()?;
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
        } else if options.json.is_none()
            && !terminal
            && !matches!(self, Self::Following(_) | Self::MyPixiv(_))
        {
            DetailOutput::Ndjson
        } else {
            DetailOutput::Human
        })
    }
}
#[derive(Clone)]
struct Listing {
    options: UserRelationships,
    identity: Arc<AtomicI64>,
    plan: crate::search::SearchPlan,
}
use crate::json_spool::JsonSpool;
use pixiv_app::{facade::UseOutcome, scheduler::SchedulerError};
use std::sync::{Mutex, atomic::AtomicBool};
pub async fn saved_user_relationships<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: UserRelationships,
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    options.validate()?;
    let listing = Listing {
        identity: Arc::new(AtomicI64::new(options.user_id()?)),
        plan: options.plan()?,
        options,
    };
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

pub async fn user_relationships<T: Transport, W: Write>(
    client: &Client<T>,
    options: &UserRelationships,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    options.validate()?;
    let listing = Listing {
        options: options.clone(),
        identity: Arc::new(AtomicI64::new(options.user_id()?)),
        plan: options.plan()?,
    };
    if let Some(mut spool) = attempt(client, &listing, mode, out).await? {
        spool.commit(out)?;
    }
    Ok(())
}
async fn attempt<T: Transport, W: Write>(
    client: &Client<T>,
    listing: &Listing,
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
            skip: listing.plan.skip as i64,
            limit: listing.plan.limit as i64,
            one_batch: listing.plan.one_batch,
        },
        Cursor::default(),
        |cursor| async move {
            let user_id = if matches!(listing.options, UserRelationships::MyPixiv(_)) {
                let mut id = listing.identity.load(Ordering::Acquire);
                if id == 0 {
                    id = client.user_id();
                    if id <= 0 {
                        return Err(Error::new(Reason::Unauthorized, "MyPixivUsers")
                            .with_detail("cannot determine current user id")
                            .into());
                    }
                    listing.identity.store(id, Ordering::Release);
                }
                id
            } else {
                crate::user_works::current_target(client, &listing.identity)?
            };
            use pixiv_sdk::pixiv::*;
            let page = match &listing.options {
                UserRelationships::Following(o) => {
                    client
                        .user_following(UserFollowingRequest {
                            user_id,
                            restrict: o.restrict.clone(),
                            cursor,
                        })
                        .await
                }
                UserRelationships::Followers(o) => {
                    client
                        .user_followers(UserFollowersRequest {
                            user_id,
                            restrict: o.restrict.clone(),
                            cursor,
                        })
                        .await
                }
                UserRelationships::Related(_) => {
                    client
                        .related_users(RelatedUsersRequest { user_id, cursor })
                        .await
                }
                UserRelationships::MyPixiv(_) => {
                    client.my_pixiv_users(MyPixivUsersRequest { cursor }).await
                }
                UserRelationships::Blocked(_) => {
                    client
                        .user_blocked_users(UserBlockedUsersRequest { user_id, cursor })
                        .await
                }
            }
            .map_err(CommandError::from)?;
            Ok((page.items, page.next))
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
                let id = listing.identity.load(Ordering::Acquire);
                let text = match listing.options {
                    UserRelationships::Following(_) => "users followed by",
                    UserRelationships::Followers(_) => "followers of",
                    UserRelationships::Related(_) => "users related to",
                    UserRelationships::Blocked(_) => "blocked users of",
                    UserRelationships::MyPixiv(_) => "MyPixiv users for",
                };
                writeln!(out, "{text} {id}")?;
                heading = true;
            }
            for item in &items {
                let name = if matches!(listing.options, UserRelationships::MyPixiv(_)) {
                    item.user.name.clone()
                } else {
                    crate::safe_line(&item.user.name)
                };
                let line = format!("{} {}", item.user.id, name);
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
