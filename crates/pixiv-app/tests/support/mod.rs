use pixiv_app::diagnostics::Event;
use serde_json::{Value, json};

pub fn event_value(event: Event) -> Value {
    json!({
        "Module":event.module,"Kind":event.kind,"Operation":event.operation,
        "Resource":event.resource,"Route":event.route,"Target":event.target,
        "Proxy":event.proxy,"UserAgent":event.user_agent,"Reason":event.reason,
        "Status":event.status,"Count":event.count,"RequestID":event.request_id,
        "Duration":event.duration_ns,
    })
}
