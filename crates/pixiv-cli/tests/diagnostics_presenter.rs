#[path = "support/diagnostics_presenter.rs"]
mod support;

use chrono::{DateTime, Local};
use pixiv_cli_rs::diagnostics::{Clock, Presenter};
use pixiv_sdk::diagnostics::{Scope, Sink};
use std::{
    io,
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use support::*;

fn counted_clock(clock: &ClockInput) -> (Clock, Arc<AtomicUsize>) {
    let value = clock.value();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    (
        Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            value
        }),
        calls,
    )
}
fn error_text(presenter: &Presenter) -> String {
    presenter
        .error()
        .map(|error| error.to_string())
        .unwrap_or_default()
}

#[test]
fn frozen_shared_presenter_records_match_exact_go_bytes() {
    let primary = fixture();
    let extra = extra_fixture();
    assert_eq!(primary.schema_version, 1);
    assert_eq!(extra.schema_version, 1);
    assert_eq!(
        primary.frozen_go_ref,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(extra.frozen_go_ref, primary.frozen_go_ref);
    assert_eq!(primary.render_cases.len(), 170);
    assert_eq!(extra.render_cases.len(), 22);
    let mut go_only = Vec::new();
    for row in primary.render_cases.into_iter().chain(extra.render_cases) {
        if row.compatibility == "go_only_string_representation" {
            go_only.push(row.name);
            continue;
        }
        assert!(
            matches!(row.compatibility.as_str(), "shared" | "shared_on_64_bit_go"),
            "{}",
            row.name
        );
        let capture = Capture::default();
        let writer: Option<Box<dyn io::Write + Send>> =
            (!row.nil_writer).then(|| Box::new(capture.clone()) as Box<dyn io::Write + Send>);
        let (clock, calls) = counted_clock(&row.clock);
        let presenter = match row.constructor.as_str() {
            "clock" => Presenter::with_clock(writer, Some(clock)),
            "format" => Presenter::with_format(writer, &row.format, Some(clock)),
            other => panic!("unknown constructor {other}"),
        };
        presenter.emit(row.event.event());
        let records = capture.0.lock().unwrap();
        let actual: Vec<u8> = records.iter().flatten().copied().collect();
        assert_eq!(actual, row.output.bytes(), "{}", row.name);
        assert_eq!(records.len(), usize::from(!row.nil_writer), "{}", row.name);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            row.clock_calls,
            "{}",
            row.name
        );
        assert_eq!(error_text(&presenter), row.error, "{}", row.name);
    }
    assert_eq!(
        go_only,
        [
            "invalid_go_string_bytes_text",
            "invalid_go_string_bytes_json"
        ]
    );
}

#[test]
fn frozen_signed_subminute_clock_offsets_match_exact_go_records() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/diagnostics-presenter-offset.json")).unwrap();
    assert_eq!(fixture.schema_version, 1);
    assert_eq!(
        fixture.frozen_go_ref,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture.render_cases.len(), 14);
    assert_eq!(
        fixture
            .render_cases
            .chunks_exact(2)
            .map(|pair| (pair[0].clock.offset_seconds, pair[1].clock.offset_seconds))
            .collect::<Vec<_>>(),
        [
            (-1, -1),
            (-59, -59),
            (-60, -60),
            (1, 1),
            (59, 59),
            (60, 60),
            (0, 0)
        ]
    );
    for row in fixture.render_cases {
        assert_eq!(row.compatibility, "shared", "{}", row.name);
        assert_eq!(row.constructor, "format", "{}", row.name);
        assert!(!row.nil_writer, "{}", row.name);
        let capture = Capture::default();
        let (clock, calls) = counted_clock(&row.clock);
        let presenter =
            Presenter::with_format(Some(Box::new(capture.clone())), &row.format, Some(clock));
        presenter.emit(row.event.event());
        assert_eq!(
            *capture.0.lock().unwrap(),
            [row.output.bytes()],
            "{}",
            row.name
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            row.clock_calls,
            "{}",
            row.name
        );
        assert_eq!(error_text(&presenter), row.error, "{}", row.name);
    }
}

