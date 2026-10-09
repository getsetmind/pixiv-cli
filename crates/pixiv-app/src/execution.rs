use crate::{
    account_service::AccountService,
    config::Store,
    connection::CommandConnection,
    database::Database,
    facade::{Facade, UseCallback, UseOutcome, pool_executor},
    gate::Gate,
    lifecycle::{Context, Lease},
    scheduler::SchedulerError,
    sessions::{ClientOpen, ClientSessions},
};
use pixiv_sdk::{
    Client,
    transport::{HttpTransport, Transport},
};
use std::sync::{Arc, Mutex};

pub struct Execution<T> {
    config: Store,
    facade: Facade<Client<T>, CommandConnection>,
}

impl Execution<HttpTransport> {
    pub fn http(config: Store, database: Arc<Mutex<Database>>) -> Self {
        Self::new(config, database, |options| {
            options.open_transport().map_err(Into::into)
        })
    }
}

impl<T: Transport + 'static> Execution<T> {
    pub async fn write<F, U>(
        &self,
        context: &Context,
        user_id: i64,
        proxy: Option<&str>,
        invoke: F,
    ) -> Result<(), SchedulerError>
    where
        F: Fn(Context, Arc<Client<T>>) -> U + Send + Sync + 'static,
        U: std::future::Future<Output = Result<(), SchedulerError>> + Send + 'static,
    {
        self.use_client(
            Some(context),
            user_id,
            proxy,
            Some(Arc::new(move |context, client| {
                let future = invoke(context, client);
                Box::pin(async move {
                    UseOutcome {
                        // A failed mutation can already have reached the service, so replay is unsafe.
                        committed: true,
                        error: future.await.err(),
                    }
                })
            })),
        )
        .await
    }

    pub async fn read<V, F, U>(
        &self,
        context: &Context,
        user_id: i64,
        proxy: Option<&str>,
        invoke: F,
    ) -> Result<V, SchedulerError>
    where
        V: Send + 'static,
        F: Fn(Context, Arc<Client<T>>) -> U + Send + Sync + 'static,
        U: std::future::Future<Output = Result<V, SchedulerError>> + Send + 'static,
    {
        let result = Arc::new(Mutex::new(None));
        let fetched = result.clone();
        self.use_client(
            Some(context),
            user_id,
            proxy,
            Some(Arc::new(move |context, client| {
                let future = invoke(context, client);
                let fetched = fetched.clone();
                Box::pin(async move {
                    match future.await {
                        Ok(value) => {
                            *fetched
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value);
                            UseOutcome {
                                committed: false,
                                error: None,
                            }
                        }
                        Err(error) => UseOutcome {
                            committed: false,
                            error: Some(error),
                        },
                    }
                })
            })),
        )
        .await?;
        result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| {
                SchedulerError::Message("pixiv read operation returned no result".into())
            })
    }

    pub fn new(
        config: Store,
        database: Arc<Mutex<Database>>,
        connection: impl Fn(CommandConnection) -> Result<T, SchedulerError> + Send + Sync + 'static,
    ) -> Self {
        let defaults = config.clone();
        let service = Arc::new(AccountService {
            repository: database.clone(),
            defaults: Some(Arc::new(move || {
                defaults.read_pixiv_default_user_id().map_err(Into::into)
            })),
        });
        let connection = Arc::new(connection);
        let sessions = Arc::new(ClientSessions {
            accounts: Some(Arc::new(move |context, id, options| {
                let service = service.clone();
                let connection = connection.clone();
                Box::pin(async move {
                    let result = match connection(options) {
                        Ok(transport) => service.open(&context, id, transport).await,
                        Err(error) => Err(error),
                    };
                    match result {
                        Ok(client) => ClientOpen {
                            client: Some(Arc::new(client)),
                            error: None,
                        },
                        Err(error) => ClientOpen {
                            client: None,
                            error: Some(error),
                        },
                    }
                })
            })),
            gate: Some(Gate::new()),
            close_client: Arc::new(|_| Ok(())),
        });
        let pool_config = config.clone();
        let facade = Facade {
            sessions,
            load_pool_config: Some(Arc::new(move || {
                Ok(pool_config.current()?.runtime()?.account_pool)
            })),
            pool_factory: Some(Arc::new(move |config| {
                Ok(Some(pool_executor(
                    config,
                    Box::new(database.clone()),
                    None,
                )))
            })),
        };
        Self { config, facade }
    }

    pub async fn open_client(
        &self,
        context: &Context,
        user_id: i64,
        proxy: Option<&str>,
    ) -> Result<Lease<Arc<Client<T>>, SchedulerError>, SchedulerError> {
        let runtime = self.config.current()?.runtime()?;
        let connection =
            CommandConnection::resolve(&runtime, proxy).map_err(SchedulerError::Proxy)?;
        self.facade
            .sessions
            .open(Some(context), user_id, connection)
            .await
            .map_err(|error| SchedulerError::Joined(error.into_errors()))
    }

    pub async fn use_client(
        &self,
        context: Option<&Context>,
        user_id: i64,
        proxy: Option<&str>,
        callback: Option<Arc<UseCallback<Client<T>>>>,
    ) -> Result<(), SchedulerError> {
        let runtime = self.config.current()?.runtime()?;
        let connection =
            CommandConnection::resolve(&runtime, proxy).map_err(SchedulerError::Proxy)?;
        self.facade
            .use_client(context, user_id, connection, callback)
            .await
    }
}
