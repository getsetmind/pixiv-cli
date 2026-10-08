use crate::{
    config::ConfigError,
    database::{
        AccountError, Database, PixivAccount, PoolChooser, PoolError, PoolSelectionKind,
        PoolSnapshot, choose_pool_account,
    },
    diagnostics::Event,
    lifecycle::{Attempt, Context, ContextError},
};
use chrono::{DateTime, TimeDelta, Utc};
use pixiv_sdk::{
    Error, Reason,
    error::{Cause, RetryAdvice},
};
use std::{
    collections::BTreeSet,
    error::Error as StdError,
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

#[derive(Debug)]
pub enum SchedulerError {
    Sdk(Box<Error>),
    Pool(PoolError),
    Account(AccountError),
    Config(ConfigError),
    Joined(Vec<SchedulerError>),
    Shared(Arc<SchedulerError>),
    Message(String),
    Canceled,
    DeadlineExceeded,
    Exhausted(Option<Box<Error>>),
    Wrapped {
        message: String,
        source: Box<SchedulerError>,
    },
}
impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sdk(error) | Self::Exhausted(Some(error)) => fmt::Display::fmt(error, f),
            Self::Pool(error) => fmt::Display::fmt(error, f),
            Self::Account(error) => fmt::Display::fmt(error, f),
            Self::Config(error) => fmt::Display::fmt(error, f),
            Self::Shared(error) => fmt::Display::fmt(error, f),
            Self::Joined(errors) => {
                for (index, error) in errors.iter().enumerate() {
                    if index > 0 {
                        f.write_str("\n")?;
                    }
                    fmt::Display::fmt(error, f)?;
                }
                Ok(())
            }
            Self::Message(message) => f.write_str(message),
            Self::Canceled => f.write_str("context canceled"),
            Self::DeadlineExceeded => f.write_str("context deadline exceeded"),
            Self::Exhausted(None) => f.write_str("account pool has no available account"),
            Self::Wrapped { message, source } => write!(f, "{message}: {source}"),
        }
    }
}
impl StdError for SchedulerError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Sdk(error) | Self::Exhausted(Some(error)) => Some(error.as_ref()),
            Self::Pool(error) => Some(error),
            Self::Account(error) => Some(error),
            Self::Config(error) => Some(error),
            Self::Shared(error) => Some(error.as_ref()),
            Self::Joined(errors) => errors.first().map(|error| error as &dyn StdError),
            Self::Wrapped { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}
impl From<Error> for SchedulerError {
    fn from(error: Error) -> Self {
        Self::Sdk(Box::new(error))
    }
}
impl From<PoolError> for SchedulerError {
    fn from(error: PoolError) -> Self {
        Self::Pool(error)
    }
}
impl From<ContextError> for SchedulerError {
    fn from(error: ContextError) -> Self {
        match error {
            ContextError::Canceled => Self::Canceled,
            ContextError::DeadlineExceeded => Self::DeadlineExceeded,
        }
    }
}

impl From<ConfigError> for SchedulerError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl SchedulerError {
    fn find<T: StdError + 'static>(&self) -> Option<&T> {
        let mut current: &(dyn StdError + 'static) = self;
        loop {
            if let Some(value) = current.downcast_ref() {
                return Some(value);
            }
            current = current.source()?;
        }
    }
    pub fn classified(&self) -> Option<&Error> {
        match self {
            Self::Joined(errors) => errors.iter().find_map(Self::classified),
            Self::Shared(error) => error.classified(),
            Self::Wrapped { source: error, .. } => error.classified(),
            _ => self.find(),
        }
    }
    pub fn is_exhausted(&self) -> bool {
        match self {
            Self::Exhausted(_) => true,
            Self::Wrapped { source, .. } => source.is_exhausted(),
            _ => false,
        }
    }
    pub fn is_canceled(&self) -> bool {
        match self {
            Self::Joined(errors) => errors.iter().any(Self::is_canceled),
            Self::Shared(error) => error.is_canceled(),
            Self::Canceled => true,
            Self::Wrapped { source, .. } => source.is_canceled(),
            _ => pixiv_sdk::error::is_canceled(self),
        }
    }
    pub fn is_deadline_exceeded(&self) -> bool {
        match self {
            Self::Joined(errors) => errors.iter().any(Self::is_deadline_exceeded),
            Self::Shared(error) => error.is_deadline_exceeded(),
            Self::DeadlineExceeded => true,
            Self::Wrapped { source, .. } => source.is_deadline_exceeded(),
            _ => pixiv_sdk::error::is_deadline_exceeded(self),
        }
    }
    fn into_cause(self) -> Cause {
        match self {
            Self::Joined(errors) => {
                Cause::Joined(errors.into_iter().map(Self::into_cause).collect())
            }
            Self::Shared(error) => error.snapshot_cause(),
            Self::Sdk(error) | Self::Exhausted(Some(error)) => Cause::Classified(error),
            Self::Canceled => Cause::Canceled,
            Self::DeadlineExceeded => Cause::DeadlineExceeded,
            Self::Wrapped { message, source } => Cause::Wrapped {
                message,
                source: Box::new(source.into_cause()),
            },
            other => Cause::Redacted(other.to_string()),
        }
    }
    fn snapshot_cause(&self) -> Cause {
        match self {
            Self::Sdk(error) | Self::Exhausted(Some(error)) => Cause::Classified(error.clone()),
            Self::Canceled => Cause::Canceled,
            Self::DeadlineExceeded => Cause::DeadlineExceeded,
            Self::Wrapped { message, source } => Cause::Wrapped {
                message: message.clone(),
                source: Box::new(source.snapshot_cause()),
            },
            Self::Shared(error) => error.snapshot_cause(),
            Self::Joined(errors) => {
                Cause::Joined(errors.iter().map(Self::snapshot_cause).collect())
            }
            other => Cause::Redacted(other.to_string()),
        }
    }
}

