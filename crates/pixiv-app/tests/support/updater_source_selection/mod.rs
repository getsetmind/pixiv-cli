use pixiv_app::{
    reverse_search::{
        ReverseFuture,
        http::{HttpRequest, HttpTransport},
    },
    update::{
        CallerContext, ExternalError,
        source::{ReleaseSource, ReleaseSourceKind, ReleaseSourceSelector, parse_release_sources},
    },
};
use pixiv_sdk::{
    context::{Context, ContextKey},
    fanbox::transport::{BodyFuture, Headers, RawBody, RawRead, RawResponse},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Notify, mpsc};

pub fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../pixiv-cli/tests/fixtures/updater-source-selection.json"
    ))
    .unwrap()
}
pub fn rows(fixture: &Value) -> impl Iterator<Item = &Value> {
    fixture["cases"].as_array().unwrap().iter()
}
pub fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap()
}
pub fn go_only(row: &Value) -> bool {
    row["go_only"].as_bool().unwrap_or(false)
}
pub fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect()
}
pub fn source_ids(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| text(v, "id").into())
        .collect()
}
pub fn ids(sources: &[ReleaseSource]) -> Vec<String> {
    sources.iter().map(|s| s.id().into()).collect()
}
pub fn sources_from_descriptions(value: &Value) -> Vec<ReleaseSource> {
    let body = value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            format!(
                "{}|{}|{}",
                text(v, "id"),
                if text(v, "api").is_empty() {
                    "-"
                } else {
                    text(v, "api")
                },
                text(v, "asset")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if body.is_empty() {
        vec![]
    } else {
        parse_release_sources(body.as_bytes()).unwrap()
    }
}
pub fn kind(value: &str) -> ReleaseSourceKind {
    match value {
        "GitHub Releases API" => ReleaseSourceKind::API,
        "release asset" => ReleaseSourceKind::Asset,
        _ => ReleaseSourceKind::Unknown(value.into()),
    }
}
pub fn assert_message<T>(result: &Result<T, ExternalError>, expected: &Value, row: &Value) {
    assert_eq!(
        result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
        text(expected, "message"),
        "{} / {}",
        row["operation"],
        row["name"]
    );
    let mut cause = result
        .as_ref()
        .err()
        .map(|error| error.as_ref() as &(dyn std::error::Error + 'static));
    let mut canceled = false;
    let mut deadline = false;
    while let Some(error) = cause {
        canceled |= error.downcast_ref::<pixiv_sdk::context::ContextError>()
            == Some(&pixiv_sdk::context::ContextError::Canceled);
        deadline |= error.downcast_ref::<pixiv_sdk::context::ContextError>()
            == Some(&pixiv_sdk::context::ContextError::DeadlineExceeded);
        cause = error.source();
    }
    assert_eq!(
        canceled,
        expected["canceled"].as_bool().unwrap(),
        "{} canceled cause",
        row["name"]
    );
    assert_eq!(
        deadline,
        expected["deadline"].as_bool().unwrap(),
        "{} deadline cause",
        row["name"]
    );
}
pub fn assert_probe_message<T>(result: &Result<T, ExternalError>, expected: &Value, row: &Value) {
    if text(row, "name") == "api-nil-response" {
        let concrete_go_type = "(source.updaterSourceTransport)";
        let expected_message = text(expected, "message");
        assert_eq!(expected_message.matches(concrete_go_type).count(), 1);
        let mut mapped = expected.clone();
        mapped["message"] =
            json!(expected_message.replacen(concrete_go_type, "(HttpTransport)", 1));
        assert_message(result, &mapped, row);
    } else {
        assert_message(result, expected, row);
    }
}
pub fn context(mode: &str) -> (CallerContext, Context) {
    let caller = match mode {
        "deadline" => Context::with_deadline(Instant::now() - Duration::from_secs(1)),
        "future-deadline" => Context::with_deadline(Instant::now() + Duration::from_secs(3600)),
        _ => Context::new(),
    }
    .with_value(
        ContextKey::new("source-test"),
        Arc::new(String::from("owned-context")),
    );
    if mode == "canceled" {
        caller.cancel();
    }
    (Arc::new(caller.clone()), caller)
}
fn request_value(request: &HttpRequest, parent: &CallerContext) -> Value {
    json!({"method":request.method,"url":request.url,"header":request.headers,"nil_body":request.body.is_none(),"nil_get_body":true,"content_length":request.content_length,"context_value":request.context.value(&ContextKey::new("source-test")).and_then(|v|v.downcast::<String>().ok()).as_deref(),"context_same_as_parent":Arc::ptr_eq(&request.context,parent),"context_deadline":request.context.deadline().is_some(),"context_canceled":request.context.error().is_some()})
}
fn failure(value: &str) -> ExternalError {
    Box::new(io::Error::other(value))
}
fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap())
        .collect()
}
#[derive(Clone)]
struct BodyPlan {
    bytes: Vec<u8>,
    mode: String,
    trace: Arc<Mutex<Vec<String>>>,
    cancel: Context,
    error: String,
    closed: Option<Arc<Notify>>,
}
struct OwnedBody {
    plan: BodyPlan,
    offset: usize,
}
impl RawBody for OwnedBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            if self.offset < self.plan.bytes.len() {
                let count = output.len().min(self.plan.bytes.len() - self.offset);
                output[..count].copy_from_slice(&self.plan.bytes[self.offset..self.offset + count]);
                self.offset += count;
                self.plan
                    .trace
                    .lock()
                    .unwrap()
                    .push(format!("read:{count}"));
                if self.plan.mode == "cancel-final-eof" || self.plan.mode == "cancel-after-bytes" {
                    self.plan.cancel.cancel();
                }
                return RawRead {
                    count,
                    eof: self.plan.mode == "cancel-final-eof",
                    error: (self.plan.mode == "error-with-bytes")
                        .then(|| failure(&self.plan.error)),
                };
            }
            if self.plan.mode == "read-error" || self.plan.mode == "error-with-bytes" {
                self.plan.trace.lock().unwrap().push("read:error".into());
                RawRead {
                    count: 0,
                    eof: false,
                    error: Some(failure(&self.plan.error)),
                }
            } else {
                self.plan.trace.lock().unwrap().push("read:eof".into());
                RawRead {
                    count: 0,
                    eof: true,
                    error: None,
                }
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            self.plan.trace.lock().unwrap().push("close".into());
            if let Some(closed) = &self.plan.closed {
                closed.notify_one();
            }
            if self.plan.mode == "close-error" {
                Err(failure("owned body close error"))
            } else {
                Ok(())
            }
        })
    }
}
fn response(status: u16, plan: BodyPlan) -> RawResponse {
    RawResponse {
        status,
        headers: Headers::new(),
        content_length: -1,
        body: Some(Box::new(OwnedBody { plan, offset: 0 })),
    }
}
pub struct SingleTransport {
    input: Value,
    parent: CallerContext,
    plan: BodyPlan,
    requests: Mutex<Vec<Value>>,
}
impl SingleTransport {
    pub fn from_input(input: &Value, parent: CallerContext, caller: Context) -> Self {
        Self {
            input: input.clone(),
            parent,
            plan: BodyPlan {
                bytes: unhex(text(input, "body_hex")),
                mode: text(input, "body_mode").into(),
                trace: Arc::new(Mutex::new(vec![])),
                cancel: caller,
                error: "owned body read error".into(),
                closed: None,
            },
            requests: Mutex::new(vec![]),
        }
    }
    pub fn trace(&self) -> Vec<String> {
        self.plan.trace.lock().unwrap().clone()
    }
    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}
