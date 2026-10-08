use chrono::{DateTime, Utc};
use pixiv_app::{
    database::{Database, PixivAccount, PoolChooser, PoolError},
    lifecycle::{Attempt, Context},
    scheduler::{AttemptFuture, PoolState, Scheduler, SchedulerError},
};
use pixiv_sdk::{Error, Reason, error::RetryAdvice};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
struct Case {
    name: String,
    mode: String,
    commit: bool,
    cancel: String,
    once: bool,
    state: String,
    enabled: bool,
    strategy: String,
    step_ns: i64,
    missing_attempt: bool,
    error: Value,
    attempts: Value,
    selects: Value,
    clocks: Value,
    freezes: Value,
    rows: Value,
}

#[derive(Default)]
struct Trace {
    attempts: Option<Vec<i64>>,
    selects: Option<Vec<Vec<i64>>>,
    clocks: Option<Vec<i64>>,
    freezes: Option<Vec<Vec<i64>>>,
}

struct State {
    database: Database,
    mode: String,
    trace: Arc<Mutex<Trace>>,
}
impl PoolState for State {
    fn select(
        &mut self,
        context: &Context,
        now: i64,
        attempted: &[i64],
        chooser: Option<&mut PoolChooser<'_>>,
    ) -> Result<PixivAccount, SchedulerError> {
        self.trace
            .lock()
            .unwrap()
            .selects
            .get_or_insert_default()
            .push(attempted.to_vec());
        match self.mode.as_str() {
            "storage_error" => {
                return Err(SchedulerError::Message("synthetic storage error".into()));
            }
            "unknown" => return Err(PoolError::UnknownSelection("unknown".into()).into()),
            "sentinel" => return Err(SchedulerError::Exhausted(None)),
            "sentinel_after" if !attempted.is_empty() => {
                return Err(SchedulerError::Exhausted(None));
            }
            "invalid_uid" => return Ok(PixivAccount::new(0, "", b"")),
            "repeated_uid" => return Ok(PixivAccount::new(1, "", b"")),
            _ => {}
        }
        <Database as PoolState>::select(&mut self.database, context, now, attempted, chooser)
    }
    fn freeze(&mut self, context: &Context, id: i64, until: i64) -> Result<(), SchedulerError> {
        self.trace
            .lock()
            .unwrap()
            .freezes
            .get_or_insert_default()
            .push(vec![id, until]);
        if self.mode == "freeze_error" {
            return Err(SchedulerError::Message("synthetic freeze error".into()));
        }
        <Database as PoolState>::freeze(&mut self.database, context, id, until)
    }
}

fn error_value(result: Result<(), SchedulerError>) -> Value {
    match result {
        Ok(()) => {
            json!({"text":"","classified":null,"exhausted":false,"canceled":false,"deadline":false})
        }
        Err(error) => {
            let classified = error.classified().map(|typed| json!({"reason":typed.code,"product":typed.product,"operation":typed.operation,"detail":typed.detail.as_deref().unwrap_or(""),"safe":typed.retry.safe,"after_ns":typed.retry.after.map(|after|after.timestamp_nanos_opt().unwrap())}));
            json!({"text":error.to_string(),"classified":classified,"exhausted":error.is_exhausted(),"canceled":error.is_canceled(),"deadline":error.is_deadline_exceeded()})
        }
    }
}