#[test]
fn writer_records_first_error_identity_and_keeps_attempting_after_errors_and_short_successes() {
    let fixture = fixture();
    assert_eq!(fixture.writer_cases.len(), 6);
    let mut go_only = Vec::new();
    for row in fixture.writer_cases {
        if row.name == "invalid_writer_counts" {
            assert_eq!(row.compatibility, "go_only_writer_contract_violation");
            assert_eq!(row.calls[0].returned_n, -1);
            assert_eq!(
                row.calls[1].returned_n,
                row.calls[1].record.bytes().len() as i64 + 1
            );
            go_only.push(row.name);
            continue;
        }
        assert_eq!(row.compatibility, "shared");
        let state = Arc::new(Mutex::new(WriterState::default()));
        let first = Marker {
            token: Arc::new(()),
            text: "synthetic first writer error",
        };
        let later = Marker {
            token: Arc::new(()),
            text: "synthetic later writer error",
        };
        let writer = ScriptedWriter {
            steps: row.steps,
            state: state.clone(),
            first: first.clone(),
            later: later.clone(),
        };
        let (clock, calls) = counted_clock(&row.clock);
        let presenter = Presenter::with_format(Some(Box::new(writer)), &row.format, Some(clock));
        let mut retained = None;
        for (event, expected) in row.events.iter().zip(&row.error_after_each_emit) {
            presenter.emit(event.event());
            let error = presenter.error();
            assert_eq!(error_text(&presenter), expected.error, "{}", row.name);
            let marker = error
                .as_ref()
                .and_then(|error| error.get_ref())
                .and_then(|error| error.downcast_ref::<Marker>());
            assert_eq!(
                marker.is_some_and(|marker| Arc::ptr_eq(&marker.token, &first.token)),
                expected.is_first_error_identity,
                "{}",
                row.name
            );
            assert_eq!(
                marker.is_some_and(|marker| Arc::ptr_eq(&marker.token, &later.token)),
                expected.is_later_error_identity,
                "{}",
                row.name
            );
            if let Some(error) = error {
                assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
                if let Some(retained) = &retained {
                    assert!(Arc::ptr_eq(retained, &error), "{}", row.name);
                } else {
                    retained = Some(error);
                }
            }
        }
        let state = state.lock().unwrap();
        assert_eq!(state.calls.len(), row.calls.len(), "{}", row.name);
        for ((bytes, count, error), expected) in state.calls.iter().zip(&row.calls) {
            assert_eq!(*bytes, expected.record.bytes(), "{}", row.name);
            assert_eq!(*count as i64, expected.returned_n, "{}", row.name);
            assert_eq!(*error, expected.error, "{}", row.name);
            // Go io.StringWriter dispatch maps to one Rust Write::write record, including partial writes with errors.
            assert_eq!(
                expected.method,
                if row.implements_string_writer {
                    "WriteString"
                } else {
                    "Write"
                }
            );
        }
        assert_eq!(state.stored, row.stored_output.bytes(), "{}", row.name);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            row.clock_calls,
            "{}",
            row.name
        );
    }
    assert_eq!(go_only, ["invalid_writer_counts"]);
}

#[test]
fn default_constructors_use_local_clock_and_none_writer_discards_records() {
    let fixture = fixture();
    assert_eq!(fixture.default_cases.len(), 7);
    for row in fixture.default_cases {
        assert!(row.nil_clock && row.clock_in_call_window);
        let capture = Capture::default();
        let writer =
            (!row.nil_writer).then(|| Box::new(capture.clone()) as Box<dyn io::Write + Send>);
        let presenter = match row.constructor.as_str() {
            "default" => Presenter::new(writer),
            "clock" => Presenter::with_clock(writer, None),
            "format" => Presenter::with_format(writer, &row.format, None),
            other => panic!("unknown constructor {other}"),
        };
        let before = Local::now();
        presenter.emit(row.event.event());
        let after = Local::now();
        let bytes: Vec<u8> = capture
            .0
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .copied()
            .collect();
        let output = String::from_utf8(bytes).unwrap();
        let normalized = if row.nil_writer {
            output
        } else if row.format == "json" {
            let value: serde_json::Value = serde_json::from_str(&output).unwrap();
            let stamp = value["time"].as_str().unwrap();
            let parsed = DateTime::parse_from_rfc3339(stamp).unwrap();
            assert!(parsed >= before && parsed <= after, "{}", row.name);
            output.replacen(stamp, "<LOCAL_CLOCK>", 1)
        } else {
            let stamp = output.get(12..20).unwrap();
            assert!(
                stamp == before.format("%H:%M:%S").to_string()
                    || stamp == after.format("%H:%M:%S").to_string(),
                "{}",
                row.name
            );
            output.replacen(stamp, "<LOCAL_CLOCK>", 1)
        };
        assert_eq!(normalized, row.normalized_output, "{}", row.name);
        assert_eq!(error_text(&presenter), row.error, "{}", row.name);
    }
}

