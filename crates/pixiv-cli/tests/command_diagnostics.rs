use chrono::DateTime;
use pixiv_app::config::Snapshot;
use pixiv_cli_rs::{CommandError, command_diagnostics::CommandDiagnostics, finish_command};
use pixiv_sdk::{
    context::{Context, ContextKey, RequestContext},
    diagnostics::{Event, Scope},
};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone)]
struct Writer {
    attempts: Arc<Mutex<Vec<Vec<u8>>>>,
    fail: bool,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut attempts = self.attempts.lock().unwrap();
        attempts.push(bytes.to_vec());
        if self.fail {
            if attempts.len() == 1 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                Err(io::Error::other("owned later diagnostic failure"))
            }
        } else {
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn session(fail: bool) -> (CommandDiagnostics, Arc<Mutex<Vec<Vec<u8>>>>) {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let now = DateTime::parse_from_rfc3339("2000-01-02T03:04:05Z").unwrap();
    (
        CommandDiagnostics::new(
            Some(Box::new(Writer {
                attempts: attempts.clone(),
                fail,
            })),
            Some(Arc::new(move || now)),
        ),
        attempts,
    )
}
fn runtime(level: &str, format: &str) -> pixiv_app::config::RuntimeConfig {
    let mut runtime = Snapshot::parse("", BTreeMap::new())
        .unwrap()
        .runtime()
        .unwrap();
    runtime.log_level = level.into();
    runtime.log_format = format.into();
    runtime
}
#[test]
fn frozen_go_finish_cause_cases_preserve_first_epipe_and_business_classification() {
    for (usage, ndjson, exit) in [
        (false, false, 1),
        (false, true, 0),
        (true, false, 2),
        (true, true, 0),
    ] {
        let (mut session, attempts) = session(true);
        session.start(&runtime("debug", "json"), "pixiv detail");
        let result = session.finish(if usage {
            Err(CommandError::Usage("owned business usage".into()))
        } else {
            Ok(())
        });
        assert!(result.as_ref().unwrap_err().is_broken_pipe());
        assert_eq!(result.as_ref().unwrap_err().is_usage(), usage);
        let kind = if usage { "failed" } else { "completed" };
        let reason = if usage {
            ",\"reason\":\"command failed\""
        } else {
            ""
        };
        assert_eq!(attempts.lock().unwrap()[1], format!("{{\"time\":\"2000-01-02T03:04:05Z\",\"level\":\"DEBUG\",\"module\":\"Pixiv CLI\",\"kind\":\"{kind}\",\"operation\":\"pixiv detail\"{reason}}}\n").as_bytes());
        let mut output = Vec::new();
        assert_eq!(finish_command(result, ndjson, true, &mut output), exit);
        if usage && !ndjson {
            assert_eq!(output, b"error: owned business usage\n");
        }
        if ndjson {
            assert!(output.is_empty());
        }
    }
}
#[test]
fn context_scope_replaces_sink_preserving_values_cancellation_and_caller_ports() {
    let parent_calls = Arc::new(AtomicUsize::new(0));
    let observed = parent_calls.clone();
    let key = ContextKey::new("owned key");
    let parent = Context::background()
        .with_value(key.clone(), Arc::new(42_u64))
        .with_scope(Scope::new(
            Some(Arc::new(move |_: Event| {
                observed.fetch_add(1, Ordering::SeqCst);
            })),
            "Pixiv MCP",
            88,
        ));
    let (mut session, attempts) = session(false);
    session.start(&runtime("debug", "json"), "pixiv fanbox creators");
    let scoped = session.context(&parent);
    assert_eq!(*scoped.value::<u64>(&key).unwrap(), 42);
    assert_eq!(scoped.deadline(), parent.deadline());
    scoped.emit(Event {
        kind: "planned".into(),
        ..Event::default()
    });
    let caller: Arc<dyn RequestContext> = Arc::new(parent.clone());
    let decorated = session.caller_context(caller);
    assert_eq!(
        *decorated.value(&key).unwrap().downcast::<u64>().unwrap(),
        42
    );
    assert_eq!(decorated.error(), parent.error());
    decorated.emit(Event {
        kind: "completed".into(),
        ..Event::default()
    });
    assert_eq!(parent_calls.load(Ordering::SeqCst), 0);
    for bytes in attempts.lock().unwrap().iter() {
        assert!(String::from_utf8_lossy(bytes).contains("FANBOX CLI"));
    }
}
#[test]
fn quiet_config_and_non_debug_leave_context_identity_and_writer_untouched() {
    for (level, operation) in [
        ("info", "pixiv detail"),
        ("debug", "pixiv config get"),
        ("debug", "pixiv config"),
    ] {
        let (mut session, attempts) = session(false);
        session.start(&runtime(level, "text"), operation);
        let caller: Arc<dyn RequestContext> = Arc::new(Context::background());
        assert!(Arc::ptr_eq(
            &caller,
            &session.caller_context(caller.clone())
        ));
        session.finish(Ok(())).unwrap();
        assert!(attempts.lock().unwrap().is_empty());
    }
}
#[test]
fn cleanup_result_selects_failed_before_diagnostic_finish() {
    let (mut session, attempts) = session(false);
    session.start(&runtime("debug", "text"), "pixiv detail");
    let result = pixiv_cli_rs::finish_with_cleanup(
        Ok(()),
        Err(CommandError::Message("owned cleanup failure")),
    );
    assert_eq!(
        session.finish(result).unwrap_err().to_string(),
        "owned cleanup failure"
    );
    assert_eq!(
        attempts.lock().unwrap()[1],
        b"[Pixiv CLI] 03:04:05 Pixiv detail failed: the command returned an error.\n"
    );
}

#[test]
fn sealed_root_rows_replay_start_finish_record_bytes_without_claiming_business_execution() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/diagnostics-root.json")).unwrap();
    let rows = fixture["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 19);
    for row in rows {
        let input = &row["input"];
        let operation = row["observation"]["target"].as_str().unwrap();
        let format = input["environment"]["PIXIV_LOG_FORMAT"].as_str().unwrap();
        let level = input["environment"]["PIXIV_LOG_LEVEL"].as_str().unwrap();
        let writes = row["observation"]["error_writes"].as_array().unwrap();
        let (mut session, attempts) = session(false);
        session.start(&runtime(level, format), operation);
        let failed = writes
            .get(1)
            .is_some_and(|write| write["payload"].as_str().unwrap().contains("failed"));
        session
            .finish(if failed {
                Err(CommandError::Message("owned business or cleanup failure"))
            } else {
                Ok(())
            })
            .ok();
        let attempts = attempts.lock().unwrap();
        let expected: Vec<Vec<u8>> = writes
            .iter()
            .filter_map(|write| {
                let payload = write["payload"].as_str().unwrap();
                (payload.starts_with("[Pixiv CLI]")
                    || payload.starts_with("[FANBOX CLI]")
                    || payload.starts_with("{\"time\":"))
                .then(|| payload.as_bytes().to_vec())
            })
            .collect();
        assert_eq!(*attempts, expected, "{}", row["name"]);
    }
}

#[tokio::test]
async fn scoped_context_delegates_deadline_extensions_and_cancel_future() {
    use std::{
        any::TypeId,
        time::{Duration, Instant},
    };
    let parent = Context::background()
        .child_with_deadline(Instant::now() + Duration::from_secs(60))
        .with_extension(Arc::new(42_u64));
    let (mut session, _) = session(false);
    session.start(&runtime("debug", "json"), "pixiv detail");
    let scoped = session.context(&parent);
    let caller = session.caller_context(Arc::new(parent.clone()));
    assert_eq!(caller.deadline(), parent.deadline());
    assert_eq!(
        *caller
            .extension(TypeId::of::<u64>())
            .unwrap()
            .downcast::<u64>()
            .unwrap(),
        42
    );
    parent.cancel();
    assert_eq!(scoped.error(), parent.error());
    assert_eq!(caller.error(), parent.error());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), caller.cancelled())
            .await
            .unwrap(),
        pixiv_sdk::context::ContextError::Canceled
    );
}

