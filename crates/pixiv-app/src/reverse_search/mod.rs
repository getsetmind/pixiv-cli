mod aggregator;
pub mod ascii2d;
pub mod assembly;
mod errors;
mod facade;
pub mod http;
mod normalize;
pub mod saucenao;
mod source;

pub use aggregator::{Aggregator, AggregatorDependencies};
pub use errors::{Error, ErrorCode, code_of};
pub use facade::{Dependencies, Facade};
pub use source::{Loader, RedirectDecision, RedirectHook, Snapshot, SourceLoaderOptions};

use pixiv_sdk::context::RequestContext;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};

pub type ReverseFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type CallerContext = Arc<dyn RequestContext>;

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub enum Provider {
    SauceNao,
    Ascii2dColor,
    Ascii2dBovw,
    All,
    #[default]
    Unspecified,
    Other(String),
}
impl Provider {
    pub fn as_str(&self) -> &str {
        match self {
            Self::SauceNao => "saucenao",
            Self::Ascii2dColor => "ascii2d-color",
            Self::Ascii2dBovw => "ascii2d-bovw",
            Self::All => "all",
            Self::Unspecified => "",
            Self::Other(value) => value,
        }
    }
}
impl From<&str> for Provider {
    fn from(value: &str) -> Self {
        match value {
            "saucenao" => Self::SauceNao,
            "ascii2d-color" => Self::Ascii2dColor,
            "ascii2d-bovw" => Self::Ascii2dBovw,
            "all" => Self::All,
            "" => Self::Unspecified,
            value => Self::Other(value.to_owned()),
        }
    }
}
impl Serialize for Provider {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for Provider {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(Self::from(String::deserialize(deserializer)?.as_str()))
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub enum SourceKind {
    #[serde(rename = "")]
    #[default]
    Unspecified,
    #[serde(rename = "file")]
    File,
    #[serde(rename = "url")]
    Url,
}
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub source: String,
    pub provider: Provider,
    pub pixiv_only: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Input {
    pub kind: SourceKind,
    pub sha256: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Response {
    pub input: Input,
    pub providers: Option<Vec<ProviderSummary>>,
    pub results: Option<Vec<SearchResult>>,
    pub provider_errors: Option<Vec<ProviderError>>,
    pub partial: bool,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderStatus {
    Success,
    Error,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProviderSummary {
    pub name: Provider,
    pub status: ProviderStatus,
    pub result_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<Quota>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProviderError {
    pub provider: Provider,
    pub code: ErrorCode,
    pub message: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum PixivRefType {
    Artwork,
    User,
    Other(String),
}
impl PixivRefType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Artwork => "artwork",
            Self::User => "user",
            Self::Other(value) => value,
        }
    }
}
impl From<&str> for PixivRefType {
    fn from(value: &str) -> Self {
        match value {
            "artwork" => Self::Artwork,
            "user" => Self::User,
            value => Self::Other(value.to_owned()),
        }
    }
}
impl Serialize for PixivRefType {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for PixivRefType {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(Self::from(String::deserialize(deserializer)?.as_str()))
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash)]
pub struct PixivRef {
    #[serde(rename = "type")]
    pub kind: PixivRefType,
    pub id: i64,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Evidence {
    pub provider: Provider,
    pub rank: i64,
    #[serde(serialize_with = "serialize_similarity")]
    pub similarity: f64,
    pub index_id: i64,
    pub index_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_urls: Vec<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct SearchResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixiv: Option<PixivRef>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    pub evidence: Option<Vec<Evidence>>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Quota {
    pub short_remaining: i64,
    pub long_remaining: i64,
    pub short_limit: i64,
    pub long_limit: i64,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Match {
    pub rank: i64,
    #[serde(serialize_with = "serialize_similarity")]
    pub similarity: f64,
    pub index_id: i64,
    pub index_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub artwork_id: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub user_id: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_urls: Vec<String>,
}
fn serialize_similarity<S: serde::Serializer>(
    value: &f64,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    if !value.is_finite() {
        return Err(serde::ser::Error::custom(
            "reverse search similarity is not finite",
        ));
    }
    if value.fract() == 0.0
        && value.abs() <= 9_007_199_254_740_992.0
        && !(value.is_sign_negative() && *value == 0.0)
    {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}
fn is_zero(value: &i64) -> bool {
    *value == 0
}
#[derive(Clone, Debug, Default)]
pub struct ProviderResponse {
    pub provider: Provider,
    pub matches: Vec<Match>,
    pub quota: Option<Quota>,
}
#[derive(Clone, Debug, Default)]
pub struct SearchOutcome {
    pub response: Response,
    pub error: Option<Error>,
}
impl SearchOutcome {
    pub fn failure(error: Error) -> Self {
        Self {
            response: Response::default(),
            error: Some(error),
        }
    }
}
#[derive(Clone, Debug)]
pub struct PayloadQuery {
    pub provider: Provider,
    pub pixiv_only: bool,
}
#[derive(Clone, Debug)]
pub struct PayloadRequest {
    pub snapshot: Arc<Snapshot>,
    pub provider: Provider,
    pub pixiv_only: bool,
}
pub trait SourceLoader: Send + Sync {
    fn load<'a>(
        &'a self,
        context: CallerContext,
        source: &'a str,
    ) -> ReverseFuture<'a, std::result::Result<Arc<Snapshot>, Error>>;
}
pub trait Searcher: Send + Sync {
    fn search(&self, context: CallerContext, request: Request) -> ReverseFuture<'_, SearchOutcome>;
    fn close(&self) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}
pub trait PayloadSearcher: Send + Sync {
    fn preflight(
        &self,
        context: CallerContext,
        query: PayloadQuery,
    ) -> ReverseFuture<'_, std::result::Result<(), Error>>;
    fn search_payload(
        &self,
        context: CallerContext,
        request: PayloadRequest,
    ) -> ReverseFuture<'_, SearchOutcome>;
    fn close(&self) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}
pub trait ProviderClient: Send + Sync {
    fn preflight(
        &self,
        context: CallerContext,
    ) -> ReverseFuture<'_, std::result::Result<(), Error>>;
    fn search(
        &self,
        context: CallerContext,
        snapshot: Arc<Snapshot>,
    ) -> ReverseFuture<'_, std::result::Result<ProviderResponse, Error>>;
    fn close(&self) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}
pub trait ASCII2DClient: Send + Sync {
    fn preflight(
        &self,
        context: CallerContext,
    ) -> ReverseFuture<'_, std::result::Result<(), Error>>;
    fn upload(
        &self,
        context: CallerContext,
        snapshot: Arc<Snapshot>,
    ) -> ReverseFuture<'_, std::result::Result<Arc<dyn ASCII2DSession>, Error>>;
    fn close(&self) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}
pub trait ASCII2DSession: Send + Sync {
    fn search(
        &self,
        context: CallerContext,
        provider: Provider,
    ) -> ReverseFuture<'_, std::result::Result<ProviderResponse, Error>>;
}
