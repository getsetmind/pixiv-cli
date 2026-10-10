use crate::{
    config::{RuntimeConfig, Store},
    database::Database,
    fanbox_account::{Account, DefaultStore, Repository},
    lifecycle::Context,
    scheduler::SchedulerError,
    sessions::ClientOpen,
};
use pixiv_sdk::{
    Error, Reason,
    fanbox::{Client, CurrentUserRequest, FlareSolverrOptions, Options, SessionCredentials},
};
use serde::Serialize;
use std::sync::{Arc, Mutex};

pub type OptionsLoader = dyn Fn() -> Result<Options, SchedulerError> + Send + Sync;
pub type ClientOpener = dyn Fn(&Context) -> ClientOpen<Client> + Send + Sync;
pub type SessionOpener = dyn Fn(&str) -> ClientOpen<Client> + Send + Sync;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AccountSummary {
    #[serde(rename = "UserID")]
    pub user_id: i64,
    pub display_name: String,
    #[serde(rename = "CreatorID")]
    pub creator_id: String,
    pub default: bool,
}

pub struct AccountService {
    pub repository: Option<Arc<dyn Repository>>,
    pub defaults: Option<Arc<dyn DefaultStore>>,
    pub load_options: Option<Arc<OptionsLoader>>,
    pub open_client: Option<Arc<ClientOpener>>,
    pub open_session: Option<Arc<SessionOpener>>,
}
impl DefaultStore for Store {
    fn read(&self) -> Result<Option<i64>, SchedulerError> {
        self.read_fanbox_default_user_id().map_err(Into::into)
    }
    fn set(&self, user_id: i64) -> Result<(), SchedulerError> {
        self.set_fanbox_default_user_id(user_id).map_err(Into::into)
    }
    fn clear(&self) -> Result<(), SchedulerError> {
        self.clear_fanbox_default_user_id().map_err(Into::into)
    }
}

pub fn options_from_runtime(runtime: &RuntimeConfig) -> Options {
    Options {
        proxy_url: runtime
            .fanbox_network
            .proxy_url
            .as_ref()
            .unwrap_or(&runtime.https_proxy)
            .clone(),
        user_agent: runtime
            .fanbox_network
            .user_agent
            .clone()
            .unwrap_or_default(),
        flare_solverr: runtime
            .fanbox_flaresolverr
            .as_ref()
            .map(|solver| FlareSolverrOptions {
                url: solver.url.clone(),
                proxy_url: solver.proxy_url.clone(),
            }),
        ..Options::default()
    }
}

