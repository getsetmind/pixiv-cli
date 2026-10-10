use pixiv_sdk::context::ContextError;
use serde::{Deserialize, Serialize};
use std::{error::Error as StdError, fmt, sync::Arc};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unknown,
    InvalidRequest,
    InvalidSource,
    SourceNotRegularFile,
    SourceReadFailed,
    SourceHttpStatus,
    SnapshotFailed,
    SourceLoaderNotConfigured,
    ProviderNotConfigured,
    MissingCredential,
    MalformedUpstreamResponse,
    UpstreamHttpStatus,
    ProviderFailed,
    AllProvidersFailed,
    ChallengeRequired,
    SolverUnavailable,
    SolverFailed,
    MalformedSolverResponse,
}
impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::InvalidRequest => "invalid_request",
            Self::InvalidSource => "invalid_source",
            Self::SourceNotRegularFile => "source_not_regular_file",
            Self::SourceReadFailed => "source_read_failed",
            Self::SourceHttpStatus => "source_http_status",
            Self::SnapshotFailed => "snapshot_failed",
            Self::SourceLoaderNotConfigured => "source_loader_not_configured",
            Self::ProviderNotConfigured => "provider_not_configured",
            Self::MissingCredential => "missing_credential",
            Self::MalformedUpstreamResponse => "malformed_upstream_response",
            Self::UpstreamHttpStatus => "upstream_http_status",
            Self::ProviderFailed => "provider_failed",
            Self::AllProvidersFailed => "all_providers_failed",
            Self::ChallengeRequired => "challenge_required",
            Self::SolverUnavailable => "solver_unavailable",
            Self::SolverFailed => "solver_failed",
            Self::MalformedSolverResponse => "malformed_solver_response",
        }
    }
}
#[derive(Clone)]
pub struct Error(Arc<ErrorKind>);
enum ErrorKind {
    Domain {
        code: ErrorCode,
        message: String,
        cause: Option<Error>,
    },
    External(Arc<dyn StdError + Send + Sync>),
    Context(ContextError),
    Wrapped {
        message: String,
        cause: Error,
    },
    Joined(Vec<Error>),
}
impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>, cause: Option<Error>) -> Self {
        Self(Arc::new(ErrorKind::Domain {
            code,
            message: message.into(),
            cause,
        }))
    }
    pub fn external(error: impl StdError + Send + Sync + 'static) -> Self {
        Self(Arc::new(ErrorKind::External(Arc::new(error))))
    }
    pub fn from_box(error: Box<dyn StdError + Send + Sync>) -> Self {
        Self(Arc::new(ErrorKind::External(Arc::from(error))))
    }
    pub fn wrap(message: impl Into<String>, cause: Error) -> Self {
        Self(Arc::new(ErrorKind::Wrapped {
            message: message.into(),
            cause,
        }))
    }
    pub fn join(errors: impl IntoIterator<Item = Error>) -> Option<Self> {
        let errors: Vec<_> = errors.into_iter().collect();
        (!errors.is_empty()).then(|| Self(Arc::new(ErrorKind::Joined(errors))))
    }
    pub fn code(&self) -> ErrorCode {
        code_of(self)
    }
    pub fn classified(&self) -> Option<(ErrorCode, &str)> {
        match self.0.as_ref() {
            ErrorKind::Domain { code, message, .. } => Some((*code, message)),
            ErrorKind::Wrapped { cause, .. } => cause.classified(),
            ErrorKind::Joined(errors) => errors.iter().find_map(Self::classified),
            ErrorKind::External(error) => {
                let mut current: Option<&(dyn StdError + 'static)> = Some(error.as_ref());
                while let Some(error) = current {
                    if let Some(error) = error.downcast_ref::<Self>() {
                        return error.classified();
                    }
                    current = error.source();
                }
                None
            }
            ErrorKind::Context(_) => None,
        }
    }
    pub fn context_error(&self) -> Option<ContextError> {
        match self.0.as_ref() {
            ErrorKind::Context(error) => Some(*error),
            ErrorKind::Domain { cause, .. } => cause.as_ref().and_then(Self::context_error),
            ErrorKind::Wrapped { cause, .. } => cause.context_error(),
            ErrorKind::Joined(errors) => errors.iter().find_map(Self::context_error),
            ErrorKind::External(error) => {
                let mut current: Option<&(dyn StdError + 'static)> = Some(error.as_ref());
                while let Some(error) = current {
                    if let Some(error) = error.downcast_ref::<ContextError>() {
                        return Some(*error);
                    }
                    if let Some(error) = error.downcast_ref::<Self>() {
                        return error.context_error();
                    }
                    current = error.source();
                }
                None
            }
        }
    }
    pub fn contains_context(&self, target: ContextError) -> bool {
        match self.0.as_ref() {
            ErrorKind::Context(error) => *error == target,
            ErrorKind::Domain { cause, .. } => cause
                .as_ref()
                .is_some_and(|cause| cause.contains_context(target)),
            ErrorKind::Wrapped { cause, .. } => cause.contains_context(target),
            ErrorKind::Joined(errors) => errors.iter().any(|error| error.contains_context(target)),
            ErrorKind::External(error) => {
                let mut current: Option<&(dyn StdError + 'static)> = Some(error.as_ref());
                while let Some(error) = current {
                    if error
                        .downcast_ref::<ContextError>()
                        .is_some_and(|error| *error == target)
                    {
                        return true;
                    }
                    if error
                        .downcast_ref::<Self>()
                        .is_some_and(|error| error.contains_context(target))
                    {
                        return true;
                    }
                    current = error.source();
                }
                false
            }
        }
    }
    fn as_std_error(&self) -> &(dyn StdError + 'static) {
        match self.0.as_ref() {
            ErrorKind::External(error) => error.as_ref(),
            ErrorKind::Context(error) => error,
            _ => self,
        }
    }
    pub fn joined(&self) -> Option<&[Error]> {
        match self.0.as_ref() {
            ErrorKind::Joined(errors) => Some(errors),
            _ => None,
        }
    }
    pub fn contains(&self, target: &Error) -> bool {
        if Arc::ptr_eq(&self.0, &target.0) {
            return true;
        }
        match self.0.as_ref() {
            ErrorKind::Domain { cause, .. } => {
                cause.as_ref().is_some_and(|cause| cause.contains(target))
            }
            ErrorKind::Wrapped { cause, .. } => cause.contains(target),
            ErrorKind::Joined(errors) => errors.iter().any(|error| error.contains(target)),
            ErrorKind::External(error) => {
                let mut current: Option<&(dyn StdError + 'static)> = Some(error.as_ref());
                while let Some(error) = current {
                    if error
                        .downcast_ref::<Self>()
                        .is_some_and(|error| error.contains(target))
                    {
                        return true;
                    }
                    current = error.source();
                }
                false
            }
            ErrorKind::Context(_) => false,
        }
    }
}
pub fn code_of(error: &Error) -> ErrorCode {
    error
        .classified()
        .map_or(ErrorCode::Unknown, |(code, _)| code)
}
impl From<ContextError> for Error {
    fn from(error: ContextError) -> Self {
        Self(Arc::new(ErrorKind::Context(error)))
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.as_ref() {
            ErrorKind::Domain { message, .. } | ErrorKind::Wrapped { message, .. } => {
                f.write_str(message)
            }
            ErrorKind::External(error) => fmt::Display::fmt(error, f),
            ErrorKind::Context(error) => fmt::Display::fmt(error, f),
            ErrorKind::Joined(errors) => {
                for (index, error) in errors.iter().enumerate() {
                    if index != 0 {
                        f.write_str("\n")?;
                    }
                    fmt::Display::fmt(error, f)?;
                }
                Ok(())
            }
        }
    }
}
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("code", &self.code())
            .field("message", &self.to_string())
            .finish_non_exhaustive()
    }
}
impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self.0.as_ref() {
            ErrorKind::Domain { cause, .. } => cause.as_ref().map(Self::as_std_error),
            ErrorKind::Wrapped { cause, .. } => Some(cause.as_std_error()),
            ErrorKind::External(error) => error.source(),
            ErrorKind::Context(_) => None,
            ErrorKind::Joined(_) => None,
        }
    }
}
