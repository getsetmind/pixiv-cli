use crate::{
    CommandError,
    fanbox_auth::{BrowserBytesFuture, BrowserFuture, BrowserProvider},
};
use pixiv_app::lifecycle::Context;
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

pub struct CookieProfile {
    pub id: String,
    pub path: PathBuf,
}

pub struct CookieByteProfile {
    pub id: Vec<u8>,
    pub path: PathBuf,
}
pub type BrowserByteProfilesFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<CookieByteProfile>, CommandError>> + Send + 'a>>;
pub type BrowserProfilesFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<CookieProfile>, CommandError>> + Send + 'a>>;
pub type BrowserCookiesFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<String>, CommandError>> + Send + 'a>>;
pub type BrowserCookieBytesFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Vec<u8>>, CommandError>> + Send + 'a>>;

pub trait CookieProvider: Send + Sync {
    fn discover_profiles<'a>(&'a self, context: &'a Context) -> BrowserProfilesFuture<'a>;
    fn read<'a>(
        &'a self,
        context: &'a Context,
        host: &'a str,
        name: &'a str,
        profile: &'a str,
    ) -> BrowserCookiesFuture<'a>;
    fn read_bytes<'a>(
        &'a self,
        context: &'a Context,
        host: &'a str,
        name: &'a str,
        profile: &'a str,
    ) -> BrowserCookieBytesFuture<'a> {
        Box::pin(async move {
            self.read(context, host, name, profile)
                .await
                .map(|values| values.into_iter().map(String::into_bytes).collect())
        })
    }
    fn discover_byte_profiles<'a>(&'a self, context: &'a Context) -> BrowserByteProfilesFuture<'a> {
        Box::pin(async move {
            self.discover_profiles(context).await.map(|profiles| {
                profiles
                    .into_iter()
                    .map(|profile| CookieByteProfile {
                        id: profile.id.into_bytes(),
                        path: profile.path,
                    })
                    .collect()
            })
        })
    }
    fn read_profile_bytes<'a>(
        &'a self,
        context: &'a Context,
        host: &'a str,
        name: &'a str,
        profile: &'a [u8],
    ) -> BrowserCookieBytesFuture<'a> {
        Box::pin(async move {
            let profile = std::str::from_utf8(profile)
                .map_err(|_| CommandError::Message("browsercookies: invalid profile identifier"))?;
            self.read_bytes(context, host, name, profile).await
        })
    }
    fn close(&self) -> Result<(), CommandError>;
}

type ProviderFactory = dyn Fn(&str) -> Result<Arc<dyn CookieProvider>, CommandError> + Send + Sync;

#[derive(Clone)]
pub struct SystemBrowserProvider {
    factory: Arc<ProviderFactory>,
}
impl SystemBrowserProvider {
    pub fn with_factory(factory: Arc<ProviderFactory>) -> Self {
        Self { factory }
    }

    pub fn system() -> Self {
        Self::with_factory(Arc::new(|browser| {
            let backend = pixiv_app::browser_cookies::BrowserCookieBackend::system(browser)
                .map_err(browser_error)?;
            Ok(Arc::new(NativeCookieProvider::new(Arc::new(backend))))
        }))
    }
}
impl BrowserProvider for SystemBrowserProvider {
    fn read_session_bytes<'a>(
        &'a self,
        context: &'a Context,
        browser: &'a str,
        profile: &'a str,
    ) -> BrowserBytesFuture<'a> {
        Box::pin(async move {
            let browser = browser
                .trim()
                .chars()
                .map(|character| character.to_lowercase().next().unwrap())
                .collect::<String>();
            let provider = (self.factory)(&browser)?;
            let close = ProviderClose::new(provider.clone());
            let result = read_session_bytes(provider.as_ref(), context, profile).await;
            close.finish(result)
        })
    }
    fn read_session<'a>(
        &'a self,
        context: &'a Context,
        browser: &'a str,
        profile: &'a str,
    ) -> BrowserFuture<'a> {
        Box::pin(async move {
            // Go's registry excludes multi-character lowercase expansions.
            let browser = browser
                .trim()
                .chars()
                .map(|character| character.to_lowercase().next().unwrap())
                .collect::<String>();
            let provider = (self.factory)(&browser)?;
            let close = ProviderClose::new(provider.clone());
            let result = read_session(provider.as_ref(), context, profile).await;
            close.finish(result)
        })
    }
}