#[tokio::test]
async fn scheduler_matches_go_replay_commit_cancellation_exhaustion_and_database_state() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/scheduler.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 97);
    let base = DateTime::<Utc>::from_timestamp_nanos(1_000_250_000_000);
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let mut state = State {
            database: Database::open(directory.path()).unwrap(),
            mode: case.state.clone(),
            trace: Arc::clone(&trace),
        };
        if case.state != "empty" {
            for id in [1, 2, 3] {
                state
                    .database
                    .save_pixiv_credential(&PixivAccount::new(id, "synthetic", b"synthetic-token"))
                    .unwrap();
            }
        }
        if case.state == "disabled" {
            state.database.set_all_pixiv_schedulable(false).unwrap();
        }
        if case.state == "frozen" {
            for id in [1, 2, 3] {
                state.database.freeze_pixiv(id, 1500).unwrap();
            }
        }
        let context = if case.cancel == "deadline" {
            Context::with_deadline(std::time::Instant::now() - std::time::Duration::from_secs(1))
        } else {
            Context::new()
        };
        if case.cancel == "before" {
            context.cancel();
        }
        let mut tick = 0;
        let clock_trace = Arc::clone(&trace);
        let mut clock = || {
            let nanos = 1_000_250_000_000 + tick * case.step_ns;
            tick += 1;
            clock_trace
                .lock()
                .unwrap()
                .clocks
                .get_or_insert_default()
                .push(nanos);
            DateTime::<Utc>::from_timestamp_nanos(nanos)
        };
        let attempt_trace = Arc::clone(&trace);
        let mut attempt = |context: Context, id: i64, commit: Arc<Attempt>| -> AttemptFuture<'_> {
            let trace = Arc::clone(&attempt_trace);
            let mode = case.mode.clone();
            let should_commit = case.commit;
            let should_cancel = case.cancel == "during";
            let once = case.once;
            Box::pin(async move {
                let count = {
                    let mut trace = trace.lock().unwrap();
                    let attempts = trace.attempts.get_or_insert_default();
                    attempts.push(id);
                    attempts.len()
                };
                if should_commit {
                    commit.commit();
                    commit.commit();
                }
                if should_cancel {
                    context.cancel();
                }
                if once && count > 1 {
                    return Ok(());
                }
                let reason = if mode == "wrong_reason" {
                    Reason::UpstreamUnavailable
                } else {
                    Reason::RateLimited
                };
                let after = if mode == "missing_after" {
                    None
                } else if mode == "past" {
                    Some(base)
                } else {
                    Some(base + chrono::TimeDelta::milliseconds(3500))
                };
                let typed = Error::new(reason, "read")
                    .with_detail("synthetic_attempt")
                    .with_retry(RetryAdvice {
                        safe: mode != "unsafe",
                        after,
                    });
                match mode.as_str() {
                    "success" => Ok(()),
                    "plain" => Err(SchedulerError::Message("synthetic attempt error".into())),
                    "wrapped" => Err(SchedulerError::Wrapped {
                        message: "wrapped".into(),
                        source: Box::new(typed.into()),
                    }),
                    "cancel_raw" => Err(SchedulerError::Canceled),
                    "deadline_raw" => Err(SchedulerError::DeadlineExceeded),
                    _ => Err(typed.into()),
                }
            })
        };
        let mut scheduler = Scheduler {
            enabled: case.enabled,
            strategy: &case.strategy,
            state: if case.state == "nil" {
                None
            } else {
                Some(&mut state)
            },
            now: Some(&mut clock),
            random: None,
        };
        let callback = if case.missing_attempt {
            None
        } else {
            Some(&mut attempt as &mut pixiv_app::scheduler::AttemptCallback<'_>)
        };
        let result = scheduler.run(&context, callback).await;
        assert_eq!(error_value(result), case.error, "{}", case.name);
        let trace = trace.lock().unwrap();
        assert_eq!(json!(trace.attempts), case.attempts, "{}", case.name);
        assert_eq!(json!(trace.selects), case.selects, "{}", case.name);
        assert_eq!(json!(trace.clocks), case.clocks, "{}", case.name);
        assert_eq!(json!(trace.freezes), case.freezes, "{}", case.name);
        let rows: Vec<_>=state.database.list_pixiv().unwrap().into_iter().map(|a|json!({"id":a.user_id,"schedulable":a.schedulable,"frozen":a.pool_frozen_until,"selected":a.pool_last_selected})).collect();
        assert_eq!(json!(rows), case.rows, "{}", case.name);
    }
}

#[tokio::test]
async fn pending_attempt_observes_external_commit_and_cancellation_without_replay() {
    for committed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let mut state = State {
            database: Database::open(directory.path()).unwrap(),
            mode: String::new(),
            trace: Arc::clone(&trace),
        };
        state
            .database
            .save_pixiv_credential(&PixivAccount::new(1, "synthetic", b"synthetic-token"))
            .unwrap();
        let context = Context::new();
        let base = DateTime::<Utc>::from_timestamp_nanos(1_000_250_000_000);
        let mut clock = || base;
        let mut scheduler = Scheduler {
            enabled: true,
            strategy: "round_robin",
            state: Some(&mut state),
            now: Some(&mut clock),
            random: None,
        };
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut sender = Some(sender);
        let mut attempt = |context: Context, _: i64, attempt: Arc<Attempt>| -> AttemptFuture<'_> {
            let sender = sender.take().unwrap();
            Box::pin(async move {
                sender.send(attempt).unwrap();
                context.cancelled().await;
                Err(Error::new(Reason::RateLimited, "read")
                    .with_retry(RetryAdvice {
                        safe: true,
                        after: Some(base + chrono::TimeDelta::seconds(1)),
                    })
                    .into())
            })
        };
        let control = async {
            let attempt: Arc<Attempt> = receiver.await.unwrap();
            if committed {
                attempt.commit();
                attempt.commit();
            }
            context.cancel();
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(scheduler.run(&context, Some(&mut attempt)), control)
        })
        .await
        .unwrap();
        let error = result.unwrap_err();
        if committed {
            assert_eq!(error.classified().unwrap().code, Reason::RateLimited);
            assert!(!error.is_canceled());
        } else {
            assert!(error.is_canceled());
        }
        let trace = trace.lock().unwrap();
        assert_eq!(trace.selects, Some(vec![vec![]]));
        assert!(trace.freezes.is_none());
    }
}

#[tokio::test]
async fn context_wakes_on_deadline_and_keeps_the_first_cancellation_cause() {
    use pixiv_app::lifecycle::ContextError;
    let context =
        Context::with_deadline(std::time::Instant::now() + std::time::Duration::from_millis(5));
    let cause = tokio::time::timeout(std::time::Duration::from_secs(5), context.cancelled())
        .await
        .unwrap();
    assert_eq!(cause, ContextError::DeadlineExceeded);
    context.cancel();
    assert_eq!(context.error(), Some(ContextError::DeadlineExceeded));
    let context =
        Context::with_deadline(std::time::Instant::now() + std::time::Duration::from_millis(500));
    context.cancel();
    tokio::time::sleep(std::time::Duration::from_millis(510)).await;
    assert_eq!(context.cancelled().await, ContextError::Canceled);
}
