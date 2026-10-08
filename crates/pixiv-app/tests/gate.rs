use pixiv_app::{
    gate::{Gate, GateCallback},
    lifecycle::Context,
    scheduler::{AttemptFuture, SchedulerError},
};
use serde::Deserialize;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Deserialize)]
struct Case {
    mode: String,
    error: String,
    canceled: bool,
    deadline: bool,
    called: bool,
    reusable: bool,
}

#[tokio::test]
async fn rotation_gate_matches_go_validation_cancellation_callback_failure_and_panic_release() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../../../docs/migration/contracts/gate.json")).unwrap();
    assert_eq!(cases.len(), 12);
    for case in cases {
        let gate = if case.mode.starts_with("nil_") && case.mode != "nil_function"
            || case.mode.starts_with("zero_")
        {
            Gate::default()
        } else {
            Gate::new()
        };
        let context = if case.mode == "occupied_deadline" {
            Context::with_deadline(std::time::Instant::now() - std::time::Duration::from_secs(1))
        } else {
            Context::new()
        };
        let called = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&called);
        let mode = case.mode.clone();
        let mut callback = move |context: Context| -> AttemptFuture<'_> {
            let seen = Arc::clone(&seen);
            let mode = mode.clone();
            Box::pin(async move {
                seen.store(true, Ordering::SeqCst);
                match mode.as_str() {
                    "failure" => Err(SchedulerError::Message("synthetic callback error".into())),
                    "cancel_callback" => {
                        context.cancel();
                        Err(SchedulerError::Message("synthetic callback error".into()))
                    }
                    "panic" => panic!("synthetic panic"),
                    _ => Ok(()),
                }
            })
        };
        let result = match case.mode.as_str() {
            "nil_acquire" | "zero_acquire" => gate.acquire(&context).await.map(drop),
            "nil_function" => gate.run(&context, None).await,
            "occupied_canceled" | "occupied_deadline" | "pending_canceled" => {
                let permit = gate.acquire(&Context::new()).await.unwrap();
                if case.mode == "occupied_canceled" {
                    context.cancel();
                }
                let result = if case.mode == "pending_canceled" {
                    let (started, ready) = tokio::sync::oneshot::channel();
                    let wait = async {
                        started.send(()).unwrap();
                        gate.run(&context, Some(&mut callback as &mut GateCallback<'_>))
                            .await
                    };
                    let cancel = async {
                        ready.await.unwrap();
                        context.cancel();
                    };
                    tokio::time::timeout(std::time::Duration::from_secs(10), async {
                        tokio::join!(wait, cancel).0
                    })
                    .await
                    .unwrap()
                } else {
                    gate.run(&context, Some(&mut callback)).await
                };
                drop(permit);
                result
            }
            "panic" => {
                let gate = gate.clone();
                let context = context.clone();
                let failed =
                    tokio::spawn(async move { gate.run(&context, Some(&mut callback)).await })
                        .await;
                assert!(failed.unwrap_err().is_panic());
                Ok(())
            }
            _ => gate.run(&context, Some(&mut callback)).await,
        };
        let error = result.err();
        assert_eq!(
            error.as_ref().map(ToString::to_string).unwrap_or_default(),
            case.error,
            "{case:?}"
        );
        assert_eq!(
            error.as_ref().is_some_and(SchedulerError::is_canceled),
            case.canceled,
            "{case:?}"
        );
        assert_eq!(
            error
                .as_ref()
                .is_some_and(SchedulerError::is_deadline_exceeded),
            case.deadline,
            "{case:?}"
        );
        assert_eq!(called.load(Ordering::SeqCst), case.called, "{case:?}");
        if case.reusable {
            let permit = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                gate.acquire(&Context::new()),
            )
            .await
            .unwrap()
            .unwrap();
            drop(permit);
        }
    }
}

#[tokio::test]
async fn rotation_gate_serializes_clones_and_releases_an_aborted_owner() {
    let gate = Gate::new();
    let (started, ready) = tokio::sync::oneshot::channel();
    let owner_gate = gate.clone();
    let owner = tokio::spawn(async move {
        let _permit = owner_gate.acquire(&Context::new()).await.unwrap();
        started.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    ready.await.unwrap();
    let occupied =
        Context::with_deadline(std::time::Instant::now() + std::time::Duration::from_millis(20));
    let error = gate.acquire(&occupied).await.unwrap_err();
    assert!(error.is_deadline_exceeded());
    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());
    let _permit = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        gate.acquire(&Context::new()),
    )
    .await
    .unwrap()
    .unwrap();
}
