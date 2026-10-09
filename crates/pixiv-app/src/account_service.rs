use crate::{
    database::{Database, PixivAccount, PoolStatus},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    Client, Error, Reason,
    error::{Cause, TransportKind},
    oauth,
    transport::Transport,
};
use std::sync::{Arc, Mutex};

pub use crate::account_views::AccountSummary;

pub trait AccountRepository: Send + Sync {
    fn save_credential(
        &self,
        _context: &Context,
        _account: &PixivAccount,
    ) -> Result<(), SchedulerError> {
        Err(SchedulerError::Message(
            "pixiv credential repository is not configured".into(),
        ))
    }
    fn save_credentials(
        &self,
        _context: &Context,
        _accounts: &[PixivAccount],
    ) -> Result<(), SchedulerError> {
        Err(SchedulerError::Message(
            "pixiv credential repository is not configured".into(),
        ))
    }

    fn remove(&self, _context: &Context, _user_id: i64) -> Result<(), SchedulerError> {
        Err(SchedulerError::Message(
            "pixiv account management repository is not configured".into(),
        ))
    }

    fn pool_status(&self, _context: &Context, _now: i64) -> Result<PoolStatus, SchedulerError> {
        Err(SchedulerError::Message(
            "pixiv account pool repository is not configured".into(),
        ))
    }
    fn set_pool_schedulable(
        &self,
        _context: &Context,
        _ids: &[i64],
        _enabled: bool,
    ) -> Result<(), SchedulerError> {
        Err(SchedulerError::Message(
            "pixiv account pool repository is not configured".into(),
        ))
    }
    fn set_all_pool_schedulable(
        &self,
        _context: &Context,
        _enabled: bool,
    ) -> Result<(), SchedulerError> {
        Err(SchedulerError::Message(
            "pixiv account pool repository is not configured".into(),
        ))
    }
    fn update_metadata(
        &self,
        _context: &Context,
        _user_id: i64,
        _username: &str,
        _premium: Option<bool>,
        _checked_at: Option<i64>,
    ) -> Result<(), SchedulerError> {
        Err(SchedulerError::Message(
            "pixiv account metadata repository is not configured".into(),
        ))
    }
    fn get(&self, context: &Context, user_id: i64) -> Result<PixivAccount, SchedulerError>;
    fn list(&self, context: &Context) -> Result<Vec<PixivAccount>, SchedulerError>;
    fn rotate(
        &self,
        context: &Context,
        user_id: i64,
        revision: i64,
        refresh_token: &[u8],
    ) -> Result<(), SchedulerError>;
}

impl AccountRepository for Mutex<Database> {
    fn save_credential(
        &self,
        context: &Context,
        account: &PixivAccount,
    ) -> Result<(), SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .save_pixiv_credential(account)
            .map_err(SchedulerError::Account)
    }
    fn save_credentials(
        &self,
        context: &Context,
        accounts: &[PixivAccount],
    ) -> Result<(), SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .save_pixiv_credentials(accounts)
            .map_err(SchedulerError::Account)
    }

    fn remove(&self, context: &Context, user_id: i64) -> Result<(), SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove_pixiv(user_id)
            .map_err(SchedulerError::Account)
    }

    fn pool_status(&self, context: &Context, now: i64) -> Result<PoolStatus, SchedulerError> {
        let mut database = self.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        database
            .list_pixiv_pool_status_with_context(context, now)
            .map_err(Into::into)
    }
    fn set_pool_schedulable(
        &self,
        context: &Context,
        ids: &[i64],
        enabled: bool,
    ) -> Result<(), SchedulerError> {
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_pixiv_schedulable_with_context(context, ids, enabled)
            .map_err(Into::into)
    }
    fn set_all_pool_schedulable(
        &self,
        context: &Context,
        enabled: bool,
    ) -> Result<(), SchedulerError> {
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_all_pixiv_schedulable_with_context(context, enabled)
            .map_err(Into::into)
    }

    fn update_metadata(
        &self,
        context: &Context,
        user_id: i64,
        username: &str,
        premium: Option<bool>,
        checked_at: Option<i64>,
    ) -> Result<(), SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .update_pixiv_metadata(user_id, username, premium, checked_at)
            .map_err(SchedulerError::Account)
    }

    fn get(&self, context: &Context, user_id: i64) -> Result<PixivAccount, SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_pixiv(user_id)
            .map_err(SchedulerError::Account)
    }
    fn list(&self, context: &Context) -> Result<Vec<PixivAccount>, SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .list_pixiv()
            .map_err(SchedulerError::Account)
    }
    fn rotate(
        &self,
        context: &Context,
        user_id: i64,
        revision: i64,
        refresh_token: &[u8],
    ) -> Result<(), SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .rotate_pixiv_credentials(user_id, revision, refresh_token)
            .map_err(SchedulerError::Account)
    }
}

