use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Error {
    pub code: Reason,
    pub operation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

impl Error {
    pub fn new(code: Reason, operation: &'static str) -> Self {
        Self {
            code,
            operation,
            http_status: None,
            retry_after_seconds: None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pixiv:{}: {}",
            self.operation,
            serde_json::to_value(self.code)
                .map_err(|_| fmt::Error)?
                .as_str()
                .ok_or(fmt::Error)?
        )
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
