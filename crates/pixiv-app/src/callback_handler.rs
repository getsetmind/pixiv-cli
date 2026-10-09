use crate::{
    config::{ConfigError, private_file},
    handoff_client::{
        HandoffClient, HandoffClientError, HandoffFuture, HandoffTransport, RemoteCallbackSession,
        go_trim,
    },
    handoff_protocol::{RemoteLoginStart, is_allowed_pixiv_callback_url, parse_remote_login_link},
    handoff_state::{HandoffState, HandoffStateError},
    login_input::{equal_fold_ascii, split_host_port},
};
use pixiv_sdk::oauth::LoginUrl;
use std::{
    error::Error,
    fmt, fs, io,
    net::IpAddr,
    path::{Component, Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

pub type CallbackError = Box<dyn Error + Send + Sync>;
pub type CallbackResult<T> = Result<T, CallbackError>;

pub fn user_home_directory() -> io::Result<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::other(if cfg!(windows) {
                "%USERPROFILE% is not defined"
            } else {
                "$HOME is not defined"
            })
        })?;
    Ok(PathBuf::from(home))
}

pub fn app_data_directory() -> io::Result<PathBuf> {
    Ok(clean_native_path(
        &user_home_directory()?.join(".pixiv-cli"),
    ))
}

pub(crate) fn clean_native_path(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if cleaned.file_name().is_some_and(|name| name != "..") {
                    cleaned.pop();
                } else if !cleaned.has_root() {
                    cleaned.push("..");
                }
            }
            other => cleaned.push(other.as_os_str()),
        }
    }
    cleaned
}

pub fn callback_endpoint_path() -> io::Result<PathBuf> {
    Ok(app_data_directory()?.join("url-handler-endpoint"))
}
pub fn active_remote_login_path() -> io::Result<PathBuf> {
    Ok(app_data_directory()?.join("remote-login-session.json"))
}
pub fn handler_manifest_path() -> io::Result<PathBuf> {
    Ok(app_data_directory()?
        .join("url-handler")
        .join("handler-manifest.json"))
}

#[derive(Debug)]
pub enum CallbackEndpointError {
    NoActiveLocalCallback,
    InvalidEndpoint,
    NonLoopback,
    InvalidCallback,
    Unreadable,
    InvalidStoredEndpoint,
    Storage(ConfigError),
}
impl fmt::Display for CallbackEndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoActiveLocalCallback => "Pixiv login callback is no longer active",
            Self::InvalidEndpoint => "invalid callback endpoint",
            Self::NonLoopback => "callback endpoint must be loopback",
            Self::InvalidCallback => "invalid Pixiv callback URL",
            Self::Unreadable => "could not read Pixiv login callback endpoint",
            Self::InvalidStoredEndpoint => "Pixiv login callback endpoint is invalid",
            Self::Storage(error) => return error.fmt(f),
        })
    }
}
impl Error for CallbackEndpointError {}

pub fn validated_callback_endpoint(raw: &str) -> Result<String, CallbackEndpointError> {
    let parsed = LoginUrl::parse(go_trim(raw)).ok_or(CallbackEndpointError::InvalidEndpoint)?;
    if !equal_fold_ascii(parsed.scheme(), "http")
        || parsed.host().is_empty()
        || parsed.path() != "/callback"
        || !parsed.raw_query().is_empty()
        || !parsed.fragment().is_empty()
    {
        return Err(CallbackEndpointError::InvalidEndpoint);
    }
    let (host, _) =
        split_host_port(parsed.host()).map_err(|_| CallbackEndpointError::InvalidEndpoint)?;
    let loopback = equal_fold_ascii(host, "localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| match ip {
            IpAddr::V4(ip) => ip.is_loopback(),
            IpAddr::V6(ip) => {
                ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
            }
        });
    if !loopback {
        return Err(CallbackEndpointError::NonLoopback);
    }
    Ok(parsed.with_fragment(""))
}

pub(crate) struct ValidatedCallbackEndpoint(String);

impl ValidatedCallbackEndpoint {
    pub(crate) fn parse(raw: &str) -> Result<Self, CallbackEndpointError> {
        validated_callback_endpoint(raw).map(Self)
    }
}

