use crate::{
    account_service::AccountService, config::Store, lifecycle::Context, scheduler::SchedulerError,
};

pub trait AccountDefaultStore: Send + Sync {
    fn read(&self) -> Result<Option<i64>, SchedulerError>;
    fn set(&self, user_id: i64) -> Result<(), SchedulerError>;
    fn clear(&self) -> Result<(), SchedulerError>;
}

impl AccountDefaultStore for Store {
    fn read(&self) -> Result<Option<i64>, SchedulerError> {
        self.read_pixiv_default_user_id().map_err(Into::into)
    }
    fn set(&self, user_id: i64) -> Result<(), SchedulerError> {
        self.set_pixiv_default_user_id(user_id).map_err(Into::into)
    }
    fn clear(&self) -> Result<(), SchedulerError> {
        self.clear_pixiv_default_user_id().map_err(Into::into)
    }
}

pub struct AccountManagement<'a> {
    service: &'a AccountService,
    defaults: &'a dyn AccountDefaultStore,
}

impl AccountService {
    pub fn management<'a>(
        &'a self,
        defaults: &'a dyn AccountDefaultStore,
    ) -> AccountManagement<'a> {
        AccountManagement {
            service: self,
            defaults,
        }
    }
}

impl AccountManagement<'_> {
    pub fn use_account(&self, context: &Context, user_id: i64) -> Result<(), SchedulerError> {
        self.service.repository.get(context, user_id)?;
        self.defaults.set(user_id)
    }

    pub fn remove_account(&self, context: &Context, user_id: i64) -> Result<(), SchedulerError> {
        let selected = self
            .defaults
            .read()
            .map_err(|source| SchedulerError::Wrapped {
                message: "read pixiv default account".into(),
                source: Box::new(source),
            })?;
        if selected == Some(user_id) {
            self.defaults
                .clear()
                .map_err(|source| SchedulerError::Wrapped {
                    message: "clear pixiv default account".into(),
                    source: Box::new(source),
                })?;
        }
        self.service.repository.remove(context, user_id)
    }

    pub fn use_auto(&self) -> Result<(), SchedulerError> {
        self.defaults.clear()
    }
}
