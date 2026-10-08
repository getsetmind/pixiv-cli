use pixiv_app::{
    account_service::AccountService,
    config::Store,
    database::{Database, PixivAccount},
    facade::{Facade, PoolConfig, UseOutcome, pool_executor},
    gate::Gate,
    lifecycle::Context,
    sessions::{ClientOpen, ClientSessions},
};
use pixiv_sdk::{
    Client, Error, Reason,
    error::RetryAdvice,
    transport::{Request, Response, Transport},
};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
struct Connection {
    requests: Arc<Mutex<Vec<i64>>>,
    database: Arc<Mutex<Database>>,
}
impl Transport for Connection {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            let token = &request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1;
            let id = if token == "fixture-refresh-42" {
                42
            } else {
                assert_eq!(token, "fixture-refresh-43");
                43
            };
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"access_token":format!("fixture-access-{id}"),"refresh_token":format!("fixture-rotated-{id}"),"expires_in":3600,"user":{"id":id}}),
            });
        }
        let token = &request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .unwrap()
            .1;
        let id = if token == "Bearer fixture-access-42" {
            42
        } else {
            assert_eq!(token, "Bearer fixture-access-43");
            43
        };
        let stored = self.database.lock().unwrap().get_pixiv(id).unwrap();
        assert_eq!(stored.credential_revision, 2);
        assert_eq!(
            stored.refresh_token_copy(),
            format!("fixture-rotated-{id}").as_bytes()
        );
        self.requests.lock().unwrap().push(id);
        let error = if id == 42 {
            Error::new(Reason::RateLimited, "Artwork").with_retry(RetryAdvice {
                safe: true,
                after: Some(chrono::DateTime::from_timestamp(1120, 0).unwrap()),
            })
        } else {
            Error::new(Reason::MalformedUpstreamResponse, "Artwork")
        };
        Err(error)
    }
}

struct CurrentRetryConnection(Connection);
impl Transport for CurrentRetryConnection {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        self.0.send(request).await.map_err(|mut error| {
            if error.code == Reason::RateLimited {
                error.retry.after = Some(chrono::Utc::now() + chrono::TimeDelta::seconds(120));
            }
            error
        })
    }
}

#[tokio::test]
async fn execution_reads_current_connection_and_default_account_before_refreshing_and_persisting() {
    use pixiv_app::execution::Execution;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let mut database = Database::open(directory.path()).unwrap();
    for id in [42, 43] {
        database
            .save_pixiv_credential(&PixivAccount::new(
                id,
                "stored",
                format!("fixture-refresh-{id}").as_bytes(),
            ))
            .unwrap();
    }
    let database = Arc::new(Mutex::new(database));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(Mutex::new(Vec::new()));
    let factory_database = database.clone();
    let factory_requests = requests.clone();
    let factory_connections = connections.clone();
    let execution = Execution::new(Store::new(&path), database.clone(), move |options| {
        factory_connections
            .lock()
            .unwrap()
            .push((options.proxy().to_owned(), options.pacing()));
        Ok(Connection {
            requests: factory_requests.clone(),
            database: factory_database.clone(),
        })
    });
    for (id, proxy, interval, reason) in [
        (42, "http://first.invalid", 10, Reason::RateLimited),
        (
            43,
            "http://second.invalid",
            20,
            Reason::MalformedUpstreamResponse,
        ),
    ] {
        std::fs::write(&path, format!("[pixiv.auth]\ndefault_user_id = {id}\n[pixiv.network]\nproxy_url = '{proxy}'\n[network]\nrequest_interval = '{interval}ms'\n")).unwrap();
        let error = execution
            .use_client(
                Some(&Context::new()),
                0,
                None,
                Some(Arc::new(|_, client: Arc<Client<Connection>>| {
                    Box::pin(async move {
                        UseOutcome {
                            committed: false,
                            error: client.artwork(1).await.err().map(Into::into),
                        }
                    })
                })),
            )
            .await
            .unwrap_err();
        assert_eq!(error.classified().unwrap().code, reason);
    }
    assert_eq!(*requests.lock().unwrap(), [42, 43]);
    assert_eq!(
        *connections.lock().unwrap(),
        [
            (
                "http://first.invalid".to_owned(),
                std::time::Duration::from_millis(10)
            ),
            (
                "http://second.invalid".to_owned(),
                std::time::Duration::from_millis(20)
            ),
        ]
    );
    for id in [42, 43] {
        assert_eq!(
            database
                .lock()
                .unwrap()
                .get_pixiv(id)
                .unwrap()
                .credential_revision,
            2
        );
    }
}