impl HttpTransport for SingleTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            self.requests
                .lock()
                .unwrap()
                .push(request_value(&request, &self.parent));
            match text(&self.input, "transport") {
                "error" => return Err(failure("owned transport error")),
                "nil-response" => return Ok(None),
                _ => {}
            }
            let mut result = response(
                self.input["status"].as_u64().unwrap() as u16,
                self.plan.clone(),
            );
            if text(&self.input, "transport") == "nil-body" {
                result.body = None;
                result.content_length = 0;
            }
            Ok(Some(result))
        })
    }
}
pub struct RedirectTransport {
    input: Value,
    parent: CallerContext,
    caller: Context,
    requests: Mutex<Vec<Value>>,
    traces: Mutex<Vec<Arc<Mutex<Vec<String>>>>>,
}
impl RedirectTransport {
    pub fn new(input: Value, parent: CallerContext, caller: Context) -> Self {
        Self {
            input,
            parent,
            caller,
            requests: Mutex::new(vec![]),
            traces: Mutex::new(vec![]),
        }
    }
    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
    pub fn traces(&self) -> Vec<Vec<String>> {
        self.traces
            .lock()
            .unwrap()
            .iter()
            .map(|trace| trace.lock().unwrap().clone())
            .collect()
    }
}
impl HttpTransport for RedirectTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let index = {
                let mut requests = self.requests.lock().unwrap();
                let index = requests.len();
                requests.push(request_value(&request, &self.parent));
                index
            };
            let redirect = index < self.input["redirects"].as_u64().unwrap() as usize;
            let trace = Arc::new(Mutex::new(vec![]));
            self.traces.lock().unwrap().push(trace.clone());
            let plan = BodyPlan {
                bytes: if redirect {
                    b"owned redirect body".to_vec()
                } else {
                    text(&self.input, "final_body").as_bytes().to_vec()
                },
                mode: if redirect && self.input["redirect_close_error"].as_bool().unwrap() {
                    "close-error"
                } else {
                    ""
                }
                .into(),
                trace,
                cancel: self.caller.clone(),
                error: "owned body read error".into(),
                closed: None,
            };
            let mut result = response(if redirect { 302 } else { 200 }, plan);
            if redirect {
                result.headers.insert(
                    "Location".into(),
                    vec![format!("https://landing.test/owned/{}", index + 1)],
                );
            }
            Ok(Some(result))
        })
    }
}
struct ConcurrentTransport {
    parent: CallerContext,
    caller: Context,
    operation: String,
    winner: String,
    kind: ReleaseSourceKind,
    gates: BTreeMap<String, Arc<Notify>>,
    closed: BTreeMap<String, Arc<Notify>>,
    entered: mpsc::UnboundedSender<String>,
    requests: Mutex<BTreeMap<String, Value>>,
    events: Arc<Mutex<BTreeMap<String, Vec<String>>>>,
    traces: Mutex<BTreeMap<String, Arc<Mutex<Vec<String>>>>>,
    child_contexts: Mutex<Vec<CallerContext>>,
}
struct SendOwnership {
    context: CallerContext,
    id: String,
    events: Arc<Mutex<BTreeMap<String, Vec<String>>>>,
}
impl Drop for SendOwnership {
    fn drop(&mut self) {
        if self.context.error().is_some() {
            self.events
                .lock()
                .unwrap()
                .entry(self.id.clone())
                .or_default()
                .push("child-canceled".into());
        }
    }
}
impl HttpTransport for ConcurrentTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let id = request
                .url
                .strip_prefix("https://")
                .unwrap()
                .split('.')
                .next()
                .unwrap()
                .into();
            let ownership = SendOwnership {
                context: request.context.clone(),
                id,
                events: self.events.clone(),
            };
            let id = ownership.id.clone();
            self.requests
                .lock()
                .unwrap()
                .insert(id.clone(), request_value(&request, &self.parent));
            self.child_contexts
                .lock()
                .unwrap()
                .push(request.context.clone());
            self.events
                .lock()
                .unwrap()
                .entry(id.clone())
                .or_default()
                .push("entered".into());
            self.entered.send(id.clone()).unwrap();
            if self.operation == "ordered_race" && id != self.winner {
                let error = request.context.cancelled().await;
                return Err(Box::new(error) as ExternalError);
            }
            self.gates[&id].notified().await;
            self.events
                .lock()
                .unwrap()
                .entry(id.clone())
                .or_default()
                .push("released".into());
            let trace = Arc::new(Mutex::new(vec![]));
            self.traces
                .lock()
                .unwrap()
                .insert(id.clone(), trace.clone());
            let plan = BodyPlan {
                bytes: if self.operation == "ordered_all_failure" {
                    vec![]
                } else if self.kind == ReleaseSourceKind::API {
                    b"[]".to_vec()
                } else {
                    b"owned winning asset!".to_vec()
                },
                mode: if self.operation == "ordered_all_failure" {
                    "read-error"
                } else {
                    ""
                }
                .into(),
                trace,
                cancel: self.caller.clone(),
                error: format!("owned failure {id}"),
                closed: Some(self.closed[&id].clone()),
            };
            Ok(Some(response(200, plan)))
        })
    }
}
pub async fn run_concurrent_row(row: &Value) {
    let input = &row["input"];
    let output = &row["output"];
    let (context, caller) = context("");
    let sources = sources_from_descriptions(&input["sources"]);
    let kind = kind(text(input, "kind"));
    let eligible = if kind == ReleaseSourceKind::API {
        sources
            .iter()
            .filter(|source| source.api_url(text(input, "canonical")).is_ok())
            .map(|s| s.id().to_owned())
            .collect::<Vec<_>>()
    } else {
        ids(&sources)
    };
    let (entered_tx, mut entered_rx) = mpsc::unbounded_channel();
    let transport = Arc::new(ConcurrentTransport {
        parent: context.clone(),
        caller: caller.clone(),
        operation: text(row, "operation").into(),
        winner: input["winner"].as_str().unwrap_or("").into(),
        kind: kind.clone(),
        gates: eligible
            .iter()
            .map(|id| (id.clone(), Arc::new(Notify::new())))
            .collect(),
        closed: eligible
            .iter()
            .map(|id| (id.clone(), Arc::new(Notify::new())))
            .collect(),
        entered: entered_tx,
        requests: Mutex::new(BTreeMap::new()),
        events: Arc::new(Mutex::new(BTreeMap::new())),
        traces: Mutex::new(BTreeMap::new()),
        child_contexts: Mutex::new(vec![]),
    });
    let selector = ReleaseSourceSelector::new(sources, transport.clone());
    let canonical = text(input, "canonical").to_owned();
    let task = tokio::spawn(async move { selector.ordered(context, kind, &canonical).await });
    for _ in &eligible {
        entered_rx.recv().await.unwrap();
    }
    match text(row, "operation") {
        "ordered_race" => transport.gates[text(input, "winner")].notify_one(),
        "ordered_all_failure" => {
            for id in strings(&input["completion_order"]) {
                transport.gates[&id].notify_one();
                transport.closed[&id].notified().await;
            }
        }
        "ordered_parent_cancel" => caller.cancel(),
        _ => unreachable!(),
    }
    let result = task.await.unwrap();
    assert_message(&result, &output["error"], row);
    if text(row, "operation") != "ordered_parent_cancel" {
        assert_eq!(
            result.map(|sources| ids(&sources)).unwrap_or_default(),
            strings(&output["ids"])
        );
    }
    let requests = transport.requests.lock().unwrap().clone();
    if text(row, "operation") == "ordered_race" {
        let expected = output["requests_by_source"].as_object().unwrap();
        for id in &eligible {
            assert_eq!(
                requests[id], expected[id]["request"],
                "{} {id} request",
                row["name"]
            );
            if id != text(input, "winner") {
                assert!(transport.events.lock().unwrap()[id].contains(&"child-canceled".into()));
            } else {
                assert_eq!(
                    json!(*transport.traces.lock().unwrap()[id].lock().unwrap()),
                    expected[id]["body_trace"]
                );
            }
        }
    } else {
        assert_eq!(json!(requests), output["requests_by_source"]);
        if text(row, "operation") == "ordered_all_failure" {
            let traces: BTreeMap<_, _> = transport
                .traces
                .lock()
                .unwrap()
                .iter()
                .map(|(id, trace)| (id.clone(), trace.lock().unwrap().clone()))
                .collect();
            assert_eq!(json!(traces), output["body_trace_by_source"]);
        }
    }
    assert!(
        transport
            .child_contexts
            .lock()
            .unwrap()
            .iter()
            .all(|ctx| ctx.error().is_some())
    );
    assert_eq!(
        caller.error().is_some(),
        text(row, "operation") == "ordered_parent_cancel"
    );
}

