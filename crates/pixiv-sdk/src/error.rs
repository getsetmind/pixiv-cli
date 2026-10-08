use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{error::Error as StdError, fmt};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    InvalidArgument,
    InvalidCursor,
    Unauthorized,
    CredentialsExpired,
    Forbidden,
    NotFound,
    ContentUnavailable,
    ChallengeRequired,
    RateLimited,
    UpstreamError,
    UpstreamUnavailable,
    MalformedUpstreamResponse,
    ResourceForbidden,
    NotUgoira,
    UgoiraArchiveMissing,
    UgoiraFrameMismatch,
    LocalStateError,
    RemovedSetting,
}

impl Reason {
    pub const ALL: &'static [Self] = &[
        Self::InvalidArgument,
        Self::InvalidCursor,
        Self::Unauthorized,
        Self::CredentialsExpired,
        Self::Forbidden,
        Self::NotFound,
        Self::ContentUnavailable,
        Self::ChallengeRequired,
        Self::RateLimited,
        Self::UpstreamError,
        Self::UpstreamUnavailable,
        Self::MalformedUpstreamResponse,
        Self::ResourceForbidden,
        Self::NotUgoira,
        Self::UgoiraArchiveMissing,
        Self::UgoiraFrameMismatch,
        Self::LocalStateError,
        Self::RemovedSetting,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::InvalidCursor => "invalid_cursor",
            Self::Unauthorized => "unauthorized",
            Self::CredentialsExpired => "credentials_expired",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::ContentUnavailable => "content_unavailable",
            Self::ChallengeRequired => "challenge_required",
            Self::RateLimited => "rate_limited",
            Self::UpstreamError => "upstream_error",
            Self::UpstreamUnavailable => "upstream_unavailable",
            Self::MalformedUpstreamResponse => "malformed_upstream_response",
            Self::ResourceForbidden => "resource_forbidden",
            Self::NotUgoira => "not_ugoira",
            Self::UgoiraArchiveMissing => "ugoira_archive_missing",
            Self::UgoiraFrameMismatch => "ugoira_frame_mismatch",
            Self::LocalStateError => "local_state_error",
            Self::RemovedSetting => "removed_setting",
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    Http,
    Tls,
    Dns,
    Local,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RetryAdvice {
    pub safe: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cause {
    Canceled,
    DeadlineExceeded,
    Redacted(String),
    Classified(Box<Error>),
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canceled => f.write_str("context canceled"),
            Self::DeadlineExceeded => f.write_str("context deadline exceeded"),
            Self::Redacted(message) => f.write_str(message),
            Self::Classified(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl StdError for Cause {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Classified(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Error {
    pub code: Reason,
    pub product: String,
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportKind>,
    pub retry: RetryAdvice,
    #[serde(skip)]
    cause: Option<Box<Cause>>,
}

impl Error {
    pub fn new(code: Reason, operation: impl Into<String>) -> Self {
        Self::with_product("pixiv", code, operation)
    }

    pub fn with_product(
        product: impl Into<String>,
        code: Reason,
        operation: impl Into<String>,
    ) -> Self {
        Self {
            code,
            product: product.into(),
            operation: operation.into(),
            detail: None,
            http_status: None,
            transport: None,
            retry: RetryAdvice::default(),
            cause: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    pub fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = (status != 0).then_some(status);
        self
    }
    pub fn with_transport(mut self, transport: TransportKind) -> Self {
        self.transport = Some(transport);
        self
    }
    pub fn with_retry(mut self, retry: RetryAdvice) -> Self {
        self.retry = retry;
        self
    }
    pub fn with_cause(mut self, cause: Cause) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }

    pub fn retry_after_seconds_at(&self, now: DateTime<Utc>) -> Option<u64> {
        let after = self.retry.after?;
        if after <= now {
            return Some(0);
        }
        let nanoseconds = after
            .signed_duration_since(now)
            .num_nanoseconds()
            .unwrap_or(i64::MAX) as u64;
        // Integer rounding would differ from Go's floating-point Duration.Seconds conversion.
        let seconds = (nanoseconds / 1_000_000_000) as f64
            + (nanoseconds % 1_000_000_000) as f64 / 1_000_000_000.0;
        Some(seconds.ceil() as u64)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let product = if self.product.is_empty() {
            "sdk"
        } else {
            self.product.as_str()
        };
        write!(f, "{product}:{}: {}", self.operation, self.code)?;
        if let Some(detail) = self.detail.as_ref().filter(|detail| !detail.is_empty()) {
            write!(f, ": {detail}")?;
        }
        if let Some(cause) = self
            .cause
            .as_ref()
            .filter(|cause| !cause.to_string().is_empty())
        {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn StdError + 'static))
    }
}

pub fn reason_of(mut error: &(dyn StdError + 'static)) -> Option<Reason> {
    loop {
        if let Some(classified) = error.downcast_ref::<Error>() {
            return Some(classified.code);
        }
        error = error.source()?;
    }
}

pub fn is_reason(error: &(dyn StdError + 'static), reason: Reason) -> bool {
    reason_of(error) == Some(reason)
}

pub fn matches_reason(mut error: &(dyn StdError + 'static), reason: Reason) -> bool {
    loop {
        if error
            .downcast_ref::<Error>()
            .is_some_and(|classified| classified.code == reason)
        {
            return true;
        }
        match error.source() {
            Some(source) => error = source,
            None => return false,
        }
    }
}

fn contains_cause(mut error: &(dyn StdError + 'static), expected: &Cause) -> bool {
    loop {
        if error.downcast_ref::<Cause>() == Some(expected) {
            return true;
        }
        match error.source() {
            Some(source) => error = source,
            None => return false,
        }
    }
}

pub fn is_canceled(error: &(dyn StdError + 'static)) -> bool {
    contains_cause(error, &Cause::Canceled)
}
pub fn is_deadline_exceeded(error: &(dyn StdError + 'static)) -> bool {
    contains_cause(error, &Cause::DeadlineExceeded)
}

pub type Result<T> = std::result::Result<T, Error>;