pub trait PoolState: Send {
    fn select(
        &mut self,
        context: &Context,
        now: i64,
        attempted: &[i64],
        chooser: Option<&mut PoolChooser<'_>>,
    ) -> Result<PixivAccount, SchedulerError>;
    fn freeze(&mut self, context: &Context, id: i64, until: i64) -> Result<(), SchedulerError>;
}
impl PoolState for Arc<Mutex<Database>> {
    fn select(
        &mut self,
        context: &Context,
        now: i64,
        attempted: &[i64],
        chooser: Option<&mut PoolChooser<'_>>,
    ) -> Result<PixivAccount, SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .select_pixiv(now, attempted, chooser)
            .map_err(Into::into)
    }
    fn freeze(&mut self, context: &Context, id: i64, until: i64) -> Result<(), SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .freeze_pixiv(id, until)
            .map_err(Into::into)
    }
}
impl PoolState for Database {
    fn select(
        &mut self,
        context: &Context,
        now: i64,
        attempted: &[i64],
        chooser: Option<&mut PoolChooser<'_>>,
    ) -> Result<PixivAccount, SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.select_pixiv(now, attempted, chooser)
            .map_err(Into::into)
    }
    fn freeze(&mut self, context: &Context, id: i64, until: i64) -> Result<(), SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.freeze_pixiv(id, until).map_err(Into::into)
    }
}

pub type AttemptFuture<'a> = Pin<Box<dyn Future<Output = Result<(), SchedulerError>> + Send + 'a>>;
pub type AttemptCallback<'a> =
    dyn FnMut(Context, i64, Arc<Attempt>) -> AttemptFuture<'a> + Send + 'a;
pub type RandomSource<'a> = dyn FnMut(usize) -> Result<i64, PoolError> + Send + 'a;
pub type Clock<'a> = dyn FnMut() -> DateTime<Utc> + Send + 'a;

