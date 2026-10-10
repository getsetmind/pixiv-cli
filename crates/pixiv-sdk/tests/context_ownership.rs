use futures_util::FutureExt;
use pixiv_sdk::{
    context::{Context, ContextError, ContextKey, RequestContext},
    diagnostics::{Event, Scope, Sink},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    any::TypeId,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const FIXTURE: &str = include_str!("fixtures/fanbox-context-ownership.json");
const FIXTURE_SHA: &str = "6c0d721eeb35314058c8f928114a9ea2c6bf76e92d468502fc543fd7651cc705";

fn rows() -> Vec<Value> {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["go_version"], "go1.27.1");
    assert_eq!(
        format!("{:x}", Sha256::digest(FIXTURE.as_bytes())),
        FIXTURE_SHA
    );
    let rows = fixture["cases"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 41);
    rows
}

fn row(name: &str) -> Value {
    rows().into_iter().find(|row| row["name"] == name).unwrap()
}

fn value_key() -> ContextKey {
    ContextKey::new("owned-value".to_owned())
}

fn valued_root(value: &str) -> Context {
    Context::background().with_value(value_key(), Arc::new(value.to_owned()))
}

fn error_value(error: Option<ContextError>) -> Value {
    json!({
        "message": error.map(|error| error.to_string()).unwrap_or_default(),
        "reason": "",
        "canceled": error == Some(ContextError::Canceled),
        "deadline_exceeded": error == Some(ContextError::DeadlineExceeded),
    })
}

fn assert_error(actual: Option<ContextError>, expected: &Value) {
    let mut expected = expected.clone();
    expected
        .as_object_mut()
        .unwrap()
        .remove("go_only_error_tree");
    assert_eq!(error_value(actual), expected);
}

fn assert_state(context: &Context, expected: &Value) {
    assert_eq!(expected["nil_context"], false);
    assert_eq!(context.deadline().is_some(), expected["has_deadline"]);
    assert_eq!(context.scope().is_some(), expected["scope_present"]);
    assert_eq!(
        json!(context.value::<String>(&value_key()).as_deref()),
        expected["value"]
    );
    assert_error(context.error(), &expected["error"]);
    assert_error(context.error(), &expected["cause"]);
    assert_eq!(
        context.cancelled().now_or_never().is_some(),
        expected["done_closed"]
    );
    // Rust exposes an awaitable notification; Go's nil channel and concrete
    // interface/timestamps have no source-compatible representation here.
}

fn event(value: &Value) -> Event {
    Event {
        module: value["Module"].as_str().unwrap().to_owned(),
        kind: value["Kind"].as_str().unwrap().to_owned(),
        operation: value["Operation"].as_str().unwrap().to_owned(),
        resource: value["Resource"].as_str().unwrap().to_owned(),
        route: value["Route"].as_str().unwrap().to_owned(),
        target: value["Target"].as_str().unwrap().to_owned(),
        proxy: value["Proxy"].as_str().unwrap().to_owned(),
        user_agent: value["UserAgent"].as_str().unwrap().to_owned(),
        reason: value["Reason"].as_str().unwrap().to_owned(),
        status: value["Status"].as_i64().unwrap(),
        count: value["Count"].as_i64().unwrap(),
        request_id: value["RequestID"].as_u64().unwrap(),
        duration_ns: value["Duration"].as_i64().unwrap(),
    }
}

fn event_value(event: Event) -> Value {
    json!({"Module":event.module,"Kind":event.kind,"Operation":event.operation,
        "Resource":event.resource,"Route":event.route,"Target":event.target,
        "Proxy":event.proxy,"UserAgent":event.user_agent,"Reason":event.reason,
        "Status":event.status,"Count":event.count,"RequestID":event.request_id,
        "Duration":event.duration_ns})
}

