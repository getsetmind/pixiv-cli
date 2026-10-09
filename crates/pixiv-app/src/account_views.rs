use crate::{
    account_service::AccountService, database::PoolStatus, lifecycle::Context,
    scheduler::SchedulerError,
};
use chrono::Utc;

#[derive(Clone, Debug)]
pub struct AccountSummary {
    pub user_id: i64,
    pub username: String,
    pub default: bool,
    pub premium: Option<bool>,
    pub schedulable: bool,
    pub pool_frozen_until: Option<i64>,
    pub pool_last_selected: bool,
    pub eligible: bool,
    pub pool_status_known: bool,
}

impl AccountService {
    pub fn list_accounts(&self, context: &Context) -> Result<Vec<AccountSummary>, SchedulerError> {
        let pool = self.pool_status(context)?;
        let accounts = self.repository.list(context)?;
        let mut summaries = Vec::with_capacity(accounts.len());
        for account in accounts {
            let defaults = self.defaults.as_ref().ok_or_else(|| {
                SchedulerError::Message("pixiv default account store is not configured".into())
            })?;
            let selected = match defaults()? {
                Some(id) => Some(id),
                None => self
                    .repository
                    .list(context)?
                    .first()
                    .map(|account| account.user_id),
            };
            let candidate = pool
                .accounts
                .iter()
                .find(|candidate| candidate.user_id == account.user_id);
            summaries.push(AccountSummary {
                user_id: account.user_id,
                username: account.username,
                default: selected == Some(account.user_id),
                premium: account.premium_status,
                schedulable: candidate.is_some_and(|candidate| candidate.schedulable),
                pool_frozen_until: candidate.and_then(|candidate| candidate.pool_frozen_until),
                pool_last_selected: candidate.is_some_and(|candidate| candidate.pool_last_selected),
                eligible: candidate.is_some_and(|candidate| candidate.eligible),
                pool_status_known: true,
            });
        }
        Ok(summaries)
    }

    pub fn pool_status(&self, context: &Context) -> Result<PoolStatus, SchedulerError> {
        self.repository.pool_status(context, Utc::now().timestamp())
    }

    pub fn set_pool_schedulable(
        &self,
        context: &Context,
        ids: &[i64],
        enabled: bool,
    ) -> Result<(), SchedulerError> {
        self.repository.set_pool_schedulable(context, ids, enabled)
    }

    pub fn set_all_pool_schedulable(
        &self,
        context: &Context,
        enabled: bool,
    ) -> Result<(), SchedulerError> {
        self.repository.set_all_pool_schedulable(context, enabled)
    }
}