#[test]
fn concurrent_rendering_keeps_clock_parallel_and_writer_records_intact() {
    let fixture = fixture();
    assert_eq!(fixture.concurrent_cases.len(), 2);
    for row in fixture.concurrent_cases {
        let capture = Capture::default();
        let writes = Arc::new(Activity::default());
        let clocks = Arc::new(Activity::default());
        let observed = clocks.clone();
        let barrier = Arc::new(Barrier::new(row.events.len()));
        let value = row.clock.value();
        let clock: Clock = Arc::new(move || {
            observed.enter();
            barrier.wait();
            observed.exit();
            value
        });
        let presenter = Arc::new(Presenter::with_format(
            Some(Box::new(ConcurrentWriter {
                capture: capture.clone(),
                activity: writes.clone(),
            })),
            &row.format,
            Some(clock),
        ));
        let workers: Vec<_> = row
            .events
            .into_iter()
            .map(|event| {
                let presenter = presenter.clone();
                std::thread::spawn(move || presenter.emit(event.event()))
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let mut records = capture.0.lock().unwrap().clone();
        records.sort();
        assert_eq!(
            records,
            row.sorted_records
                .iter()
                .map(Output::bytes)
                .collect::<Vec<_>>(),
            "{}",
            row.name
        );
        assert_eq!(
            clocks.calls.load(Ordering::SeqCst),
            row.clock_calls,
            "{}",
            row.name
        );
        assert_eq!(
            clocks.maximum.load(Ordering::SeqCst),
            row.max_concurrent_clock_calls,
            "{}",
            row.name
        );
        assert_eq!(
            writes.maximum.load(Ordering::SeqCst),
            row.max_concurrent_writer_calls,
            "{}",
            row.name
        );
        assert_eq!(
            records.iter().all(
                |record| record.iter().filter(|byte| **byte == b'\n').count() == 1
                    && record.last() == Some(&b'\n')
            ),
            row.intact_single_records
        );
        assert_eq!(error_text(&presenter), row.error, "{}", row.name);
    }
}

#[test]
fn normal_sdk_scope_delivers_inherited_module_and_request_to_presenter() {
    for format in ["text", "json"] {
        let row = fixture()
            .render_cases
            .into_iter()
            .find(|row| row.name == format!("kind_started_populated_{format}"))
            .unwrap();
        let capture = Capture::default();
        let (clock, _) = counted_clock(&row.clock);
        let presenter = Arc::new(Presenter::with_format(
            Some(Box::new(capture.clone())),
            format,
            Some(clock),
        ));
        let scope = Scope::new(
            Some(presenter.clone()),
            row.event.module.clone(),
            row.event.request_id,
        );
        let mut event = row.event.event();
        event.module.clear();
        event.request_id = 999;
        scope.emit(event);
        assert_eq!(*capture.0.lock().unwrap(), [row.output.bytes()]);
        assert!(presenter.error().is_none());
    }
}

#[test]
fn go_only_nil_and_invalid_string_representations_remain_explicit_witnesses() {
    let fixture = fixture();
    let invalid: Vec<_> = fixture
        .render_cases
        .iter()
        .filter(|row| row.compatibility == "go_only_string_representation")
        .collect();
    assert_eq!(invalid.len(), 2);
    assert_eq!(invalid[0].name, "invalid_go_string_bytes_text");
    assert_eq!(invalid[1].name, "invalid_go_string_bytes_json");
    assert!(!invalid[0].output.output_is_utf8);
    assert!(invalid[1].output.output_is_utf8);
    let nil = &fixture.go_only_cases[0];
    assert_eq!(nil.name, "nil_presenter_receiver");
    assert_eq!(nil.compatibility, "go_only_nil_receiver");
    assert_eq!(
        (&*nil.panic, &*nil.error, nil.clock_calls, nil.writes),
        ("", "", 0, 0)
    );
    let typed = &fixture.go_only_cases[1];
    assert_eq!(typed.name, "typed_nil_writer");
    assert_eq!(typed.compatibility, "go_only_nil_type_or_panic");
    assert_eq!(
        (
            &*typed.panic,
            &*typed.error,
            typed.clock_calls,
            typed.writes
        ),
        ("synthetic typed-nil writer", "", 1, 0)
    );
    assert_eq!(fixture.go_only_cases.len(), 3);
}

#[test]
fn injected_clock_panic_propagates_without_writer_attempt_or_retained_error() {
    let row = fixture()
        .go_only_cases
        .into_iter()
        .find(|row| row.name == "injected_clock_panics")
        .unwrap();
    assert_eq!(row.compatibility, "go_only_nil_type_or_panic");
    let capture = Capture::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let panic_text = row.panic.clone();
    let clock: Clock = Arc::new(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        panic!("{panic_text}")
    });
    let presenter = Presenter::with_clock(Some(Box::new(capture.clone())), Some(clock));
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        presenter.emit(Default::default())
    }))
    .unwrap_err();
    assert_eq!(*panic.downcast::<String>().unwrap(), row.panic);
    assert_eq!(capture.0.lock().unwrap().len(), row.writes);
    assert_eq!(calls.load(Ordering::SeqCst), row.clock_calls);
    assert_eq!(error_text(&presenter), row.error);
}