#[tokio::test]
async fn execution_pool_overrides_the_requested_account_and_persists_replay_state() {
    use pixiv_app::execution::Execution;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "[pixiv.auth]\ndefault_user_id = 43\n[account_pool]\nenabled = true\nstrategy = 'round_robin'\n").unwrap();
    let mut database = Database::open(directory.path()).unwrap();
    for id in [42, 43] {
        database
            .save_pixiv_credential(&PixivAccount::new(
                id,
                "stored",
                format!("fixture-refresh-{id}").as_bytes(),
            ))
            .unwrap();
    }
    database.set_all_pixiv_schedulable(true).unwrap();
    let database = Arc::new(Mutex::new(database));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let factory_database = database.clone();
    let factory_requests = requests.clone();
    let execution = Execution::new(Store::new(&path), database.clone(), move |_| {
        Ok(CurrentRetryConnection(Connection {
            requests: factory_requests.clone(),
            database: factory_database.clone(),
        }))
    });
    let error = execution
        .use_client(
            Some(&Context::new()),
            99,
            Some(""),
            Some(Arc::new(
                |_, client: Arc<Client<CurrentRetryConnection>>| {
                    Box::pin(async move {
                        UseOutcome {
                            committed: false,
                            error: client.artwork(1).await.err().map(Into::into),
                        }
                    })
                },
            )),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.classified().unwrap().code,
        Reason::MalformedUpstreamResponse
    );
    assert_eq!(*requests.lock().unwrap(), [42, 43]);
    let database = database.lock().unwrap();
    for id in [42, 43] {
        let account = database.get_pixiv(id).unwrap();
        assert_eq!(account.credential_revision, 2);
        assert_eq!(account.pool_last_selected, id == 43);
    }
}

#[tokio::test]
async fn execution_rejects_configuration_and_proxy_errors_before_acquiring_an_account() {
    use pixiv_app::{execution::Execution, scheduler::SchedulerError};
    use std::error::Error as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let database = Arc::new(Mutex::new(Database::open(directory.path()).unwrap()));
    let execution = Execution::<Connection>::new(Store::new(&path), database, |_| {
        panic!("invalid configuration reached the transport")
    });
    std::fs::write(&path, "[account_pool]\nenabled = 'true'\n").unwrap();
    let error = execution.use_client(None, 0, None, None).await.unwrap_err();
    assert_eq!(error.to_string(), "account_pool.enabled must be a boolean");
    assert!(
        error
            .source()
            .unwrap()
            .is::<pixiv_app::config::ConfigError>()
    );
    std::fs::write(
        &path,
        "[pixiv.network]\nproxy_url = 'ftp://fixture-user:fixture-secret@proxy.invalid'\n",
    )
    .unwrap();
    let error = execution.use_client(None, 0, None, None).await.unwrap_err();
    assert!(matches!(error, SchedulerError::Proxy(_)));
    assert!(
        error
            .source()
            .unwrap()
            .is::<pixiv_app::connection::ProxyError>()
    );
    assert!(!format!("{error:?}").contains("fixture-secret"));
    let error = execution
        .use_client(None, 0, Some(""), None)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "lifecycle: context is nil");
}

