use pixiv_app::{
    gate::Gate,
    lifecycle::Context,
    scheduler::SchedulerError,
    sessions::{ClientOpen, ClientOpenFuture, ClientOpener, ClientSessions, CloseClient},
};
use pixiv_sdk::{
    Client, Error, Reason,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

struct Quiet;
impl Transport for Quiet {
    async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
        panic!("unexpected content request")
    }
}
type SdkClient = Client<Quiet>;

#[derive(Deserialize)]
struct Case {
    mode: String,
    user_id: i64,
    opened: Value,
    options: Value,
    error: String,
    close_errors: Value,
    close_calls: usize,
    has_lease: bool,
    held: bool,
    reusable: bool,
    panic: String,
    canceled: bool,
    reason: String,
}
#[derive(Default)]
struct Trace {
    opened: Option<Vec<i64>>,
    options: Option<Vec<String>>,
    close_calls: usize,
}

#[tokio::test]
async fn client_sessions_match_go_open_close_errors_and_gate_ownership() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/sessions.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 17);
    for case in cases {
        let trace = Arc::new(Mutex::new(Trace::default()));
        let opened = Arc::clone(&trace);
        let mode = case.mode.clone();
        let opener: Arc<ClientOpener<SdkClient, String>> = Arc::new(
            move |_: Context, id: i64, options: String| -> ClientOpenFuture<SdkClient> {
                {
                    let mut trace = opened.lock().unwrap();
                    trace.opened.get_or_insert_default().push(id);
                    trace.options.get_or_insert_default().push(options);
                }
                let mode = mode.clone();
                Box::pin(async move {
                    if mode == "open_panic" {
                        panic!("synthetic opener panic");
                    }
                    let client = if mode == "nil_client" || mode == "open_error" {
                        None
                    } else {
                        Some(Arc::new(Client::with_transport("synthetic-access", Quiet)))
                    };
                    let error = if mode == "open_error" || mode.starts_with("partial_") {
                        Some(SchedulerError::Message("synthetic open failure".into()))
                    } else {
                        None
                    };
                    ClientOpen { client, error }
                })
            },
        );
        let closed = Arc::clone(&trace);
        let mode = case.mode.clone();
        let closer: Arc<CloseClient<SdkClient>> = Arc::new(move |_| {
            closed.lock().unwrap().close_calls += 1;
            match mode.as_str() {
                "close_error" | "partial_close_error" => {
                    Err(SchedulerError::Message("synthetic close failure".into()))
                }
                "partial_close_canceled" => Err(SchedulerError::Canceled),
                "partial_close_sdk" => Err(Error::new(Reason::RateLimited, "close").into()),
                "close_panic" => panic!("synthetic closer panic"),
                _ => Ok(()),
            }
        });
        let gate = if case.mode == "zero_gate" {
            Gate::default()
        } else {
            Gate::new()
        };
        let context = Context::new();
        let held = if case.mode == "occupied_canceled" {
            let permit = gate.acquire(&context).await.unwrap();
            context.cancel();
            Some(permit)
        } else {
            None
        };
        let sessions = ClientSessions {
            accounts: if case.mode == "missing_accounts" {
                None
            } else {
                Some(opener)
            },
            gate: if case.mode == "missing_gate" {
                None
            } else {
                Some(gate.clone())
            },
            close_client: closer,
        };
        let context_arg = if case.mode == "nil_context" {
            None
        } else {
            Some(&context)
        };
        let mut panic_text = String::new();
        let result = if case.mode == "open_panic" {
            let context = context.clone();
            let result = tokio::spawn(async move {
                sessions
                    .open(Some(&context), case.user_id, "ja-JP".to_owned())
                    .await
            })
            .await;
            match result {
                Err(error) => {
                    assert!(error.is_panic());
                    let payload = error.into_panic();
                    assert_eq!(
                        payload.downcast_ref::<&str>(),
                        Some(&"synthetic opener panic")
                    );
                }
                Ok(_) => panic!("opener did not panic"),
            };
            panic_text = "synthetic opener panic".into();
            None
        } else {
            Some(
                sessions
                    .open(context_arg, case.user_id, "ja-JP".to_owned())
                    .await,
            )
        };
        let mut has_lease = false;
        let mut retained = false;
        let mut close_errors: Option<Vec<String>> = None;
        let mut error = String::new();
        let mut canceled = false;
        let mut reason = String::new();
        match result {
            Some(Ok(lease)) => {
                has_lease = true;
                let probe = Context::with_deadline(
                    std::time::Instant::now() + std::time::Duration::from_millis(1),
                );
                retained = gate
                    .acquire(&probe)
                    .await
                    .err()
                    .unwrap()
                    .is_deadline_exceeded();
                for _ in 0..3 {
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lease.close())) {
                        Ok(result) => close_errors.get_or_insert_default().push(
                            result
                                .err()
                                .map(|error| error.to_string())
                                .unwrap_or_default(),
                        ),
                        Err(payload) => {
                            assert_eq!(
                                payload.downcast_ref::<&str>(),
                                Some(&"synthetic closer panic")
                            );
                            panic_text = "synthetic closer panic".into();
                        }
                    }
                }
            }
            Some(Err(failure)) => {
                canceled = failure.is_canceled();
                reason = failure
                    .classified()
                    .map(|error| error.code.as_str().to_owned())
                    .unwrap_or_default();
                error = failure.to_string();
            }
            None => {}
        }
        drop(held);
        let reusable = if case.mode != "zero_gate" {
            drop(
                tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    gate.acquire(&Context::new()),
                )
                .await
                .unwrap()
                .unwrap(),
            );
            true
        } else {
            false
        };
        let trace = trace.lock().unwrap();
        assert_eq!(json!(trace.opened), case.opened, "{}", case.mode);
        assert_eq!(json!(trace.options), case.options, "{}", case.mode);
        assert_eq!(error, case.error, "{}", case.mode);
        assert_eq!(canceled, case.canceled, "{}", case.mode);
        assert_eq!(reason, case.reason, "{}", case.mode);
        assert_eq!(json!(close_errors), case.close_errors, "{}", case.mode);
        assert_eq!(trace.close_calls, case.close_calls, "{}", case.mode);
        assert_eq!(has_lease, case.has_lease, "{}", case.mode);
        assert_eq!(retained, case.held, "{}", case.mode);
        assert_eq!(reusable, case.reusable, "{}", case.mode);
        assert_eq!(panic_text, case.panic, "{}", case.mode);
    }
}

