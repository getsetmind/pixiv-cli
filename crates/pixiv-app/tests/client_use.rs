use pixiv_app::{
    database::{PixivAccount, PoolCandidate, PoolChooser, PoolSnapshot},
    facade::{Facade, PoolConfig, UseOutcome, pool_executor},
    gate::Gate,
    lifecycle::Context,
    scheduler::{PoolState, SchedulerError},
    sessions::{ClientOpen, ClientSessions},
};
use pixiv_sdk::{Error, Reason, error::RetryAdvice};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Trace {
    opened: Vec<i64>,
    options: Vec<String>,
    frozen: Vec<[i64; 2]>,
    loads: usize,
    factories: usize,
    uses: usize,
    closes: usize,
}
struct State(Arc<Mutex<Trace>>);
impl PoolState for State {
    fn select(
        &mut self,
        _: &Context,
        _: i64,
        attempted: &[i64],
        chooser: Option<&mut PoolChooser<'_>>,
    ) -> Result<PixivAccount, SchedulerError> {
        let id = if attempted.is_empty() { 42 } else { 43 };
        let snapshot = PoolSnapshot {
            candidates: vec![PoolCandidate {
                user_id: id,
                sort_order: 1,
                schedulable: true,
                pool_frozen_until: None,
                pool_last_selected: false,
                eligible: true,
            }],
            marker_user_id: None,
            marker_sort_order: None,
            earliest_frozen_until: None,
        };
        let id = chooser.unwrap()(&snapshot)?;
        Ok(PixivAccount::new(id, "synthetic", b""))
    }
    fn freeze(&mut self, _: &Context, id: i64, until: i64) -> Result<(), SchedulerError> {
        self.0.lock().unwrap().frozen.push([id, until]);
        Ok(())
    }
}
fn rate() -> SchedulerError {
    Error::new(Reason::RateLimited, "Artwork")
        .with_retry(RetryAdvice {
            safe: true,
            after: Some(chrono::DateTime::from_timestamp(1120, 0).unwrap()),
        })
        .into()
}
fn message(value: &str) -> SchedulerError {
    SchedulerError::Message(value.to_owned())
}
fn optional<T: serde::Serialize>(values: &[T]) -> Value {
    if values.is_empty() {
        Value::Null
    } else {
        serde_json::to_value(values).unwrap()
    }
}

#[tokio::test]
async fn client_use_matches_go_commit_close_joined_errors_and_pool_replay() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../docs/migration/contracts/use.json")).unwrap();
    assert_eq!(cases.len(), 20);
    for expected in cases {
        let mode = expected["mode"].as_str().unwrap().to_owned();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let gate = Gate::new();
        let opened = trace.clone();
        let closed = trace.clone();
        let close_mode = mode.clone();
        let sessions = Arc::new(ClientSessions {
            accounts: Some(Arc::new(move |_, id, options: String| {
                let trace = opened.clone();
                Box::pin(async move {
                    let mut trace = trace.lock().unwrap();
                    trace.opened.push(id);
                    trace.options.push(options);
                    ClientOpen {
                        client: Some(Arc::new(id)),
                        error: None,
                    }
                })
            })),
            gate: Some(gate.clone()),
            close_client: Arc::new(move |_| {
                let mut trace = closed.lock().unwrap();
                trace.closes += 1;
                match close_mode.as_str() {
                    "close_error" | "use_close_error" => Err(message("synthetic close failure")),
                    "close_rate_retry" | "use_close_rate" | "committed_close_rate"
                        if trace.closes == 1 =>
                    {
                        Err(rate())
                    }
                    "close_canceled" | "use_close_canceled" => Err(SchedulerError::Canceled),
                    _ => Ok(()),
                }
            }),
        });
        let loaded = trace.clone();
        let load_mode = mode.clone();
        let factory = trace.clone();
        let factory_mode = mode.clone();
        let facade = Facade {
            sessions,
            load_pool_config: if mode == "missing_loader" {
                None
            } else {
                Some(Arc::new(move || {
                    loaded.lock().unwrap().loads += 1;
                    if load_mode == "loader_error" {
                        return Err(message("synthetic config failure"));
                    }
                    Ok(PoolConfig {
                        enabled: load_mode != "disabled",
                        strategy: "round_robin".to_owned(),
                    })
                }))
            },
            pool_factory: if mode == "missing_factory" {
                None
            } else {
                Some(Arc::new(move |config| {
                    factory.lock().unwrap().factories += 1;
                    if factory_mode == "factory_error" {
                        return Err(message("synthetic factory failure"));
                    }
                    if factory_mode == "nil_pool" {
                        return Ok(None);
                    }
                    Ok(Some(pool_executor(
                        config,
                        Box::new(State(factory.clone())),
                        Some(Arc::new(|| {
                            chrono::DateTime::from_timestamp(1000, 0).unwrap()
                        })),
                    )))
                }))
            },
        };
        let used = trace.clone();
        let use_mode = mode.clone();
        let callback = if mode == "nil_callback" {
            None
        } else {
            Some(Arc::new(move |_: Context, _: Arc<i64>| {
                let trace = used.clone();
                let mode = use_mode.clone();
                Box::pin(async move {
                    let mut trace = trace.lock().unwrap();
                    trace.uses += 1;
                    if mode == "use_panic" {
                        drop(trace);
                        panic!("synthetic use panic");
                    }
                    let error = match mode.as_str() {
                        "committed_retry" => Some(rate()),
                        "retry" if trace.uses == 1 => Some(rate()),
                        "use_error" | "use_close_error" | "use_close_rate"
                        | "use_close_canceled" => Some(message("synthetic use failure")),
                        _ => None,
                    };
                    UseOutcome {
                        committed: mode == "committed_retry" || mode == "committed_close_rate",
                        error,
                    }
                }) as pixiv_app::facade::UseFuture
            }) as Arc<pixiv_app::facade::UseCallback<i64>>)
        };
        let context = Context::new();
        let handle = tokio::spawn(async move {
            facade
                .use_client(
                    if mode == "nil_context" {
                        None
                    } else {
                        Some(&context)
                    },
                    99,
                    "synthetic-language".to_owned(),
                    callback,
                )
                .await
        });
        let result = handle.await;
        let panic = result.as_ref().err().is_some_and(|error| error.is_panic());
        let error = result.ok().and_then(Result::err);
        let reusable =
            Context::with_deadline(std::time::Instant::now() + std::time::Duration::from_secs(5));
        let permit = gate.acquire(&reusable).await.unwrap();
        drop(permit);
        let trace = trace.lock().unwrap();
        let actual = json!({"mode":expected["mode"],"opened":optional(&trace.opened),"options":optional(&trace.options),"frozen":optional(&trace.frozen),"loads":trace.loads,"factories":trace.factories,"uses":trace.uses,"closes":trace.closes,"message":error.as_ref().map(ToString::to_string).unwrap_or_default(),"reason":error.as_ref().and_then(SchedulerError::classified).map(|error|error.code.as_str()).unwrap_or_default(),"canceled":error.as_ref().is_some_and(SchedulerError::is_canceled),"panic":panic,"reusable":true});
        assert_eq!(actual, expected);
    }
}