#[test]
fn roots_are_uncancelable_and_existing_new_is_a_cancelable_owner() {
    for (name, context) in [
        ("background", Context::background()),
        ("todo", Context::todo()),
    ] {
        let expected = row(&format!("standard/{name}"));
        assert_state(&context, &expected["observation"]["state"]);
        context.cancel();
        assert_state(&context, &expected["observation"]["state"]);
        let child = context.child();
        child.cancel();
        assert_eq!(child.error(), Some(ContextError::Canceled));
        assert_eq!(context.error(), None);
    }
    let owner = Context::new();
    let alias = owner.clone();
    alias.cancel();
    assert_eq!(owner.error(), Some(ContextError::Canceled));
}

#[test]
fn keyed_values_shadow_without_mutating_parent_or_collapsing_key_namespaces() {
    #[derive(Eq, PartialEq)]
    struct OtherKey(String);
    #[derive(Eq, PartialEq)]
    struct StructKey {
        number: u32,
        label: String,
    }
    let expected = row("standard/typed_values_and_shadowing");
    let observation = &expected["observation"];
    let parent = valued_root("owned-parent");
    let sibling_key = ContextKey::new("second-owned-value".to_owned());
    let other_key = ContextKey::new(OtherKey("owned-value".to_owned()));
    let structured = ContextKey::new(StructKey {
        number: 7,
        label: "owned".to_owned(),
    });
    let child = parent
        .with_value(value_key(), Arc::new("owned-child".to_owned()))
        .with_value(sibling_key.clone(), Arc::new(37_u64))
        .with_value(
            other_key.clone(),
            Arc::new("owned-distinct-key-type".to_owned()),
        )
        .with_value(structured, Arc::new("owned-struct-value".to_owned()));
    let nil_shadow = child.without_value(value_key());
    assert_state(&parent, &observation["parent_state"]);
    assert_state(&child, &observation["child_state"]);
    assert_state(&nil_shadow, &observation["nil_shadow_state"]);
    assert_eq!(
        json!(child.value::<String>(&other_key).as_deref()),
        observation["distinct_key_value"]
    );
    let equal_structured = ContextKey::new(StructKey {
        number: 7,
        label: "owned".to_owned(),
    });
    assert_eq!(
        json!(child.value::<String>(&equal_structured).as_deref()),
        observation["equal_struct_key_value"]
    );
    assert_eq!(*child.value::<u64>(&sibling_key).unwrap(), 37);
    assert!(child.value::<String>(&sibling_key).is_none());
    assert!(parent.value::<u64>(&sibling_key).is_none());
    let payload = Arc::new(vec![1_u8, 2, 3]);
    let extended = child.with_extension(payload.clone());
    let caller: Arc<dyn RequestContext> = Arc::new(extended);
    let erased = caller.extension(TypeId::of::<Vec<u8>>()).unwrap();
    assert!(Arc::ptr_eq(
        &erased.downcast::<Vec<u8>>().unwrap(),
        &payload
    ));
    assert_eq!(
        *caller
            .value(&sibling_key)
            .unwrap()
            .downcast::<u64>()
            .unwrap(),
        37
    );
}

#[test]
fn child_and_sibling_cancellation_matches_both_frozen_ownership_orders() {
    for order in ["child_first", "parent_first"] {
        let expected = row(&format!("standard/cancellation/{order}"));
        let observation = &expected["observation"];
        let parent = valued_root("owned-parent").child();
        let child = parent.child();
        let sibling = parent.child();
        for (name, context) in [
            ("parent", &parent),
            ("child", &child),
            ("sibling", &sibling),
        ] {
            assert_state(context, &observation["before"][name]);
        }
        if order == "child_first" {
            child.cancel();
        } else {
            parent.cancel();
        }
        for (name, context) in [
            ("parent", &parent),
            ("child", &child),
            ("sibling", &sibling),
        ] {
            assert_state(context, &observation["after_first"][name]);
        }
        child.cancel();
        parent.cancel();
        parent.cancel();
        child.cancel();
        for (name, context) in [
            ("parent", &parent),
            ("child", &child),
            ("sibling", &sibling),
        ] {
            assert_state(context, &observation["after_both"][name]);
        }
    }
}

