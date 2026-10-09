use crate::{
    account_management::AccountDefaultStore,
    account_service::{AccountService, AccountSummary},
    lifecycle::{Context, ContextError},
    scheduler::SchedulerError,
};
use chrono::Utc;
use pixiv_sdk::{
    Error, Reason,
    error::{Cause, TransportKind},
    pixiv::CurrentUserRequest,
    transport::Transport,
};

pub struct AccountValidation<'a> {
    service: &'a AccountService,
    defaults: &'a dyn AccountDefaultStore,
}

impl AccountService {
    pub fn validation<'a>(
        &'a self,
        defaults: &'a dyn AccountDefaultStore,
    ) -> AccountValidation<'a> {
        AccountValidation {
            service: self,
            defaults,
        }
    }

    pub async fn check_account_with<T: Transport>(
        &self,
        context: &Context,
        user_id: i64,
        transport: T,
    ) -> Result<AccountSummary, SchedulerError> {
        let account = self.repository.get(context, user_id)?;
        let credentials = self
            .rotate_account(context, account.user_id, &account, &transport)
            .await?;
        Ok(AccountSummary {
            user_id: credentials.user_id,
            username: credentials.username,
            default: false,
            premium: None,
            schedulable: false,
            pool_frozen_until: None,
            pool_last_selected: false,
            eligible: false,
            pool_status_known: false,
        })
    }
}

impl AccountValidation<'_> {
    pub async fn refresh_account_with<T: Transport>(
        &self,
        context: &Context,
        user_id: i64,
        transport: T,
    ) -> Result<AccountSummary, SchedulerError> {
        let client = self
            .service
            .open_account(context, user_id, transport)
            .await?;
        let detail = tokio::select! {
            result = client.current_user(CurrentUserRequest::default()) => result?,
            error = context.cancelled() => {
                let cause = match error { ContextError::Canceled => Cause::Canceled, ContextError::DeadlineExceeded => Cause::DeadlineExceeded };
                return Err(Error::new(Reason::UpstreamUnavailable, "CurrentUser").with_transport(TransportKind::Http).with_cause(Cause::TransportFailure(Box::new(cause))).into());
            }
        };
        let account = self.service.repository.get(context, user_id)?;
        self.service.repository.update_metadata(
            context,
            account.user_id,
            &account.username,
            Some(detail.profile.is_premium),
            Some(Utc::now().timestamp()),
        )?;
        self.service
            .transfer(self.defaults)
            .summary(context, user_id)
    }
}
