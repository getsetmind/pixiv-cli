use super::{Shared, context_marker, schema::Step, trace};
use pixiv_app::{
    config::{ConfigError, ConfigFiles, Store, SystemConfigFiles},
    database::Database,
    fanbox_account::{Account, DefaultStore, Repository},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub struct RepositoryPort {
    pub database: Arc<Mutex<Database>>,
    pub observed: Shared,
    pub failure: String,
}
impl RepositoryPort {
    fn before(&self, context: &Context, operation: &str) -> Result<(), SchedulerError> {
        trace(
            &self.observed,
            format!("repository.{operation}/context={}", context_marker(context)),
        );
        if self.failure == operation {
            return Err(SchedulerError::Message(format!(
                "owned fixture repository {operation} failure"
            )));
        }
        Ok(())
    }
}
impl Repository for RepositoryPort {
    fn save_credential(&self, context: &Context, account: &Account) -> Result<(), SchedulerError> {
        self.before(context, "save")?;
        self.database.save_credential(context, account)
    }
    fn rotate_session(
        &self,
        _: &Context,
        _: i64,
        _: i64,
        _: &[u8],
        _: i64,
    ) -> Result<(), SchedulerError> {
        panic!("auth management cannot rotate a session")
    }
    fn list(&self, context: &Context) -> Result<Vec<Account>, SchedulerError> {
        self.before(context, "list")?;
        self.database.list(context)
    }
    fn get(&self, context: &Context, id: i64) -> Result<Account, SchedulerError> {
        self.before(context, "get")?;
        self.database.get(context, id)
    }
    fn remove(&self, context: &Context, id: i64) -> Result<(), SchedulerError> {
        self.before(context, "remove")?;
        self.database.remove(context, id)
    }
}

pub struct DefaultPort {
    pub store: Store,
    pub step: Step,
    pub observed: Shared,
    pub reads: Mutex<usize>,
}
impl DefaultStore for DefaultPort {
    fn read(&self) -> Result<Option<i64>, SchedulerError> {
        trace(&self.observed, "defaults.read");
        let mut reads = self.reads.lock().unwrap();
        *reads += 1;
        if self.step.default_failure == "read"
            && (self.step.default_fail_nth == 0 || self.step.default_fail_nth == *reads)
        {
            return Err(SchedulerError::Message(
                "owned fixture default read failure".into(),
            ));
        }
        self.store.read_fanbox_default_user_id().map_err(Into::into)
    }
    fn set(&self, id: i64) -> Result<(), SchedulerError> {
        trace(&self.observed, format!("defaults.set/{id}"));
        if self.step.default_failure == "set" {
            return Err(SchedulerError::Message(
                "owned fixture default set failure".into(),
            ));
        }
        self.store
            .set_fanbox_default_user_id(id)
            .map_err(Into::into)
    }
    fn clear(&self) -> Result<(), SchedulerError> {
        trace(&self.observed, "defaults.clear");
        if self.step.default_failure == "clear" {
            return Err(SchedulerError::Message(
                "owned fixture default clear failure".into(),
            ));
        }
        self.store
            .clear_fanbox_default_user_id()
            .map_err(Into::into)
    }
}

pub struct Files {
    pub path: PathBuf,
    pub failure: String,
    pub observed: Shared,
}
impl ConfigFiles for Files {
    fn path(&self) -> Result<PathBuf, ConfigError> {
        trace(&self.observed, "config.path");
        if self.failure == "path" {
            return Err(ConfigError::Invalid(
                "owned fixture config path failure".into(),
            ));
        }
        Ok(self.path.clone())
    }
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ConfigError> {
        trace(&self.observed, "config.read");
        if self.failure == "read" {
            return Err(ConfigError::Invalid(
                "owned fixture config read failure".into(),
            ));
        }
        std::fs::read(path).map_err(ConfigError::Io)
    }
    fn write_private_file(&self, path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
        trace(&self.observed, "config.write");
        if self.failure == "write" {
            return Err(ConfigError::Invalid(
                "owned fixture config write failure".into(),
            ));
        }
        SystemConfigFiles::new(path).write_private_file(path, bytes)
    }
    fn ensure_private_file(&self, _: &Path, _: &[u8]) -> Result<(), ConfigError> {
        panic!("auth must not ensure config")
    }
}