#[tokio::test]
async fn pool_replay_refreshes_and_persists_each_selected_account_before_sdk_content() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    std::fs::write(&config_path, "[pixiv.auth]\ndefault_user_id = 43\n[account_pool]\nenabled = true\nstrategy = 'round_robin'\n").unwrap();
    let config = Store::new(config_path);
    let mut database = Database::open(directory.path()).unwrap();
    for id in [42, 43] {
        database
            .save_pixiv_credential(&PixivAccount::new(
                id,
                "stored",
                format!("fixture-refresh-{id}").as_bytes(),
            ))
            .unwrap();
    }
    database.set_all_pixiv_schedulable(true).unwrap();
    let database = Arc::new(Mutex::new(database));
    let default_config = config.clone();
    let service = Arc::new(AccountService {
        repository: database.clone(),
        defaults: Some(Arc::new(move || {
            default_config
                .read_pixiv_default_user_id()
                .map_err(Into::into)
        })),
    });
    assert_eq!(service.selected_user_id(&Context::new()).unwrap(), 43);
    let gate = Gate::new();
    let closes = Arc::new(AtomicUsize::new(0));
    let close_count = closes.clone();
    let sessions = Arc::new(ClientSessions {
        accounts: Some(Arc::new(move |context, id, connection| {
            let service = service.clone();
            Box::pin(async move {
                match service.open(&context, id, connection).await {
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
        gate: Some(gate.clone()),
        close_client: Arc::new(move |_| {
            close_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    });
    let pooled_database = database.clone();
    let facade = Facade {
        sessions,
        load_pool_config: Some(Arc::new(move || {
            config
                .current()?
                .runtime()
                .map(|runtime| runtime.account_pool)
                .map_err(Into::into)
        })),
        pool_factory: Some(Arc::new(move |config| {
            Ok(Some(pool_executor(
                config,
                Box::new(pooled_database.clone()),
                Some(Arc::new(|| {
                    chrono::DateTime::from_timestamp(1000, 0).unwrap()
                })),
            )))
        })),
    };
    let requests = Arc::new(Mutex::new(Vec::new()));
    let error = facade
        .use_client(
            Some(&Context::new()),
            99,
            Connection {
                requests: requests.clone(),
                database: database.clone(),
            },
            Some(Arc::new(|_, client: Arc<Client<Connection>>| {
                Box::pin(async move {
                    UseOutcome {
                        committed: false,
                        error: client.artwork(1).await.err().map(Into::into),
                    }
                })
            })),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.classified().unwrap().code,
        Reason::MalformedUpstreamResponse
    );
    assert_eq!(*requests.lock().unwrap(), [42, 43]);
    assert_eq!(closes.load(Ordering::SeqCst), 2);
    {
        let database = database.lock().unwrap();
        for id in [42, 43] {
            let account = database.get_pixiv(id).unwrap();
            assert_eq!(account.credential_revision, 2);
            assert_eq!(
                account.refresh_token_copy(),
                format!("fixture-rotated-{id}").as_bytes()
            );
            assert_eq!(account.pool_last_selected, id == 43);
            if id == 42 {
                assert_eq!(account.pool_frozen_until, Some(1120));
            }
        }
    }
    let context = Context::new();
    let permit = tokio::time::timeout(std::time::Duration::from_secs(5), gate.acquire(&context))
        .await
        .unwrap()
        .unwrap();
    drop(permit);
}

#[tokio::test]
async fn aborting_a_pending_client_callback_closes_its_lease_and_releases_the_gate() {
    let gate = Gate::new();
    let closes = Arc::new(AtomicUsize::new(0));
    let count = closes.clone();
    let sessions = Arc::new(ClientSessions::<i64, ()> {
        accounts: Some(Arc::new(|_, id, _| {
            Box::pin(async move {
                ClientOpen {
                    client: Some(Arc::new(id)),
                    error: None,
                }
            })
        })),
        gate: Some(gate.clone()),
        close_client: Arc::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    });
    let facade = Facade {
        sessions,
        load_pool_config: Some(Arc::new(|| {
            Ok(PoolConfig {
                enabled: false,
                strategy: "round_robin".to_owned(),
            })
        })),
        pool_factory: None,
    };
    let (started, received) = tokio::sync::oneshot::channel();
    let started = Arc::new(Mutex::new(Some(started)));
    let task = tokio::spawn(async move {
        facade
            .use_client(
                Some(&Context::new()),
                42,
                (),
                Some(Arc::new(move |_, _| {
                    started.lock().unwrap().take().unwrap().send(()).unwrap();
                    Box::pin(std::future::pending())
                })),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), received)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    let context = Context::new();
    let permit = tokio::time::timeout(std::time::Duration::from_secs(5), gate.acquire(&context))
        .await
        .unwrap()
        .unwrap();
    drop(permit);
}
