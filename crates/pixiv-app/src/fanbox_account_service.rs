use crate::{
    config::{RuntimeConfig, Store},
    database::Database,
    fanbox_account::{DefaultStore, Repository},
    lifecycle::Context,
    scheduler::SchedulerError,
    sessions::ClientOpen,
};
use pixiv_sdk::{
    Error, Reason,
    fanbox::{Client, FlareSolverrOptions, Options, SessionCredentials},
};
use serde::Serialize;
use std::sync::{Arc, Mutex};

pub type OptionsLoader = dyn Fn() -> Result<Options, SchedulerError> + Send + Sync;
pub type ClientOpener = dyn Fn(&Context) -> ClientOpen<Client> + Send + Sync;

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
}
impl DefaultStore for Store {
    fn read(&self) -> Result<Option<i64>, SchedulerError> {
        self.read_fanbox_default_user_id().map_err(Into::into)
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
                let selected = self.read_default_user_id()?;
                let default = if let Some(id) = selected {
                    id == account.user_id
                } else {
                    self.repository()?
                        .list(context)?
                        .first()
                        .is_some_and(|first| first.user_id == account.user_id)
                };
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
        let mut options = self
            .load_options
            .as_ref()
            .map(|load| load())
            .transpose()?
            .unwrap_or_default();
        if let Some(proxy) = proxy_override {
            options.proxy_url = proxy.to_owned();
        }
        Client::open_with(
            SessionCredentials {
                fanbox_sessid: String::from_utf8_lossy(&account.session_id_copy()).into_owned(),
            },
            options,
        )
        .map_err(Into::into)
    }
}
fn message(value: &str) -> SchedulerError {
    SchedulerError::Message(value.into())
}
