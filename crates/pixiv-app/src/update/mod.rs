pub mod assembly;
pub mod coordinator;
pub mod http;
pub mod installer;
pub mod release;
pub mod source;

use pixiv_sdk::context::RequestContext;
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, future::Future, pin::Pin, sync::Arc};

pub type UpdateFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type CallerContext = Arc<dyn RequestContext>;
pub type ExternalError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct BuildInfo {
    pub version: String,
}
impl BuildInfo {
    pub fn current() -> Self {
        Self {
            version: option_env!("PIXIV_BUILD_VERSION").unwrap_or("dev").into(),
        }
    }
    pub fn is_development(&self) -> bool {
        self.version == "dev"
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstallSource {
    Development,
    HomebrewStable,
    HomebrewBeta,
    GoInstall,
    Release,
    Other(String),
}
impl InstallSource {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Development => "development",
            Self::HomebrewStable => "homebrew-stable",
            Self::HomebrewBeta => "homebrew-beta",
            Self::GoInstall => "go-install",
            Self::Release => "release",
            Self::Other(value) => value,
        }
    }
}
impl From<&str> for InstallSource {
    fn from(value: &str) -> Self {
        match value {
            "development" => Self::Development,
            "homebrew-stable" => Self::HomebrewStable,
            "homebrew-beta" => Self::HomebrewBeta,
            "go-install" => Self::GoInstall,
            "release" => Self::Release,
            value => Self::Other(value.into()),
        }
    }
}
impl Serialize for InstallSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for InstallSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from(String::deserialize(deserializer)?.as_str()))
    }
}
pub trait SourceDetector: Send + Sync {
    fn detect(&self, info: &BuildInfo) -> Result<InstallSource, ExternalError>;
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ReleaseAsset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub download_url: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct Release {
    pub tag_name: String,
    pub version: String,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReleaseCheckOptions {
    pub include_prerelease: bool,
    pub automatic: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReleaseCheckResult {
    pub release: Option<Release>,
    pub throttled: bool,
}
pub trait ReleaseChecker: Send + Sync {
    fn check(
        &self,
        context: CallerContext,
        options: ReleaseCheckOptions,
    ) -> UpdateFuture<'_, Result<ReleaseCheckResult, ExternalError>>;
}
pub trait ReleaseCache: Send + Sync {
    fn read(
        &self,
        context: CallerContext,
    ) -> UpdateFuture<'_, Result<Option<Vec<u8>>, ExternalError>>;
    fn write(
        &self,
        context: CallerContext,
        data: Vec<u8>,
    ) -> UpdateFuture<'_, Result<(), ExternalError>>;
}
pub trait ReleaseInstaller: Send + Sync {
    fn install(
        &self,
        context: CallerContext,
        release: Release,
    ) -> UpdateFuture<'_, Result<(), ExternalError>>;
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct Command {
    pub name: String,
    pub args: Vec<String>,
}
pub trait CommandRunner: Send + Sync {
    fn run(
        &self,
        context: CallerContext,
        command: Command,
    ) -> UpdateFuture<'_, Result<(), ExternalError>>;
}

#[derive(Debug)]
pub(crate) struct MessageError(pub String);
impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for MessageError {}
pub(crate) fn message(value: impl Into<String>) -> ExternalError {
    Box::new(MessageError(value.into()))
}
#[derive(Debug)]
pub(crate) struct WrappedError {
    pub label: String,
    pub cause: ExternalError,
}
impl fmt::Display for WrappedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.label, self.cause)
    }
}
impl Error for WrappedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}
pub(crate) fn wrap(label: impl Into<String>, cause: ExternalError) -> ExternalError {
    Box::new(WrappedError {
        label: label.into(),
        cause,
    })
}
#[derive(Debug)]
pub struct JoinedError(pub Vec<ExternalError>);
impl fmt::Display for JoinedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            error.fmt(f)?;
        }
        Ok(())
    }
}
impl Error for JoinedError {}
impl JoinedError {
    pub fn causes(&self) -> impl Iterator<Item = &(dyn Error + Send + Sync + 'static)> {
        self.0.iter().map(|error| error.as_ref())
    }
}
pub(crate) fn join(errors: Vec<ExternalError>) -> ExternalError {
    Box::new(JoinedError(errors))
}
pub(crate) fn check_context(context: &CallerContext, action: &str) -> Result<(), ExternalError> {
    match context.error() {
        Some(error) => Err(wrap(action, Box::new(error))),
        None => Ok(()),
    }
}
pub(crate) use crate::auth_bundle::go_quote;