pub struct Scheduler<'a> {
    pub enabled: bool,
    pub strategy: &'a str,
    pub state: Option<&'a mut dyn PoolState>,
    pub now: Option<&'a mut Clock<'a>>,
    pub random: Option<&'a mut RandomSource<'a>>,
}
impl Scheduler<'_> {
    fn current(&mut self) -> DateTime<Utc> {
        match self.now.as_deref_mut() {
            Some(clock) => clock(),
            None => Utc::now(),
        }
    }
    pub async fn run(
        &mut self,
        context: &Context,
        attempt: Option<&mut AttemptCallback<'_>>,
    ) -> Result<(), SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        if !self.enabled {
            return Err(SchedulerError::Message("account pool is disabled".into()));
        }
        if self.state.is_none() {
            return Err(SchedulerError::Message(
                "account pool state store is not configured".into(),
            ));
        }
        let attempt = attempt.ok_or_else(|| {
            SchedulerError::Message("account pool attempt is not configured".into())
        })?;
        let mut attempted = Vec::new();
        let mut seen = BTreeSet::new();
        let mut last = None;
        loop {
            if let Some(error) = context.error() {
                return Err(error.into());
            }
            let current = self.current();
            let selected = {
                let random = &mut self.random;
                let strategy = self.strategy;
                let mut chooser = |snapshot: &PoolSnapshot| {
                    let source = random
                        .as_deref_mut()
                        .map(|value| value as &mut dyn FnMut(usize) -> Result<i64, PoolError>);
                    choose_pool_account(snapshot, strategy, source)
                };
                self.state.as_deref_mut().unwrap().select(
                    context,
                    current.timestamp(),
                    &attempted,
                    Some(&mut chooser),
                )
            };
            let account = selected.map_err(|error| map_selection(error, last.take(), current))?;
            let id = account.user_id;
            if id <= 0 {
                return Err(SchedulerError::Message(
                    "account pool state store selected an invalid user id".into(),
                ));
            }
            context.emit(Event {
                module: "Pixiv account pool".into(),
                kind: "account".into(),
                operation: "selected".into(),
                resource: format!("uid {id}"),
                ..Event::default()
            });
            if seen.contains(&id) {
                return Err(SchedulerError::Message(format!(
                    "account pool state store selected attempted user id {id}"
                )));
            }
            let state = Arc::new(Attempt::default());
            let result = attempt(context.clone(), id, Arc::clone(&state)).await;
            let error = match result {
                Ok(()) => return Ok(()),
                Err(error) => error,
            };
            if state.committed() {
                return Err(error);
            }
            if let Some(error) = context.error() {
                return Err(error.into());
            }
            let after = error
                .classified()
                .filter(|typed| typed.code == Reason::RateLimited && typed.retry.safe)
                .and_then(|typed| typed.retry.after)
                .filter(|after| *after > current);
            let Some(after) = after else {
                return Err(error);
            };
            let nanos = after
                .signed_duration_since(current)
                .num_nanoseconds()
                .unwrap_or(i64::MAX);
            last = Some(error);
            attempted.push(id);
            seen.insert(id);
            let until = self
                .current()
                .checked_add_signed(TimeDelta::nanoseconds(nanos))
                .ok_or_else(|| SchedulerError::Message("account pool retry time overflow".into()))?
                .timestamp();
            self.state
                .as_deref_mut()
                .unwrap()
                .freeze(context, id, until)?;
            context.emit(Event {
                module: "Pixiv account pool".into(),
                kind: "account".into(),
                operation: "froze".into(),
                resource: format!("uid {id}"),
                reason: "account frozen".into(),
                ..Event::default()
            });
        }
    }
}

fn rate_limit(
    detail: &str,
    earliest: Option<i64>,
    cause: Option<SchedulerError>,
    now: DateTime<Utc>,
) -> Error {
    let after = earliest
        .and_then(|value| DateTime::<Utc>::from_timestamp(value, 0))
        .filter(|after| *after > now);
    let mut error = Error::new(Reason::RateLimited, "account_pool").with_detail(detail);
    if after.is_some() {
        error = error.with_retry(RetryAdvice { safe: true, after });
    }
    if let Some(cause) = cause {
        error = error.with_cause(cause.into_cause());
    }
    error
}

fn exhausted(
    last: Option<SchedulerError>,
    earliest: Option<i64>,
    now: DateTime<Utc>,
) -> SchedulerError {
    match last {
        None => SchedulerError::Exhausted(None),
        Some(last) => SchedulerError::Exhausted(Some(Box::new(rate_limit(
            "account_pool_exhausted",
            earliest,
            Some(last),
            now,
        )))),
    }
}

fn map_selection(
    error: SchedulerError,
    last: Option<SchedulerError>,
    now: DateTime<Utc>,
) -> SchedulerError {
    match error.find::<PoolError>() {
        Some(PoolError::Selection {
            kind,
            earliest_frozen_until,
        }) => match kind {
            PoolSelectionKind::NoLocalAccount => Error::new(Reason::Unauthorized, "account_pool")
                .with_detail("account_pool_no_local_account")
                .into(),
            PoolSelectionKind::NoSchedulableAccount => {
                Error::new(Reason::LocalStateError, "account_pool")
                    .with_detail("account_pool_no_schedulable_account")
                    .into()
            }
            PoolSelectionKind::AllFrozen => {
                rate_limit("account_pool_all_frozen", *earliest_frozen_until, None, now).into()
            }
            PoolSelectionKind::Exhausted => exhausted(last, *earliest_frozen_until, now),
        },
        Some(PoolError::UnknownSelection(kind)) => SchedulerError::Message(format!(
            "account pool state store returned unknown selection kind {kind:?}"
        )),
        _ if error.is_exhausted() => exhausted(last, None, now),
        _ => Error::new(Reason::LocalStateError, "account_pool")
            .with_detail("account_pool_state_error")
            .with_cause(error.into_cause())
            .into(),
    }
}