pub trait CallbackEndpointStore: Send + Sync {
    fn local_relay_url(&self, raw_callback: &str) -> CallbackResult<String>;
}
#[derive(Clone, Debug)]
pub struct FileCallbackEndpointStore {
    path: PathBuf,
}
impl FileCallbackEndpointStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn from_default_path() -> io::Result<Self> {
        Ok(Self::new(callback_endpoint_path()?))
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn write(&self, endpoint: &str) -> Result<&Path, CallbackEndpointError> {
        self.write_validated(&ValidatedCallbackEndpoint::parse(endpoint)?)
    }
    pub(crate) fn write_validated(
        &self,
        endpoint: &ValidatedCallbackEndpoint,
    ) -> Result<&Path, CallbackEndpointError> {
        private_file::write(&self.path, format!("{}\n", endpoint.0).as_bytes())
            .map_err(CallbackEndpointError::Storage)?;
        Ok(&self.path)
    }
}
impl CallbackEndpointStore for FileCallbackEndpointStore {
    fn local_relay_url(&self, raw_callback: &str) -> CallbackResult<String> {
        if !is_allowed_pixiv_callback_url(raw_callback) {
            return Err(Box::new(CallbackEndpointError::InvalidCallback));
        }
        let body = fs::read(&self.path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                CallbackEndpointError::NoActiveLocalCallback
            } else {
                CallbackEndpointError::Unreadable
            }
        })?;
        let endpoint = validated_callback_endpoint(&String::from_utf8_lossy(&body))
            .map_err(|_| CallbackEndpointError::InvalidStoredEndpoint)?;
        let parsed =
            LoginUrl::parse(&endpoint).ok_or(CallbackEndpointError::InvalidStoredEndpoint)?;
        Ok(parsed.with_fragment(raw_callback))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultCallbackEndpointStore;
impl CallbackEndpointStore for DefaultCallbackEndpointStore {
    fn local_relay_url(&self, raw_callback: &str) -> CallbackResult<String> {
        if !is_allowed_pixiv_callback_url(raw_callback) {
            return Err(Box::new(CallbackEndpointError::InvalidCallback));
        }
        FileCallbackEndpointStore::from_default_path()?.local_relay_url(raw_callback)
    }
}

pub trait PreviousHandler: Send + Sync {
    fn delegate<'a>(
        &'a self,
        raw_url: &'a str,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<()>>;
}
pub trait BrowserOpener: Send + Sync {
    fn open(&self, url: &str) -> CallbackResult<()>;
}
pub trait RemoteCallback: Send + Sync {
    fn result_url(&self) -> &str;
    fn complete(&self) -> HandoffFuture<'_, CallbackResult<()>>;
    fn abort(&self);
}
impl RemoteCallback for RemoteCallbackSession {
    fn result_url(&self) -> &str {
        &self.result_url
    }
    fn complete(&self) -> HandoffFuture<'_, CallbackResult<()>> {
        Box::pin(async move {
            RemoteCallbackSession::complete(self)
                .await
                .map_err(|error| Box::new(error) as CallbackError)
        })
    }
    fn abort(&self) {
        RemoteCallbackSession::abort(self);
    }
}
pub trait RemoteLoginHandoff: Send + Sync {
    type Session: RemoteCallback;
    fn start<'a>(
        &'a self,
        start: &'a RemoteLoginStart,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<String>>;
    fn forward_callback<'a>(
        &'a self,
        raw_url: &'a str,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<Self::Session>>;
    fn clear(&self, start: &RemoteLoginStart) -> CallbackResult<()>;
}
pub struct ClientHandoff<T, S = HandoffState> {
    client: HandoffClient<T, S>,
    state: S,
}
impl<T: HandoffTransport, S: crate::handoff_state::HandoffStateStore> ClientHandoff<T, S> {
    pub fn new(transport: T, state: S) -> Self {
        Self {
            client: HandoffClient::new(transport, state.clone()),
            state,
        }
    }
}
impl<T: HandoffTransport, S: crate::handoff_state::HandoffStateStore> RemoteLoginHandoff
    for ClientHandoff<T, S>
{
    type Session = RemoteCallbackSession;
    fn start<'a>(
        &'a self,
        start: &'a RemoteLoginStart,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<String>> {
        Box::pin(async move {
            self.client
                .start(start, cancellation)
                .await
                .map_err(|error| Box::new(error) as CallbackError)
        })
    }
    fn forward_callback<'a>(
        &'a self,
        raw_url: &'a str,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<Self::Session>> {
        Box::pin(async move {
            self.client
                .forward_callback(raw_url, cancellation)
                .await
                .map_err(|error| Box::new(error) as CallbackError)
        })
    }
    fn clear(&self, start: &RemoteLoginStart) -> CallbackResult<()> {
        self.state
            .clear_remote_login_handoff(start)
            .map_err(|error| Box::new(error) as CallbackError)
    }
}