struct PendingOwner {
    dropped: Arc<std::sync::atomic::AtomicUsize>,
}
impl Drop for PendingOwner {
    fn drop(&mut self) {
        self.dropped
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
struct PendingBody {
    _owner: PendingOwner,
    entered: Arc<Notify>,
    closed: Arc<std::sync::atomic::AtomicUsize>,
}
impl RawBody for PendingBody {
    fn read<'a>(&'a mut self, _output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            self.entered.notify_one();
            std::future::pending().await
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        self.closed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
}
struct OwnershipTransport {
    losing_body: bool,
    entered: Arc<Notify>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
    closed: Arc<std::sync::atomic::AtomicUsize>,
    child: Mutex<Option<CallerContext>>,
    caller: Context,
}
impl HttpTransport for OwnershipTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            if request.url.starts_with("https://loser.test/") {
                *self.child.lock().unwrap() = Some(request.context);
                let owner = PendingOwner {
                    dropped: self.dropped.clone(),
                };
                if self.losing_body {
                    return Ok(Some(RawResponse {
                        status: 200,
                        headers: Headers::new(),
                        content_length: -1,
                        body: Some(Box::new(PendingBody {
                            _owner: owner,
                            entered: self.entered.clone(),
                            closed: self.closed.clone(),
                        })),
                    }));
                }
                self.entered.notify_one();
                let result = std::future::pending().await;
                drop(owner);
                return result;
            }
            self.entered.notified().await;
            Ok(Some(response(
                200,
                BodyPlan {
                    bytes: b"[]".to_vec(),
                    mode: String::new(),
                    trace: Arc::new(Mutex::new(vec![])),
                    cancel: self.caller.clone(),
                    error: String::new(),
                    closed: None,
                },
            )))
        })
    }
}
pub async fn ignoring_loser_is_dropped_without_delaying_the_winner(losing_body: bool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (context, caller) = context("");
    let transport = Arc::new(OwnershipTransport {
        losing_body,
        entered: Arc::new(Notify::new()),
        dropped: Arc::new(AtomicUsize::new(0)),
        closed: Arc::new(AtomicUsize::new(0)),
        child: Mutex::new(None),
        caller: caller.clone(),
    });
    let sources = parse_release_sources(b"loser|https://loser.test/{url}|https://loser.test/{url}\nwinner|https://winner.test/{url}|https://winner.test/{url}").unwrap();
    let selector = ReleaseSourceSelector::new(sources, transport.clone());
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        selector.ordered(
            context,
            ReleaseSourceKind::API,
            "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases",
        ),
    )
    .await
    .expect("context-ignoring loser must not stall Ordered")
    .unwrap();
    assert_eq!(ids(&result), ["winner", "loser"]);
    assert_eq!(transport.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        transport.closed.load(Ordering::SeqCst),
        usize::from(losing_body)
    );
    assert!(
        transport
            .child
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .error()
            .is_some()
    );
    assert!(caller.error().is_none());
}
