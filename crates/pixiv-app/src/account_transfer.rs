use crate::{
    account_management::AccountDefaultStore,
    account_service::{AccountService, AccountSummary},
    database::PixivAccount,
    lifecycle::{Context, ContextError},
    scheduler::SchedulerError,
};
use chrono::Utc;
use pixiv_sdk::{
    Error, Reason,
    error::{Cause, TransportKind},
    oauth,
    transport::Transport,
};
use serde::Serialize;
use std::{collections::BTreeSet, fmt};

#[derive(Clone, Serialize)]
pub struct AccountWithToken {
    pub user_id: i64,
    pub username: String,
    pub default: bool,
    #[serde(skip)]
    refresh_token: String,
    #[serde(skip)]
    refresh_token_bytes: Vec<u8>,
}
impl AccountWithToken {
    pub fn new(
        user_id: i64,
        username: impl Into<String>,
        default: bool,
        refresh_token: impl Into<String>,
    ) -> Self {
        let token = refresh_token.into();
        Self::from_token_bytes(user_id, username, default, token.as_bytes())
    }
    pub fn from_token_bytes(
        user_id: i64,
        username: impl Into<String>,
        default: bool,
        refresh_token: &[u8],
    ) -> Self {
        Self {
            user_id,
            username: username.into(),
            default,
            refresh_token: crate::auth_bundle::go_utf8(refresh_token),
            refresh_token_bytes: refresh_token.to_vec(),
        }
    }
    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }
    pub fn refresh_token_bytes(&self) -> &[u8] {
        &self.refresh_token_bytes
    }
}
impl fmt::Debug for AccountWithToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountWithToken")
            .field("user_id", &self.user_id)
            .field("username", &self.username)
            .field("default", &self.default)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct RestoreAccountInput {
    pub account: AccountSummary,
    pub refresh_token: String,
    pub is_bundle_default: bool,
}
impl fmt::Debug for RestoreAccountInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RestoreAccountInput")
            .field("account", &self.account)
            .field("is_bundle_default", &self.is_bundle_default)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug)]
pub struct RestoreAccountOutcome {
    pub account: AccountSummary,
    pub is_replacement: bool,
}
#[derive(Clone, Debug)]
pub struct RestoreAccountsResult {
    pub accounts: Vec<RestoreAccountOutcome>,
    pub resulting_default: i64,
}

pub struct AccountTransfer<'a> {
    service: &'a AccountService,
    defaults: &'a dyn AccountDefaultStore,
}
impl AccountService {
    pub fn transfer<'a>(&'a self, defaults: &'a dyn AccountDefaultStore) -> AccountTransfer<'a> {
        AccountTransfer {
            service: self,
            defaults,
        }
    }
}
fn wrapped(message: &str, source: SchedulerError) -> SchedulerError {
    SchedulerError::Wrapped {
        message: message.into(),
        source: Box::new(source),
    }
}
fn required_token() -> SchedulerError {
    SchedulerError::Message("pixiv refresh token is required".into())
}

