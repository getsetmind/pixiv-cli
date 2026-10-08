use crate::{
    lifecycle::Context,
    scheduler::{AttemptFuture, SchedulerError},
};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub type GateCallback<'a> = dyn FnMut(Context) -> AttemptFuture<'a> + Send + 'a;

#[derive(Clone, Debug, Default)]
pub struct Gate {
    slots: Option<Arc<Semaphore>>,
}

#[derive(Debug)]
pub struct Permit {
    _slot: OwnedSemaphorePermit,
}

impl Gate {
    pub fn new() -> Self {
        Self {
            slots: Some(Arc::new(Semaphore::new(1))),
        }
    }
    fn slots(&self) -> Result<&Arc<Semaphore>, SchedulerError> {
        self.slots
            .as_ref()
            .ok_or_else(|| SchedulerError::Message("pixiv rotation gate is not configured".into()))
    }
    pub async fn acquire(&self, context: &Context) -> Result<Permit, SchedulerError> {
        let slots = Arc::clone(self.slots()?);
        tokio::select! {
            slot = slots.acquire_owned() => Ok(Permit { _slot:slot.expect("rotation gate unexpectedly closed") }),
            error = context.cancelled() => Err(error.into()),
        }
    }
    pub async fn run(
        &self,
        context: &Context,
        callback: Option<&mut GateCallback<'_>>,
    ) -> Result<(), SchedulerError> {
        self.slots()?;
        let callback = callback
            .ok_or_else(|| SchedulerError::Message("pixiv rotation gate function is nil".into()))?;
        let _permit = self.acquire(context).await?;
        callback(context.clone()).await
    }
}