impl AccountService {
    pub fn new(
        repository: Option<Arc<dyn Repository>>,
        defaults: Option<Arc<dyn DefaultStore>>,
    ) -> Self {
        Self {
            repository,
            defaults,
            load_options: None,
            open_client: None,
            open_session: None,
        }
    }
    pub fn from_store(database: Arc<Mutex<Database>>, store: Arc<Store>) -> Self {
        let mut service = Self::new(Some(database), Some(store.clone()));
        service.load_options = Some(Arc::new(move || {
            Ok(options_from_runtime(&store.current()?.runtime()?))
        }));
        service
    }
    fn repository(&self) -> Result<&dyn Repository, SchedulerError> {
        self.repository
            .as_deref()
            .ok_or_else(|| message("fanbox account repository is not configured"))
    }
    fn read_default_user_id(&self) -> Result<Option<i64>, SchedulerError> {
        self.defaults
            .as_ref()
            .ok_or_else(|| message("fanbox default account store is not configured"))?
            .read()
    }
    fn set_default_user_id(&self, user_id: i64) -> Result<(), SchedulerError> {
        self.defaults
            .as_ref()
            .ok_or_else(|| message("fanbox default account store is not configured"))?
            .set(user_id)
    }
    fn is_default(&self, context: &Context, user_id: i64) -> Result<bool, SchedulerError> {
        if let Some(id) = self.read_default_user_id()? {
            return Ok(id == user_id);
        }
        Ok(self
            .repository()?
            .list(context)?
            .first()
            .is_some_and(|first| first.user_id == user_id))
    }
    pub async fn import_session(
        &self,
        context: &Context,
        session_value: &str,
        set_default: bool,
    ) -> Result<AccountSummary, SchedulerError> {
        self.import_session_with_proxy(context, session_value, set_default, None)
            .await
    }
    pub async fn import_session_bytes_with_proxy(
        &self,
        context: &Context,
        session_value: &[u8],
        set_default: bool,
        proxy_override: Option<&str>,
    ) -> Result<AccountSummary, SchedulerError> {
        let session_value = trim_session_space(session_value);
        if let Ok(session_value) = std::str::from_utf8(session_value) {
            return self
                .import_session_with_proxy(context, session_value, set_default, proxy_override)
                .await;
        }
        // Invalid Go strings cannot enter the String-only opener without changing the credential.
        let options = self.connection_options(proxy_override)?;
        match Client::open_session_bytes_with(session_value, options) {
            Err(error) => Err(error.into()),
            Ok(client) => {
                client.close_idle_connections();
                Err(message("FANBOX session input is not valid UTF-8"))
            }
        }
    }
    pub async fn import_session_with_proxy(
        &self,
        context: &Context,
        session_value: &str,
        set_default: bool,
        proxy_override: Option<&str>,
    ) -> Result<AccountSummary, SchedulerError> {
        let session_value = session_value.trim();
        if session_value.is_empty() {
            return Err(message("FANBOX session value is required"));
        }
        let opened = if let Some(open) = &self.open_session {
            open(session_value)
        } else {
            let options = self.connection_options(proxy_override)?;
            ClientOpen {
                client: Some(Arc::new(Client::open_with(
                    SessionCredentials {
                        fanbox_sessid: session_value.into(),
                    },
                    options,
                )?)),
                error: None,
            }
        };
        if let Some(error) = opened.error {
            return Err(error);
        }
        let client = opened
            .client
            .ok_or_else(|| message("fanbox session factory returned no client"))?;
        let _idle_connections = IdleConnections(client.as_ref());
        let user = client
            .current_user(Arc::new(context.clone()), CurrentUserRequest {})
            .await?;
        let mut account = Account::new(
            user.user_id,
            &user.display_name,
            &user.creator_id,
            session_value.as_bytes(),
        );
        account.credential_revision = 1;
        account.validated_at = chrono::Utc::now().timestamp();
        self.repository()?
            .save_credential(context, &account)
            .map_err(|source| wrapped("save fanbox account", source))?;
        let has_default = self
            .read_default_user_id()
            .map_err(|source| wrapped("read fanbox default account", source))?
            .is_some();
        if set_default || !has_default {
            self.set_default_user_id(user.user_id)
                .map_err(|source| wrapped("set fanbox default account", source))?;
        }
        let default = self.is_default(context, user.user_id)?;
        Ok(AccountSummary {
            user_id: user.user_id,
            display_name: user.display_name,
            creator_id: user.creator_id,
            default,
        })
    }
    pub fn use_account(&self, context: &Context, user_id: i64) -> Result<(), SchedulerError> {
        self.repository()?.get(context, user_id)?;
        self.set_default_user_id(user_id)
    }
    pub fn use_auto(&self) -> Result<(), SchedulerError> {
        self.defaults
            .as_ref()
            .ok_or_else(|| message("fanbox default account store is not configured"))?
            .clear()
    }
    pub fn remove_account(&self, context: &Context, user_id: i64) -> Result<(), SchedulerError> {
        let default = self
            .read_default_user_id()
            .map_err(|source| wrapped("read fanbox default account", source))?;
        if default == Some(user_id) {
            return Err(message(&format!(
                "cannot remove fanbox account {user_id} while it is the explicit default; use `pixiv fanbox auth use --auto` or select another account first"
            )));
        }
        self.repository()?.remove(context, user_id)
    }
    pub fn selected_user_id(&self, context: &Context) -> Result<i64, SchedulerError> {
        if let Some(id) = self.read_default_user_id()? {
            self.repository()?.get(context, id).map_err(|_| {
                message(&format!(
                    "configured default fanbox account {id} is missing"
                ))
            })?;
            return Ok(id);
        }
        self.repository()?
            .list(context)?
            .first()
            .map(|account| account.user_id)
            .ok_or_else(|| {
                Error::with_product("fanbox", Reason::Unauthorized, "")
                    .with_detail("no fanbox account is authenticated")
                    .into()
            })
    }
    pub fn list_accounts(&self, context: &Context) -> Result<Vec<AccountSummary>, SchedulerError> {
        let accounts = self.repository()?.list(context)?;
        accounts
            .into_iter()
            .map(|account| {
                let default = self.is_default(context, account.user_id)?;
                Ok(AccountSummary {
                    user_id: account.user_id,
                    display_name: account.display_name,
                    creator_id: account.creator_id,
                    default,
                })
            })
            .collect()
    }
    pub fn status(&self, context: &Context) -> Result<AccountSummary, SchedulerError> {
        let id = self.selected_user_id(context)?;
        let account = self.repository()?.get(context, id)?;
        Ok(AccountSummary {
            user_id: account.user_id,
            display_name: account.display_name,
            creator_id: account.creator_id,
            default: true,
        })
    }
    pub fn open_client_with_proxy(
        &self,
        context: &Context,
        proxy_override: Option<&str>,
    ) -> ClientOpen<Client> {
        if let Some(open) = &self.open_client {
            return open(context);
        }
        match self.open_selected(context, proxy_override) {
            Ok(client) => ClientOpen {
                client: Some(Arc::new(client)),
                error: None,
            },
            Err(error) => ClientOpen {
                client: None,
                error: Some(error),
            },
        }
    }
    fn open_selected(
        &self,
        context: &Context,
        proxy_override: Option<&str>,
    ) -> Result<Client, SchedulerError> {
        let id = self.selected_user_id(context)?;
        let account =
            self.repository()?
                .get(context, id)
                .map_err(|source| SchedulerError::Wrapped {
                    message: "select fanbox account".into(),
                    source: Box::new(source),
                })?;
        let options = self.connection_options(proxy_override)?;
        Client::open_session_bytes_with(&account.session_id_copy(), options).map_err(Into::into)
    }
    fn connection_options(&self, proxy_override: Option<&str>) -> Result<Options, SchedulerError> {
        let mut options = self
            .load_options
            .as_ref()
            .map(|load| load())
            .transpose()?
            .unwrap_or_default();
        if let Some(proxy) = proxy_override {
            options.proxy_url = proxy.to_owned();
        }
        Ok(options)
    }
}
fn trim_session_space(mut bytes: &[u8]) -> &[u8] {
    while let Some(length) = session_edge_space(bytes, true) {
        bytes = &bytes[length..];
    }
    while let Some(length) = session_edge_space(bytes, false) {
        bytes = &bytes[..bytes.len() - length];
    }
    bytes
}
fn session_edge_space(bytes: &[u8], front: bool) -> Option<usize> {
    for length in 1..=bytes.len().min(4) {
        let edge = if front {
            &bytes[..length]
        } else {
            &bytes[bytes.len() - length..]
        };
        if let Ok(value) = std::str::from_utf8(edge) {
            return value
                .chars()
                .next()
                .filter(|value| value.is_whitespace())
                .map(|_| length);
        }
    }
    None
}
struct IdleConnections<'a>(&'a Client);
impl Drop for IdleConnections<'_> {
    fn drop(&mut self) {
        self.0.close_idle_connections();
    }
}
fn wrapped(value: &str, source: SchedulerError) -> SchedulerError {
    SchedulerError::Wrapped {
        message: value.into(),
        source: Box::new(source),
    }
}
fn message(value: &str) -> SchedulerError {
    SchedulerError::Message(value.into())
}