pub enum CallbackHandlingResult<S> {
    Delegated,
    LocalRelay(String),
    RemoteCallback(S),
    RemoteLoginStart(RemoteLoginStart),
}

fn message(value: &'static str) -> CallbackError {
    Box::new(io::Error::other(value))
}
fn inactive_local(mut error: &(dyn Error + 'static)) -> bool {
    loop {
        if matches!(
            error.downcast_ref::<CallbackEndpointError>(),
            Some(CallbackEndpointError::NoActiveLocalCallback)
        ) {
            return true;
        }
        match error.source() {
            Some(source) => error = source,
            None => return false,
        }
    }
}
fn inactive_remote(mut error: &(dyn Error + 'static)) -> bool {
    loop {
        if matches!(
            error.downcast_ref::<HandoffStateError>(),
            Some(HandoffStateError::NoActiveRemoteLogin)
        ) || matches!(
            error.downcast_ref::<HandoffClientError>(),
            Some(HandoffClientError::State(
                HandoffStateError::NoActiveRemoteLogin
            ))
        ) {
            return true;
        }
        match error.source() {
            Some(source) => error = source,
            None => return false,
        }
    }
}

pub async fn handle_callback<
    E: CallbackEndpointStore,
    H: RemoteLoginHandoff,
    P: PreviousHandler,
>(
    raw_url: &str,
    cancellation: &CancellationToken,
    endpoint: &E,
    handoff: &H,
    previous: &P,
) -> CallbackResult<CallbackHandlingResult<H::Session>> {
    let parsed =
        LoginUrl::parse(go_trim(raw_url)).ok_or_else(|| message("invalid Pixiv login link"))?;
    if !equal_fold_ascii(parsed.scheme(), "pixiv") {
        return Err(message("invalid Pixiv login link"));
    }
    if parsed.path() == "/remote-login" {
        return Ok(CallbackHandlingResult::RemoteLoginStart(
            parse_remote_login_link(raw_url)?,
        ));
    }
    if !is_allowed_pixiv_callback_url(raw_url) {
        previous.delegate(raw_url, cancellation).await?;
        return Ok(CallbackHandlingResult::Delegated);
    }
    match endpoint.local_relay_url(raw_url) {
        Ok(url) => return Ok(CallbackHandlingResult::LocalRelay(url)),
        Err(error) if inactive_local(error.as_ref()) => {}
        Err(error) => return Err(error),
    }
    match handoff.forward_callback(raw_url, cancellation).await {
        Ok(session) => return Ok(CallbackHandlingResult::RemoteCallback(session)),
        Err(error) if inactive_remote(error.as_ref()) => {}
        Err(error) => return Err(error),
    }
    previous.delegate(raw_url, cancellation).await?;
    Ok(CallbackHandlingResult::Delegated)
}

pub async fn run_callback<
    E: CallbackEndpointStore,
    H: RemoteLoginHandoff,
    P: PreviousHandler,
    B: BrowserOpener,
>(
    raw_url: &str,
    cancellation: &CancellationToken,
    endpoint: &E,
    handoff: &H,
    previous: &P,
    browser: &B,
) -> CallbackResult<()> {
    match handle_callback(raw_url, cancellation, endpoint, handoff, previous).await? {
        CallbackHandlingResult::RemoteLoginStart(start) => {
            let url = handoff.start(&start, cancellation).await?;
            if browser.open(&url).is_err() {
                if handoff.clear(&start).is_err() {
                    return Err(message(
                        "could not clear remote Pixiv login handoff after browser launch failed",
                    ));
                }
                return Err(message("could not open Pixiv authorization page"));
            }
        }
        CallbackHandlingResult::LocalRelay(url) => {
            if !url.is_empty() && browser.open(&url).is_err() {
                return Err(message("could not open Pixiv login callback bridge"));
            }
        }
        CallbackHandlingResult::RemoteCallback(session) => {
            if browser.open(session.result_url()).is_err() {
                session.abort();
                return Err(message("could not open Pixiv login result page"));
            }
            session.complete().await?;
        }
        CallbackHandlingResult::Delegated => {}
    }
    Ok(())
}