#[tokio::test]
async fn aborting_a_pending_client_opener_releases_the_rotation_gate() {
    let gate = Gate::new();
    let (started, ready) = tokio::sync::oneshot::channel();
    let started = Arc::new(Mutex::new(Some(started)));
    let opener: Arc<ClientOpener<SdkClient, ()>> = Arc::new(move |_, _, _| {
        let started = started.lock().unwrap().take().unwrap();
        Box::pin(async move {
            started.send(()).unwrap();
            std::future::pending::<ClientOpen<SdkClient>>().await
        })
    });
    let sessions = ClientSessions {
        accounts: Some(opener),
        gate: Some(gate.clone()),
        close_client: Arc::new(|_| panic!("client was never returned")),
    };
    let owner = tokio::spawn(async move { sessions.open(Some(&Context::new()), 7, ()).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), ready)
        .await
        .unwrap()
        .unwrap();
    let probe =
        Context::with_deadline(std::time::Instant::now() + std::time::Duration::from_millis(20));
    assert!(
        gate.acquire(&probe)
            .await
            .err()
            .unwrap()
            .is_deadline_exceeded()
    );
    owner.abort();
    match owner.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("opener was not aborted"),
    };
    drop(
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            gate.acquire(&Context::new()),
        )
        .await
        .unwrap()
        .unwrap(),
    );
}
