use crate::{
    lifecycle::{Attempt, Context, Lease},
    scheduler::{AttemptFuture, PoolState, Scheduler, SchedulerError},
    sessions::ClientSessions,
};
use chrono::{DateTime, Utc};
use std::{future::Future, pin::Pin, sync::Arc};

#[derive(Clone)]
pub struct PoolConfig {
    pub enabled: bool,
    pub strategy: String,
}
pub struct UseOutcome {
    pub committed: bool,
    pub error: Option<SchedulerError>,
}
pub type UseFuture = Pin<Box<dyn Future<Output = UseOutcome> + Send>>;
pub type UseCallback<C> = dyn Fn(Context, Arc<C>) -> UseFuture + Send + Sync;
pub type PoolAttempt = dyn Fn(Context, i64, Arc<Attempt>) -> AttemptFuture<'static> + Send + Sync;
pub type PoolExecutor = Box<dyn FnOnce(Context, Arc<PoolAttempt>) -> AttemptFuture<'static> + Send>;
pub type ConfigLoader = dyn Fn() -> Result<PoolConfig, SchedulerError> + Send + Sync;
pub type PoolFactory =
    dyn Fn(PoolConfig) -> Result<Option<PoolExecutor>, SchedulerError> + Send + Sync;
pub type PoolClock = dyn Fn() -> DateTime<Utc> + Send + Sync;

pub fn pool_executor(
    config: PoolConfig,
    mut state: Box<dyn PoolState>,
    now: Option<Arc<PoolClock>>,
) -> PoolExecutor {
    Box::new(move |context, attempt| {
        Box::pin(async move {
            let mut clock = move || now.as_ref().map(|clock| clock()).unwrap_or_else(Utc::now);
            let mut callback = move |context, id, state| attempt(context, id, state);
            let mut scheduler = Scheduler {
                enabled: config.enabled,
                strategy: &config.strategy,
                state: Some(state.as_mut()),
                now: Some(&mut clock),
                random: None,
            };
            scheduler.run(&context, Some(&mut callback)).await
        })
    })
}

pub struct Facade<C, O> {
    pub sessions: Arc<ClientSessions<C, O>>,
    pub load_pool_config: Option<Arc<ConfigLoader>>,
    pub pool_factory: Option<Arc<PoolFactory>>,
}

struct ActiveLease<C>(Lease<Arc<C>, SchedulerError>);
impl<C> Drop for ActiveLease<C> {
    fn drop(&mut self) {
        let _ = self.0.close();
    }
}
fn message(value: &str) -> SchedulerError {
    SchedulerError::Message(value.to_owned())
}

impl<C: Send + Sync + 'static, O: Clone + Send + Sync + 'static> Facade<C, O> {
    pub async fn use_client(
        &self,
        context: Option<&Context>,
        user_id: i64,
        options: O,
        callback: Option<Arc<UseCallback<C>>>,
    ) -> Result<(), SchedulerError> {
        let context = context.ok_or_else(|| message("lifecycle: context is nil"))?;
        let callback =
            callback.ok_or_else(|| message("pixiv client callback is not configured"))?;
        let loader = self
            .load_pool_config
            .as_ref()
            .ok_or_else(|| message("account pool runtime loader is not configured"))?;
        let config = loader()?;
        let sessions = self.sessions.clone();
        let attempt: Arc<PoolAttempt> = Arc::new(move |context, id, state| {
            let sessions = sessions.clone();
            let callback = callback.clone();
            let options = options.clone();
            Box::pin(async move {
                let lease = sessions
                    .open(Some(&context), id, options)
                    .await
                    .map_err(|error| SchedulerError::Joined(error.into_errors()))?;
                let lease = ActiveLease(lease);
                let outcome = callback(context, lease.0.value().clone()).await;
                if outcome.committed {
                    state.commit();
                }
                let closed = lease.0.close();
                drop(lease);
                match (outcome.error, closed) {
                    (None, Ok(())) => Ok(()),
                    (Some(error), Ok(())) => Err(error),
                    (None, Err(error)) => Err(SchedulerError::Shared(error)),
                    (Some(error), Err(closed)) => Err(SchedulerError::Joined(vec![
                        error,
                        SchedulerError::Shared(closed),
                    ])),
                }
            })
        });
        if !config.enabled {
            return attempt(context.clone(), user_id, Arc::new(Attempt::default())).await;
        }
        let factory = self
            .pool_factory
            .as_ref()
            .ok_or_else(|| message("account pool executor is not configured"))?;
        let executor =
            factory(config)?.ok_or_else(|| message("account pool executor is not configured"))?;
        executor(context.clone(), attempt).await
    }
}