pub type DefaultAccountReader = dyn Fn() -> Result<Option<i64>, SchedulerError> + Send + Sync;

pub struct AccountService {
    pub repository: Arc<dyn AccountRepository>,
    pub defaults: Option<Arc<DefaultAccountReader>>,
}

fn wrapped(message: &str, source: SchedulerError) -> SchedulerError {
    SchedulerError::Wrapped {
        message: message.to_owned(),
        source: Box::new(source),
    }
}

impl AccountService {
    pub fn selected_user_id(&self, context: &Context) -> Result<i64, SchedulerError> {
        let defaults = self.defaults.as_ref().ok_or_else(|| {
            SchedulerError::Message("pixiv default account store is not configured".to_owned())
        })?;
        if let Some(id) = defaults()? {
            self.repository.get(context, id).map_err(|_| {
                SchedulerError::Message(format!("configured default pixiv account {id} is missing"))
            })?;
            return Ok(id);
        }
        self.repository
            .list(context)?
            .first()
            .map(|account| account.user_id)
            .ok_or_else(|| {
                Error::new(Reason::Unauthorized, "auth")
                    .with_detail("no pixiv account is authenticated")
                    .into()
            })
    }

    pub async fn open<T: Transport>(
        &self,
        context: &Context,
        user_id: i64,
        transport: T,
    ) -> Result<Client<T>, SchedulerError> {
        let user_id = if user_id == 0 {
            self.selected_user_id(context)?
        } else {
            user_id
        };
        self.open_account(context, user_id, transport).await
    }

    pub async fn open_account<T: Transport>(
        &self,
        context: &Context,
        user_id: i64,
        transport: T,
    ) -> Result<Client<T>, SchedulerError> {
        let account = self
            .repository
            .get(context, user_id)
            .map_err(|error| wrapped("select pixiv account", error))?;
        let credentials = self
            .rotate_account(context, user_id, &account, &transport)
            .await?;
        Ok(Client::from_credentials(&credentials, transport))
    }

    pub(crate) async fn rotate_account<T: Transport>(
        &self,
        context: &Context,
        user_id: i64,
        account: &PixivAccount,
        transport: &T,
    ) -> Result<oauth::Credentials, SchedulerError> {
        let token = account.refresh_token_copy();
        let token = String::from_utf8_lossy(&token);
        let credentials = tokio::select! {
            result=oauth::refresh(transport,&token)=>result?,
            error=context.cancelled()=>{
                let cause=match error {crate::lifecycle::ContextError::Canceled=>Cause::Canceled,crate::lifecycle::ContextError::DeadlineExceeded=>Cause::DeadlineExceeded};
                return Err(Error::new(Reason::UpstreamUnavailable,"Open").with_transport(TransportKind::Http).with_cause(Cause::TransportFailure(Box::new(cause))).into());
            }
        };
        let persisted =
            if user_id <= 0 || credentials.user_id <= 0 || user_id != credentials.user_id {
                Err(Error::new(Reason::LocalStateError, "OpenAccountClient")
                    .with_detail("credential identity does not match selected account")
                    .into())
            } else {
                self.repository.rotate(
                    context,
                    user_id,
                    account.credential_revision,
                    credentials.refresh_token().as_bytes(),
                )
            };
        persisted.map_err(|error| wrapped("persist rotated pixiv credentials", error))?;
        Ok(credentials)
    }
}
