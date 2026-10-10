use crate::{
    fanbox_account_service::AccountService,
    lifecycle::{Attempt, Context, Lease},
    scheduler::SchedulerError,
    sessions::{ClientOpen, CloseClient},
};
use futures_util::FutureExt;
use pixiv_sdk::fanbox::Client;
use std::{
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    pin::Pin,
    sync::Arc,
};

pub trait AccountOpener: Send + Sync {
    fn open_client_with_proxy(
        &self,
        context: &Context,
        proxy_override: Option<&str>,
    ) -> ClientOpen<Client>;
}
impl AccountOpener for AccountService {
    fn open_client_with_proxy(
        &self,
        context: &Context,
        proxy_override: Option<&str>,
    ) -> ClientOpen<Client> {
        AccountService::open_client_with_proxy(self, context, proxy_override)
    }
}
#[derive(Clone, Default)]
pub struct OpenRequest {
    pub proxy_override: Option<String>,
}

pub type UseFuture = Pin<Box<dyn Future<Output = Result<(), SchedulerError>> + Send>>;
pub type UseCallback = dyn Fn(Context, Arc<Client>, Arc<Attempt>) -> UseFuture + Send + Sync;

pub struct Facade {
    pub accounts: Option<Arc<dyn AccountOpener>>,
    pub close_client: Option<Arc<CloseClient<Client>>>,
}
impl Facade {
    pub fn new(accounts: Option<Arc<dyn AccountOpener>>) -> Self {
        Self::with_close_client(accounts, None)
    }
    pub fn with_close_client(
        accounts: Option<Arc<dyn AccountOpener>>,
        close_client: Option<Arc<CloseClient<Client>>>,
    ) -> Self {
        Self {
            accounts,
            close_client,
        }
    }
    pub async fn open(
        &self,
        context: Option<&Context>,
        request: OpenRequest,
    ) -> Result<Lease<Arc<Client>, SchedulerError>, SchedulerError> {
        let context = context.ok_or_else(|| message("lifecycle: context is nil"))?;
        let accounts = self
            .accounts
            .as_ref()
            .ok_or_else(|| message("fanbox account service is not configured"))?;
        let close: Arc<CloseClient<Client>> = self.close_client.clone().unwrap_or_else(|| {
            Arc::new(|client| {
                client.close_idle_connections();
                Ok(())
            })
        });
        let opened = accounts.open_client_with_proxy(context, request.proxy_override.as_deref());
        if let Some(error) = opened.error {
            if let Some(client) = opened.client {
                return Err(SchedulerError::Joined(match close(&client) {
                    Ok(()) => vec![error],
                    Err(closed) => vec![error, closed],
                }));
            }
            return Err(error);
        }
        let client = opened
            .client
            .ok_or_else(|| message("fanbox account service returned a nil client"))?;
        let retained = client.clone();
        Ok(Lease::new(client, Some(Box::new(move || close(&retained)))))
    }
    pub async fn use_client(
        &self,
        context: Option<&Context>,
        request: OpenRequest,
        callback: Option<Arc<UseCallback>>,
    ) -> Result<(), SchedulerError> {
        let context = context.ok_or_else(|| message("lifecycle: context is nil"))?;
        let callback = callback.ok_or_else(|| message("lifecycle: use function is nil"))?;
        let child = context.child();
        let _cancel = CancelContext(child.clone());
        let lease = OwnedLease(self.open(Some(&child), request).await?);
        let attempt = Arc::new(Attempt::default());
        let future = catch_unwind(AssertUnwindSafe(|| {
            callback(child, lease.0.value().clone(), attempt)
        }));
        let used = match future {
            Ok(future) => AssertUnwindSafe(future).catch_unwind().await,
            Err(panic) => Err(panic),
        };
        let closed = lease.0.close();
        match used {
            Err(panic) => resume_unwind(panic),
            Ok(used) => match (used, closed) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(error), Ok(())) => Err(error),
                (Ok(()), Err(error)) => Err(SchedulerError::Shared(error)),
                (Err(error), Err(closed)) => Err(SchedulerError::Joined(vec![
                    error,
                    SchedulerError::Shared(closed),
                ])),
            },
        }
    }
}
struct CancelContext(Context);
impl Drop for CancelContext {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct OwnedLease(Lease<Arc<Client>, SchedulerError>);
impl Drop for OwnedLease {
    fn drop(&mut self) {
        let _ = self.0.close();
    }
}
fn message(value: &str) -> SchedulerError {
    SchedulerError::Message(value.into())
}
