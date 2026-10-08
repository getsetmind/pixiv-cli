mod support;

use pixiv_app::{
    diagnostics::{Event, Scope, Sink},
    lifecycle::Context,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Debug, Deserialize)]
struct Case {
    scope: bool,
    nil_sink: bool,
    child: bool,
    explicit: bool,
    direct: bool,
    canceled: bool,
    events: Value,
}

#[test]
fn diagnostic_scopes_match_go_inheritance_silence_cancellation_and_all_event_fields() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/diagnostics.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 64);
    for case in cases {
        let events: Arc<Mutex<Option<Vec<Value>>>> = Arc::new(Mutex::new(None));
        let output = Arc::clone(&events);
        let sink: Arc<dyn Sink> = Arc::new(move |event| {
            output
                .lock()
                .unwrap()
                .get_or_insert_default()
                .push(support::event_value(event));
        });
        let context = Context::new();
        let context = if case.scope {
            context.with_scope(Scope::new(
                if case.nil_sink { None } else { Some(sink) },
                "Pixiv CLI",
                7,
            ))
        } else {
            context
        };
        let context = if case.child {
            context.with_child_scope("FANBOX FlareSolverr", 8)
        } else {
            context
        };
        if case.canceled {
            context.cancel();
        }
        let event = Event {
            module: if case.explicit {
                "Pixiv account pool".into()
            } else {
                String::new()
            },
            kind: "account".into(),
            operation: "selected".into(),
            resource: "uid 1".into(),
            route: "/safe".into(),
            target: "synthetic.file".into(),
            proxy: "synthetic".into(),
            user_agent: "synthetic-agent".into(),
            reason: "account frozen".into(),
            status: 429,
            count: 3,
            request_id: 999,
            duration_ns: 123456789,
        };
        if case.direct {
            context.scope().cloned().unwrap_or_default().emit(event);
        } else {
            context.emit(event);
        }
        assert_eq!(json!(*events.lock().unwrap()), case.events, "{case:?}");
    }
}
