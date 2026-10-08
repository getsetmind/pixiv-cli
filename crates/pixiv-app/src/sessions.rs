use crate::{
    gate::Gate,
    lifecycle::{Context, Lease},
    scheduler::SchedulerError,
};
use std::{error::Error as StdError, fmt, future::Future, pin::Pin, sync::Arc};

pub struct ClientOpen<C> {
    pub client: Option<Arc<C>>,
    pub error: Option<SchedulerError>,
}
pub type ClientOpenFuture<C> = Pin<Box<dyn Future<Output = ClientOpen<C>> + Send>>;
pub type ClientOpener<C, O> = dyn Fn(Context, i64, O) -> ClientOpenFuture<C> + Send + Sync;
pub type CloseClient<C> = dyn Fn(&C) -> Result<(), SchedulerError> + Send + Sync;

pub struct ClientSessions<C, O> {
    pub accounts: Option<Arc<ClientOpener<C, O>>>,
    pub gate: Option<Gate>,
    pub close_client: Arc<CloseClient<C>>,
}

#[derive(Debug)]
pub struct SessionError {
    errors: Vec<SchedulerError>,
}
impl From<SchedulerError> for SessionError {
    fn from(error: SchedulerError) -> Self {
        Self {
            errors: vec![error],
        }
    }
}
impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.errors.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            fmt::Display::fmt(error, f)?;
        }
        Ok(())
    }
}
impl StdError for SessionError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.errors.first().map(|error| error as &dyn StdError)
    }
}
impl SessionError {
    pub fn into_errors(self) -> Vec<SchedulerError> {
        self.errors
    }
    pub fn errors(&self) -> &[SchedulerError] {
        &self.errors
    }
    pub fn classified(&self) -> Option<&pixiv_sdk::Error> {
        self.errors.iter().find_map(SchedulerError::classified)
    }
    pub fn is_canceled(&self) -> bool {
        self.errors.iter().any(SchedulerError::is_canceled)
    }
    pub fn is_deadline_exceeded(&self) -> bool {
        self.errors.iter().any(SchedulerError::is_deadline_exceeded)
    }
    fn message(message: &str) -> Self {
        SchedulerError::Message(message.into()).into()
    }
}

impl<C: Send + Sync + 'static, O> ClientSessions<C, O> {
    pub async fn open(
        &self,
        context: Option<&Context>,
        user_id: i64,
        options: O,
    ) -> Result<Lease<Arc<C>, SchedulerError>, SessionError> {
        let context = context.ok_or_else(|| SessionError::message("lifecycle: context is nil"))?;
        let accounts = self
            .accounts
            .as_ref()
            .ok_or_else(|| SessionError::message("pixiv account service is not configured"))?;
        let gate = self
            .gate
            .as_ref()
            .ok_or_else(|| SessionError::message("pixiv rotation gate is not configured"))?;
        let permit = gate.acquire(context).await?;
        let opened = accounts(context.clone(), user_id, options).await;
        if let Some(error) = opened.error {
            let mut error = SessionError::from(error);
            if let Some(client) = opened.client
                && let Err(close_error) = (self.close_client)(&client)
            {
                error.errors.push(close_error);
            }
            return Err(error);
        }
        let client = opened
            .client
            .ok_or_else(|| SessionError::message("pixiv account service returned a nil client"))?;
        let retained = Arc::clone(&client);
        let close = Arc::clone(&self.close_client);
        Ok(Lease::new(
            client,
            Some(Box::new(move || {
                let _permit = permit;
                close(&retained)
            })),
        ))
    }
}