impl AccountTransfer<'_> {
    fn selected(&self, context: &Context) -> Result<Option<i64>, SchedulerError> {
        match self.defaults.read()? {
            Some(id) => Ok(Some(id)),
            None => Ok(self
                .service
                .repository
                .list(context)?
                .first()
                .map(|account| account.user_id)),
        }
    }
    pub(crate) fn summary(
        &self,
        context: &Context,
        id: i64,
    ) -> Result<AccountSummary, SchedulerError> {
        let account = self.service.repository.get(context, id)?;
        let selected = self.selected(context)?;
        let now = Utc::now().timestamp();
        let frozen = account.pool_frozen_until.filter(|until| *until > now);
        Ok(AccountSummary {
            user_id: account.user_id,
            username: account.username,
            default: selected == Some(account.user_id),
            premium: account.premium_status,
            schedulable: account.schedulable,
            pool_frozen_until: frozen,
            pool_last_selected: account.pool_last_selected,
            eligible: account.schedulable && frozen.is_none(),
            pool_status_known: true,
        })
    }
    pub async fn import_account_with<T: Transport>(
        &self,
        context: &Context,
        token: &str,
        set_default: bool,
        transport: T,
    ) -> Result<AccountSummary, SchedulerError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(required_token());
        }
        let credentials = tokio::select! {
            result = oauth::refresh(&transport, token) => result?,
            error = context.cancelled() => {
                let cause = match error { ContextError::Canceled => Cause::Canceled, ContextError::DeadlineExceeded => Cause::DeadlineExceeded };
                return Err(Error::new(Reason::UpstreamUnavailable, "Open").with_transport(TransportKind::Http).with_cause(Cause::TransportFailure(Box::new(cause))).into());
            }
        };
        let mut account = PixivAccount::new(
            credentials.user_id,
            &credentials.username,
            credentials.refresh_token().as_bytes(),
        );
        account.credential_revision = 1;
        self.service
            .repository
            .save_credential(context, &account)
            .map_err(|source| wrapped("save pixiv account", source))?;
        let selected = self
            .defaults
            .read()
            .map_err(|source| wrapped("read pixiv default account", source))?;
        if set_default || selected.is_none() {
            self.defaults
                .set(credentials.user_id)
                .map_err(|source| wrapped("set pixiv default account", source))?;
        }
        self.summary(context, credentials.user_id)
    }
    pub fn current_user(&self, context: &Context) -> Result<AccountSummary, SchedulerError> {
        let id = self.selected(context)?.ok_or_else(|| {
            SchedulerError::from(
                Error::new(Reason::Unauthorized, "auth")
                    .with_detail("no pixiv account is authenticated"),
            )
        })?;
        self.service.repository.get(context, id).map_err(|_| {
            SchedulerError::Message(format!("configured default pixiv account {id} is missing"))
        })?;
        let account = self.service.repository.get(context, id)?;
        Ok(AccountSummary {
            user_id: account.user_id,
            username: account.username,
            default: true,
            premium: account.premium_status,
            schedulable: account.schedulable,
            pool_frozen_until: account.pool_frozen_until,
            pool_last_selected: account.pool_last_selected,
            eligible: account.schedulable
                && account
                    .pool_frozen_until
                    .is_none_or(|until| until <= Utc::now().timestamp()),
            pool_status_known: true,
        })
    }
    pub fn accounts_with_tokens(
        &self,
        context: &Context,
    ) -> Result<Vec<AccountWithToken>, SchedulerError> {
        let accounts = self.service.repository.list(context)?;
        let mut output = Vec::with_capacity(accounts.len());
        for account in accounts {
            let selected = self.selected(context)?;
            let token = account.refresh_token_copy();
            output.push(AccountWithToken::from_token_bytes(
                account.user_id,
                account.username,
                selected == Some(account.user_id),
                &token,
            ));
        }
        Ok(output)
    }
    pub fn restore_accounts(
        &self,
        context: &Context,
        inputs: &[RestoreAccountInput],
    ) -> Result<RestoreAccountsResult, SchedulerError> {
        if inputs.is_empty() {
            return Err(SchedulerError::Message(
                "pixiv restore bundle has no accounts".into(),
            ));
        }
        let mut stored = Vec::with_capacity(inputs.len());
        for input in inputs {
            let token = input.refresh_token.trim();
            if input.account.user_id <= 0 || token.is_empty() {
                return Err(required_token());
            }
            let mut account = PixivAccount::new(
                input.account.user_id,
                &input.account.username,
                token.as_bytes(),
            );
            account.credential_revision = 1;
            account.premium_status = input.account.premium;
            stored.push(account);
        }
        let before = self
            .defaults
            .read()
            .map_err(|source| wrapped("read pixiv default account", source))?;
        let existing: BTreeSet<_> = self
            .service
            .repository
            .list(context)
            .map_err(|source| wrapped("read pixiv accounts", source))?
            .into_iter()
            .map(|account| account.user_id)
            .collect();
        self.service
            .repository
            .save_credentials(context, &stored)
            .map_err(|source| wrapped("restore pixiv accounts", source))?;
        let resulting_default = match before {
            Some(id) => id,
            None => {
                let id = inputs
                    .iter()
                    .find(|input| input.is_bundle_default)
                    .unwrap_or(&inputs[0])
                    .account
                    .user_id;
                self.defaults
                    .set(id)
                    .map_err(|source| wrapped("set pixiv default account", source))?;
                id
            }
        };
        Ok(RestoreAccountsResult {
            accounts: inputs
                .iter()
                .map(|input| RestoreAccountOutcome {
                    account: input.account.clone(),
                    is_replacement: existing.contains(&input.account.user_id),
                })
                .collect(),
            resulting_default,
        })
    }
}