#[tokio::test]
async fn inherited_minimum_deadlines_and_first_error_match_frozen_cases() {
    let now = Instant::now();
    let past = now - Duration::from_secs(1);
    let future = now + Duration::from_secs(3_600);
    let later = future + Duration::from_secs(3_600);
    for scenario in [
        "expired_parent_later_child",
        "expired_child_live_parent",
        "earlier_parent_later_child",
        "later_parent_earlier_child",
        "cancel_before_future_deadline",
        "cancel_before_expired_child",
    ] {
        let expected = row(&format!("standard/deadline/{scenario}"));
        let observation = &expected["observation"];
        let cancel_parent = valued_root("owned-parent").child();
        let parent = match scenario {
            "expired_parent_later_child" => cancel_parent.child_with_deadline(past),
            "earlier_parent_later_child" => cancel_parent.child_with_deadline(future),
            "later_parent_earlier_child" => cancel_parent.child_with_deadline(later),
            _ => cancel_parent.clone(),
        };
        if scenario == "cancel_before_expired_child" {
            cancel_parent.cancel();
        }
        let requested = match scenario {
            "expired_parent_later_child"
            | "later_parent_earlier_child"
            | "cancel_before_future_deadline" => future,
            "earlier_parent_later_child" => later,
            _ => past,
        };
        let child = parent.child_with_deadline(requested);
        let effective = match scenario {
            "expired_parent_later_child" => past,
            "earlier_parent_later_child"
            | "later_parent_earlier_child"
            | "cancel_before_future_deadline" => future,
            _ => past,
        };
        assert_eq!(child.deadline(), Some(effective));
        assert_state(&parent, &observation["before_cleanup"]["parent"]);
        assert_state(&child, &observation["before_cleanup"]["child"]);
        child.cancel();
        cancel_parent.cancel();
        parent.cancel();
        assert_state(&parent, &observation["after_cleanup"]["parent"]);
        assert_state(&child, &observation["after_cleanup"]["child"]);
    }
}

#[test]
fn deadline_construction_requires_no_runtime_and_expiry_precedes_later_cancel() {
    let deadline = Instant::now() + Duration::from_millis(5);
    let parent = Context::new();
    let context = parent.child_with_deadline(deadline);
    std::thread::sleep(Duration::from_millis(10));
    parent.cancel();
    assert_eq!(context.deadline(), Some(deadline));
    assert_eq!(context.error(), Some(ContextError::DeadlineExceeded));
    let canceled = Context::new();
    canceled.cancel();
    let expired = canceled.child_with_deadline(Instant::now() - Duration::from_secs(1));
    assert_eq!(expired.error(), Some(ContextError::Canceled));
}

#[tokio::test]
async fn awaited_deadline_notification_finishes_without_polling_error() {
    let expected = row("standard/deadline/real_timer_channel");
    let interval = Duration::from_nanos(expected["input"]["interval_ns"].as_u64().unwrap());
    let origin = Instant::now();
    let deadline = origin + interval;
    let parent = valued_root("owned-parent");
    let timer = parent.child_with_deadline(deadline);
    let child = timer.child();
    let caller: Arc<dyn RequestContext> = Arc::new(child.clone());
    let error = tokio::time::timeout(Duration::from_secs(2), caller.cancelled())
        .await
        .unwrap();
    assert_error(Some(error), &expected["observation"]["error"]);
    assert_eq!(
        timer.deadline().unwrap().duration_since(origin).as_nanos(),
        interval.as_nanos()
    );
    assert_eq!(child.deadline(), Some(deadline));
    assert_eq!(timer.error(), Some(ContextError::DeadlineExceeded));
    assert_eq!(parent.error(), None);
    timer.cancel();
    assert_error(timer.error(), &expected["observation"]["cause"]);
    assert_eq!(
        json!(timer.value::<String>(&value_key()).as_deref()),
        expected["observation"]["value"]
    );
}