async fn read_session_bytes(
    provider: &dyn CookieProvider,
    context: &Context,
    profile_id: &str,
) -> Result<Vec<u8>, CommandError> {
    let profiles = provider.discover_byte_profiles(context).await?;
    let profile = if profile_id.is_empty() {
        match profiles.as_slice() {
            [] => return Err(CommandError::Message("browsercookies: profile not found")),
            [profile] => profile,
            _ => {
                return Err(CommandError::MessageText(format!(
                    "browsercookies: multiple profiles match and no profile was specified: {}",
                    profiles
                        .iter()
                        .map(|profile| String::from_utf8_lossy(&profile.id))
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
    } else {
        profiles
            .iter()
            .find(|profile| profile.id == profile_id.as_bytes())
            .ok_or(CommandError::Message("browsercookies: profile not found"))?
    };
    let mut cookies = provider
        .read_profile_bytes(context, ".fanbox.cc", "FANBOXSESSID", &profile.id)
        .await?;
    match cookies.len() {
        0 => Err(CommandError::Message(
            "browser profile does not contain a FANBOXSESSID cookie",
        )),
        1 => Ok(cookies.pop().unwrap()),
        _ => Err(CommandError::Message(
            "browser profile contains multiple FANBOXSESSID cookies",
        )),
    }
}

async fn read_session(
    provider: &dyn CookieProvider,
    context: &Context,
    profile_id: &str,
) -> Result<String, CommandError> {
    let profiles = provider.discover_profiles(context).await?;
    let profile = select_profile(&profiles, profile_id)?;
    let mut cookies = provider
        .read(context, ".fanbox.cc", "FANBOXSESSID", &profile.id)
        .await?;
    match cookies.len() {
        0 => Err(CommandError::Message(
            "browser profile does not contain a FANBOXSESSID cookie",
        )),
        1 => Ok(cookies.pop().unwrap()),
        _ => Err(CommandError::Message(
            "browser profile contains multiple FANBOXSESSID cookies",
        )),
    }
}

fn select_profile<'a>(
    profiles: &'a [CookieProfile],
    profile_id: &str,
) -> Result<&'a CookieProfile, CommandError> {
    if profile_id.is_empty() {
        match profiles {
            [] => Err(CommandError::Message("browsercookies: profile not found")),
            [profile] => Ok(profile),
            _ => Err(CommandError::MessageText(format!(
                "browsercookies: multiple profiles match and no profile was specified: {}",
                profiles
                    .iter()
                    .map(|profile| profile.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    } else {
        profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .ok_or(CommandError::Message("browsercookies: profile not found"))
    }
}

struct ProviderClose {
    provider: Option<Arc<dyn CookieProvider>>,
}
impl ProviderClose {
    fn new(provider: Arc<dyn CookieProvider>) -> Self {
        Self {
            provider: Some(provider),
        }
    }

    fn finish<T>(mut self, result: Result<T, CommandError>) -> Result<T, CommandError> {
        let close = self.provider.take().unwrap().close();
        match (result, close) {
            (result, Ok(())) => result,
            (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(close)) => Err(CommandError::Joined(vec![error, close])),
        }
    }
}
impl Drop for ProviderClose {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.close();
        }
    }
}

fn browser_error(error: pixiv_app::browser_cookies::BrowserCookieError) -> CommandError {
    CommandError::MessageText(error.to_string())
}

pub struct NativeCookieProvider {
    backend: Arc<pixiv_app::browser_cookies::BrowserCookieBackend>,
}
impl NativeCookieProvider {
    pub fn new(backend: Arc<pixiv_app::browser_cookies::BrowserCookieBackend>) -> Self {
        Self { backend }
    }
}
async fn native_job<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError> {
    match tokio::task::spawn_blocking(operation).await {
        Ok(result) => result,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(_) => Err(CommandError::Message(
            "browsercookies: cookie database query failed",
        )),
    }
}
impl CookieProvider for NativeCookieProvider {
    fn discover_profiles<'a>(&'a self, context: &'a Context) -> BrowserProfilesFuture<'a> {
        Box::pin(async move {
            self.discover_byte_profiles(context)
                .await?
                .into_iter()
                .map(|profile| {
                    Ok(CookieProfile {
                        id: String::from_utf8(profile.id).map_err(|_| {
                            CommandError::Message("browsercookies: invalid profile identifier")
                        })?,
                        path: profile.path,
                    })
                })
                .collect()
        })
    }
    fn discover_byte_profiles<'a>(&'a self, context: &'a Context) -> BrowserByteProfilesFuture<'a> {
        let backend = self.backend.clone();
        let context = context.clone();
        Box::pin(native_job(move || {
            backend
                .discover_profiles(&context)
                .map(|profiles| {
                    profiles
                        .into_iter()
                        .map(|profile| CookieByteProfile {
                            id: profile.id,
                            path: profile.path,
                        })
                        .collect()
                })
                .map_err(browser_error)
        }))
    }
    fn read<'a>(
        &'a self,
        context: &'a Context,
        host: &'a str,
        name: &'a str,
        profile: &'a str,
    ) -> BrowserCookiesFuture<'a> {
        Box::pin(async move {
            self.read_bytes(context, host, name, profile).await?.into_iter().map(|value| String::from_utf8(value).map_err(|_| CommandError::Message("browsercookies: cookie value is encrypted and decryption is not supported by this provider"))).collect()
        })
    }
    fn read_bytes<'a>(
        &'a self,
        context: &'a Context,
        host: &'a str,
        name: &'a str,
        profile: &'a str,
    ) -> BrowserCookieBytesFuture<'a> {
        self.read_profile_bytes(context, host, name, profile.as_bytes())
    }
    fn read_profile_bytes<'a>(
        &'a self,
        context: &'a Context,
        host: &'a str,
        name: &'a str,
        profile: &'a [u8],
    ) -> BrowserCookieBytesFuture<'a> {
        let backend = self.backend.clone();
        let context = context.clone();
        let query = pixiv_app::browser_cookies::CookieQuery::new(host, name);
        let profile = profile.to_vec();
        Box::pin(native_job(move || {
            backend
                .read(&context, &query.map_err(browser_error)?, &profile)
                .map(|values| values.into_iter().map(|value| value.into_bytes()).collect())
                .map_err(browser_error)
        }))
    }
    fn close(&self) -> Result<(), CommandError> {
        self.backend.close().map_err(browser_error)
    }
}