#[test]
fn supplemental_go_joined_pipeline_startup_classification_precedes_extra_error_output() {
    struct FailWriter {
        epipe: bool,
        attempts: Arc<Mutex<Vec<Vec<u8>>>>,
    }
    impl Write for FailWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.attempts.lock().unwrap().push(bytes.to_vec());
            Err(if self.epipe {
                io::Error::from_raw_os_error(32)
            } else {
                io::Error::other("owned diagnostic writer failure")
            })
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/diagnostics-root-joined.json")).unwrap();
    let rows = fixture["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 6);
    for row in rows {
        let epipe = row["input"]["diagnostic_writer"]
            .as_str()
            .unwrap()
            .contains("epipe");
        let startup = row["input"]["business_error"] == "startup";
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let now = DateTime::parse_from_rfc3339("2000-01-02T03:04:05Z").unwrap();
        let mut session = CommandDiagnostics::new(
            Some(Box::new(FailWriter {
                epipe,
                attempts: attempts.clone(),
            })),
            Some(Arc::new(move || now)),
        );
        let start_record: serde_json::Value = serde_json::from_str(
            row["observation"]["error_writes"][0]["payload"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let operation = start_record["operation"].as_str().unwrap();
        // The fixed-clock Go fixture exercises finish after a source-classified business error.
        // It does not claim real startup reaches an active production diagnostic session.
        session.start(&runtime("debug", "json"), operation);
        let result = session.finish(Err(if startup {
            CommandError::Startup("owned startup cause".into())
        } else {
            CommandError::Pipeline
        }));
        assert_eq!(result.as_ref().unwrap_err().is_broken_pipe(), epipe);
        let mut output = Vec::new();
        let exit = finish_command(
            result,
            row["input"]["ndjson_output"].as_bool().unwrap(),
            true,
            &mut output,
        );
        assert_eq!(
            exit,
            row["observation"]["exit"].as_i64().unwrap() as i32,
            "{}",
            row["name"]
        );
        assert_eq!(
            output,
            row["observation"]["stderr"].as_str().unwrap().as_bytes(),
            "{}",
            row["name"]
        );
        let expected: Vec<Vec<u8>> = row["observation"]["error_writes"]
            .as_array()
            .unwrap()
            .iter()
            .take(2)
            .map(|write| write["payload"].as_str().unwrap().as_bytes().to_vec())
            .collect();
        assert_eq!(*attempts.lock().unwrap(), expected, "{}", row["name"]);
    }
}