#[tokio::test]
async fn parent_notification_reaches_child_and_detach_removes_all_cancel_ownership() {
    let parent = Context::new().with_value(value_key(), Arc::new("owned-parent".to_owned()));
    let child = parent.child();
    let detached = child.without_cancel();
    let independent = detached.child();
    parent.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), child.cancelled())
            .await
            .unwrap(),
        ContextError::Canceled
    );
    detached.cancel();
    assert!(detached.cancelled().now_or_never().is_none());
    assert!(independent.cancelled().now_or_never().is_none());
    independent.cancel();
    assert_eq!(independent.cancelled().await, ContextError::Canceled);
    assert_eq!(detached.error(), None);
    assert_eq!(detached.deadline(), None);
    assert_eq!(
        detached
            .value::<String>(&value_key())
            .as_deref()
            .map(String::as_str),
        Some("owned-parent")
    );
}

#[tokio::test]
async fn diagnostic_scope_events_and_detached_ownership_match_frozen_payloads() {
    for expected in rows()
        .into_iter()
        .filter(|row| row["name"].as_str().unwrap().starts_with("diagnostics/"))
    {
        let input = &expected["input"];
        let observation = &expected["observation"];
        let mode = input["mode"].as_str().unwrap();
        let events = Arc::new(Mutex::new(Vec::<Value>::new()));
        let output = events.clone();
        let sink: Arc<dyn Sink> =
            Arc::new(move |event| output.lock().unwrap().push(event_value(event)));
        let mut context = if mode.starts_with("nil_context") {
            None
        } else {
            Some(valued_root("owned-parent"))
        };
        if !matches!(mode, "absent" | "nil_context_absent" | "zero_scope_direct") {
            // An optional Rust caller is explicitly mapped to Background for
            // WithScope, as Go diagnostics.WithScope does for nil callers.
            context = Some(
                context
                    .unwrap_or_else(Context::background)
                    .with_scope(Scope::new(
                        if matches!(mode, "nil_sink" | "nil_sink_func") {
                            None
                        } else {
                            Some(sink)
                        },
                        input["parent_module"].as_str().unwrap(),
                        input["parent_request_id"].as_u64().unwrap(),
                    )),
            );
        }
        match &context {
            Some(context) => assert_state(context, &observation["before"]),
            None => assert_eq!(observation["before"], json!({"nil_context":true})),
        }
        let child = context.as_ref().map(|context| {
            context.with_child_scope(
                input["child_module"].as_str().unwrap(),
                input["child_request_id"].as_u64().unwrap(),
            )
        });
        let initial = event(&input["event"]);
        let mut explicit = initial.clone();
        explicit.module = "FANBOX network".to_owned();
        if let Some(context) = &context {
            context.emit(initial.clone());
        }
        if let Some(child) = &child {
            child.emit(initial.clone());
            child.emit(explicit.clone());
        }
        let scope = child.as_ref().and_then(Context::scope).cloned();
        assert_eq!(scope.is_some(), observation["scope_lookup_present"]);
        scope.unwrap_or_default().emit(initial.clone());
        if let Some(child) = &child {
            assert_state(child, &observation["child"]);
        }
        if mode.starts_with("parent_child_detached") {
            let deadline = if mode.ends_with("expired") {
                Instant::now() - Duration::from_secs(1)
            } else {
                Instant::now() + Duration::from_secs(3_600)
            };
            let parent = child.as_ref().unwrap().child_with_deadline(deadline);
            let detached = parent.without_cancel();
            let independent = detached.child();
            parent.cancel();
            for (name, context) in [
                ("parent", &parent),
                ("detached", &detached),
                ("independent", &independent),
            ] {
                assert_state(context, &observation["after_parent_cancel"][name]);
            }
            detached.emit(initial.clone());
            independent.emit(explicit);
            independent.cancel();
            for (name, context) in [
                ("parent", &parent),
                ("detached", &detached),
                ("independent", &independent),
            ] {
                assert_state(context, &observation["after_independent_cancel"][name]);
            }
            independent.emit(initial.clone());
        }
        assert_eq!(event_value(initial), observation["input_event_after_emit"]);
        assert_eq!(
            json!(*events.lock().unwrap()),
            observation["events"],
            "{}",
            expected["name"]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn paused_deadline_notification_records_error_and_notifies_children() {
    let context = Context::with_deadline(Instant::now() + Duration::from_secs(30));
    let child = context.child();
    let sibling = context.child();
    let descendant = child.child();
    let waiter = context.clone();
    let notified = tokio::spawn(async move { waiter.cancelled().await });
    tokio::task::yield_now().await;
    assert_eq!(context.error(), None);
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), notified)
            .await
            .unwrap()
            .unwrap(),
        ContextError::DeadlineExceeded,
    );
    for owned in [&context, &child, &sibling, &descendant] {
        assert_eq!(owned.error(), Some(ContextError::DeadlineExceeded));
        assert_eq!(
            owned.cancelled().now_or_never(),
            Some(ContextError::DeadlineExceeded)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn paused_earlier_child_deadline_preserves_later_parent_ownership() {
    let parent = Context::with_deadline(Instant::now() + Duration::from_secs(60));
    let child = parent.child_with_deadline(Instant::now() + Duration::from_secs(30));
    let descendant = child.child();
    let sibling = parent.child();
    let waiter = child.clone();
    let notified = tokio::spawn(async move { waiter.cancelled().await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), notified)
            .await
            .unwrap()
            .unwrap(),
        ContextError::DeadlineExceeded,
    );
    for owned in [&child, &descendant] {
        assert_eq!(owned.error(), Some(ContextError::DeadlineExceeded));
        assert_eq!(
            owned.cancelled().now_or_never(),
            Some(ContextError::DeadlineExceeded)
        );
    }
    for independent in [&parent, &sibling] {
        assert_eq!(independent.error(), None);
        assert!(independent.cancelled().now_or_never().is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn paused_inherited_deadline_notifies_the_owning_parent_and_siblings() {
    let parent = Context::with_deadline(Instant::now() + Duration::from_secs(30));
    let child = parent.child();
    let sibling = parent.child();
    let descendant = child.child();
    let waiter = child.clone();
    let notified = tokio::spawn(async move { waiter.cancelled().await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), notified)
            .await
            .unwrap()
            .unwrap(),
        ContextError::DeadlineExceeded,
    );
    for owned in [&parent, &child, &sibling, &descendant] {
        assert_eq!(owned.error(), Some(ContextError::DeadlineExceeded));
        assert_eq!(
            owned.cancelled().now_or_never(),
            Some(ContextError::DeadlineExceeded)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn paused_deadline_keeps_parent_cancellation_that_precedes_timer_delivery() {
    let parent = Context::new();
    let child = parent.child_with_deadline(Instant::now() + Duration::from_secs(30));
    let descendant = child.child();
    let waiter = child.clone();
    let notified = tokio::spawn(async move { waiter.cancelled().await });
    tokio::task::yield_now().await;
    parent.cancel();
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), notified)
            .await
            .unwrap()
            .unwrap(),
        ContextError::Canceled,
    );
    for owned in [&parent, &child, &descendant] {
        assert_eq!(owned.error(), Some(ContextError::Canceled));
        assert_eq!(
            owned.cancelled().now_or_never(),
            Some(ContextError::Canceled)
        );
    }
    let already_expired_child = parent.child_with_deadline(Instant::now() - Duration::from_secs(1));
    assert_eq!(already_expired_child.error(), Some(ContextError::Canceled));
    assert_eq!(
        already_expired_child.cancelled().await,
        ContextError::Canceled
    );
}
