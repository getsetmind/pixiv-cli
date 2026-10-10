use pixiv_sdk::{
    context::{Context, ContextError, ContextFuture, ContextKey, ContextValue, RequestContext},
    diagnostics::{Event, Scope},
    error::{Cause, Error, is_canceled, is_deadline_exceeded},
    fanbox::{
        User,
        transport::{
            BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
            TransportFuture,
        },
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    any::TypeId,
    collections::BTreeMap,
    fmt, io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, mpsc},
};
pub const SESSION: &str = "synthetic-contract-session";
pub fn body_bytes(step: &Value) -> Vec<u8> {
    let bytes = match step["body_hex"].as_str() {
        Some(hex) => {
            assert_eq!(hex.len() % 2, 0, "raw body hex length");
            assert!(
                hex.bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "raw body hex must be lowercase"
            );
            hex.as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect()
        }
        None => step["body"].as_str().unwrap_or("").as_bytes().to_vec(),
    };
    if let Some(length) = step["body_length"].as_u64() {
        assert_eq!(bytes.len() as u64, length, "raw body length");
    }
    if let Some(sha256) = step["body_sha256"].as_str() {
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            sha256,
            "raw body SHA256"
        );
    }
    bytes
}
pub fn event(event: Event) -> Value {
    json!({"Module":event.module,"Kind":event.kind,"Operation":event.operation,"Resource":event.resource,"Route":event.route,"Target":event.target,"Proxy":event.proxy,"UserAgent":event.user_agent,"Reason":event.reason,"Status":event.status,"Count":event.count,"RequestID":event.request_id,"Duration":event.duration_ns})
}
pub fn error(error: Option<&Error>) -> Value {
    let mut value = match error {
        None => {
            json!({"message":"","reason":"","code":"","source_message":"","canceled":false,"deadline":false})
        }
        Some(error) => {
            json!({"message":error.to_string(),"reason":error.code.as_str(),"code":error.code.as_str(),"source_message":std::error::Error::source(error).map(ToString::to_string).unwrap_or_default(),"canceled":is_canceled(error),"deadline":is_deadline_exceeded(error)})
        }
    };
    if let Some(error) = error {
        value["sdk"] = json!({"product":error.product,"operation":error.operation,"reason":error.code.as_str(),"detail":error.detail.clone().unwrap_or_default(),"http_status":error.http_status.unwrap_or(0),"transport":error.transport.map(|kind|format!("{kind:?}").to_lowercase()).unwrap_or_default(),"retry_safe":error.retry.safe,"retry_has_after":error.retry.after.is_some()});
    }
    value
}
pub fn expected_error(value: &Value) -> Value {
    let mut result = json!({"message":value["message"],"reason":value["reason"],"code":value["code"],"source_message":value["source_message"],"canceled":value["canceled"],"deadline":value["deadline"]});
    if let Some(sdk) = value.get("sdk") {
        let mut sdk = sdk.clone();
        sdk.as_object_mut()
            .unwrap()
            .remove("go_only_matches_reason");
        result["sdk"] = sdk;
    }
    result
}
pub fn outcome(result: &pixiv_sdk::Result<User>) -> Value {
    json!({"dto":result.as_ref().map(|user|user.to_dto()).unwrap_or_default(),"error":error(result.as_ref().err())})
}
pub fn expected_outcome(value: &Value) -> Value {
    json!({"dto":value["dto"],"error":expected_error(&value["error"])})
}
pub fn scoped(label: &str, id: u64, deadline: bool, events: Arc<Mutex<Vec<Value>>>) -> Context {
    let context = if deadline {
        Context::with_deadline(Instant::now() + std::time::Duration::from_secs(3600))
    } else {
        Context::new()
    };
    context
        .with_value(ContextKey::new("solver-caller"), Arc::new(label.to_owned()))
        .with_scope(Scope::new(
            Some(Arc::new(move |value: Event| {
                events.lock().unwrap().push(event(value))
            })),
            "FANBOX CLI",
            id,
        ))
}
#[derive(Debug)]
pub struct Probe {
    pub context: Context,
    pub armed: Arc<AtomicBool>,
    pub registered: Notify,
}
impl Probe {
    pub fn new(context: Context) -> Self {
        Self {
            context,
            armed: Arc::new(AtomicBool::new(false)),
            registered: Notify::new(),
        }
    }
}
impl RequestContext for Probe {
    fn error(&self) -> Option<ContextError> {
        self.context.error()
    }
    fn deadline(&self) -> Option<Instant> {
        self.context.deadline()
    }
    fn cancelled(&self) -> ContextFuture<'_> {
        if self.armed.load(Ordering::SeqCst) {
            self.registered.notify_one();
        }
        Box::pin(self.context.cancelled())
    }
    fn value(&self, key: &ContextKey) -> Option<ContextValue> {
        RequestContext::value(&self.context, key)
    }
    fn extension(&self, key: TypeId) -> Option<ContextValue> {
        RequestContext::extension(&self.context, key)
    }
    fn scope(&self) -> Option<&Scope> {
        self.context.scope()
    }
}
#[derive(Default)]
struct Record {
    bytes: usize,
    closes: usize,
}
struct Body {
    step: Value,
    bytes: Vec<u8>,
    offset: usize,
    record: Arc<Mutex<Record>>,
    armed: Option<Arc<AtomicBool>>,
}
fn external(kind: &str) -> Option<ExternalError> {
    (!kind.is_empty()).then(|| {
        Box::new(match kind {
            "canceled" => Cause::Canceled,
            "deadline" => Cause::DeadlineExceeded,
            _ => Cause::Redacted(
                "synthetic-external-canary https://example.invalid/private?token=synthetic-secret"
                    .into(),
            ),
        }) as ExternalError
    })
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let bytes = &self.bytes;
            let limit = self.step["chunk"]
                .as_u64()
                .filter(|count| *count > 0)
                .map(|count| count as usize)
                .unwrap_or(output.len());
            let count = (bytes.len() - self.offset).min(output.len()).min(limit);
            output[..count].copy_from_slice(&bytes[self.offset..self.offset + count]);
            self.offset += count;
            self.record.lock().unwrap().bytes += count;
            let kind = if self.offset == bytes.len() {
                self.step["read_error"].as_str().unwrap_or("")
            } else {
                ""
            };
            RawRead {
                count,
                eof: count == 0 && kind.is_empty(),
                error: external(kind),
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, std::result::Result<(), ExternalError>> {
        Box::pin(async move {
            self.record.lock().unwrap().closes += 1;
            if self.step["status"] == 403
                && let Some(armed) = &self.armed
            {
                armed.store(true, Ordering::SeqCst);
            }
            match external(self.step["close_error"].as_str().unwrap_or("")) {
                Some(error) => Err(error),
                None => Ok(()),
            }
        })
    }
}
pub struct Native {
    steps: Vec<Value>,
    concurrent: bool,
    seen: Mutex<BTreeMap<String, usize>>,
    pub requests: Mutex<Vec<Value>>,
    pub contexts: Mutex<Vec<Arc<dyn RequestContext>>>,
    records: Mutex<Vec<Arc<Mutex<Record>>>>,
    probes: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
    idle: AtomicUsize,
}
impl fmt::Debug for Native {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Owned native fixture")
    }
}
impl Native {
    pub fn new(input: &Value) -> Self {
        Self {
            steps: input["native_steps"].as_array().unwrap().clone(),
            concurrent: input["mode"] != "",
            seen: Mutex::new(BTreeMap::new()),
            requests: Mutex::new(vec![]),
            contexts: Mutex::new(vec![]),
            records: Mutex::new(vec![]),
            probes: Mutex::new(BTreeMap::new()),
            idle: AtomicUsize::new(0),
        }
    }
    pub fn probe(&self, label: &str, probe: &Probe) {
        self.probes
            .lock()
            .unwrap()
            .insert(label.into(), probe.armed.clone());
    }
    pub fn view(&self) -> Value {
        let requests = self.requests.lock().unwrap().clone();
        let bodies: Vec<_> = self
            .records
            .lock()
            .unwrap()
            .iter()
            .map(|record| {
                let record = record.lock().unwrap();
                json!({"bytes_read":record.bytes,"close_calls":record.closes})
            })
            .collect();
        if self.concurrent {
            let mut groups = BTreeMap::<String, Vec<Value>>::new();
            for request in requests {
                groups
                    .entry(request["caller"].as_str().unwrap().into())
                    .or_default()
                    .push(request);
            }
            let bytes: usize = bodies
                .iter()
                .map(|body| body["bytes_read"].as_u64().unwrap() as usize)
                .sum();
            let closes: usize = bodies
                .iter()
                .map(|body| body["close_calls"].as_u64().unwrap() as usize)
                .sum();
            json!({"requests_by_caller":groups,"body_totals":{"bytes_read":bytes,"close_calls":closes},"idle_calls":self.idle.load(Ordering::SeqCst)})
        } else {
            json!({"requests":requests,"bodies":bodies,"idle_calls":self.idle.load(Ordering::SeqCst)})
        }
    }
}
impl RawTransport for Native {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, std::result::Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let label = request
                .context
                .value(&ContextKey::new("solver-caller"))
                .unwrap()
                .downcast::<String>()
                .unwrap()
                .as_ref()
                .clone();
            let mut headers = request.headers;
            for (key, values) in &mut headers {
                if key.eq_ignore_ascii_case("Cookie") {
                    for value in values {
                        *value = value.replace(SESSION, "[SYNTHETIC_SESSION]");
                    }
                }
            }
            let index = {
                let mut requests = self.requests.lock().unwrap();
                let index = requests.len();
                requests.push(json!({"caller":label,"method":request.method,"url":request.url,"headers":headers,"context_canceled":request.context.error().is_some(),"context_has_deadline":request.context.deadline().is_some(),"request_has_body":request.body.is_some()}));
                index
            };
            self.contexts.lock().unwrap().push(request.context);
            let index = if self.concurrent {
                let mut seen = self.seen.lock().unwrap();
                let count = seen.entry(label.clone()).or_default();
                let index = usize::from(*count > 0);
                *count += 1;
                index
            } else {
                index
            };
            let step = self
                .steps
                .get(index)
                .unwrap_or_else(|| panic!("unexpected native request {index}"))
                .clone();
            let record = Arc::new(Mutex::new(Record::default()));
            self.records.lock().unwrap().push(record.clone());
            let mut headers = step
                .get("headers")
                .map(|value| serde_json::from_value(value.clone()).unwrap())
                .unwrap_or_default();
            if let Some(location) = step["location"].as_str() {
                std::collections::BTreeMap::<String, Vec<String>>::insert(
                    &mut headers,
                    "Location".into(),
                    vec![location.into()],
                );
            }
            Ok(Some(RawResponse {
                status: step["status"].as_u64().unwrap() as u16,
                headers,
                content_length: step["content_length"].as_i64().unwrap_or(0),
                body: Some(Box::new(Body {
                    bytes: body_bytes(&step),
                    step,
                    offset: 0,
                    record,
                    armed: self.probes.lock().unwrap().get(&label).cloned(),
                })),
            }))
        })
    }
    fn close_idle_connections(&self) {
        self.idle.fetch_add(1, Ordering::SeqCst);
    }
}
pub fn expected_native(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(bodies) = value.get_mut("bodies").and_then(Value::as_array_mut) {
        for body in bodies {
            body.as_object_mut().unwrap().remove("go_only_read_calls");
        }
    }
    value
}
pub fn expected_control(value: &Value) -> Value {
    Value::Array(
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|request| {
                let mut request = request.clone();
                request
                    .as_object_mut()
                    .unwrap()
                    .retain(|key, _| !key.starts_with("go_only_"));
                request
            })
            .collect(),
    )
}
pub struct Control {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    pub entered: mpsc::UnboundedReceiver<usize>,
    pub canceled: mpsc::UnboundedReceiver<usize>,
    pub finished: mpsc::UnboundedReceiver<usize>,
    pub releases: Vec<Arc<Notify>>,
    listener: tokio::task::JoinHandle<()>,
}
impl Control {
    pub async fn new(input: &Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let (entered_tx, entered) = mpsc::unbounded_channel();
        let (canceled_tx, canceled) = mpsc::unbounded_channel();
        let (finished_tx, finished) = mpsc::unbounded_channel();
        let steps = match input.get("control_response_hex") {
            Some(hex) => vec![json!({"status":200,"body_hex":hex})],
            None => input["control_steps"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        };
        let blocked = input["mode"] != "";
        let releases = (0..steps.len())
            .map(|_| Arc::new(Notify::new()))
            .collect::<Vec<_>>();
        let cloned = requests.clone();
        let gates = releases.clone();
        let listener = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let requests = cloned.clone();
                let steps = steps.clone();
                let gates = gates.clone();
                let entered = entered_tx.clone();
                let canceled = canceled_tx.clone();
                let finished = finished_tx.clone();
                tokio::spawn(async move {
                    serve(
                        socket,
                        requests,
                        steps,
                        gates,
                        blocked,
                        Signals {
                            entered,
                            canceled,
                            finished,
                        },
                    )
                    .await;
                });
            }
        });
        Self {
            url,
            requests,
            entered,
            canceled,
            finished,
            releases,
            listener,
        }
    }
    pub async fn stop(&mut self) {
        self.listener.abort();
        let _ = (&mut self.listener).await;
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.listener.abort();
        for release in &self.releases {
            release.notify_one();
        }
    }
}
struct Signals {
    entered: mpsc::UnboundedSender<usize>,
    canceled: mpsc::UnboundedSender<usize>,
    finished: mpsc::UnboundedSender<usize>,
}
async fn serve(
    mut socket: TcpStream,
    requests: Arc<Mutex<Vec<Value>>>,
    steps: Vec<Value>,
    gates: Vec<Arc<Notify>>,
    blocked: bool,
    signals: Signals,
) {
    let Ok((request, _)) = control_request(&mut socket).await else {
        return;
    };
    let index = {
        let mut requests = requests.lock().unwrap();
        let index = requests.len();
        requests.push(request);
        index
    };
    let step = steps
        .get(index)
        .unwrap_or_else(|| panic!("unexpected control request {index}"));
    if blocked {
        signals.entered.send(index).unwrap();
        let mut byte = [0];
        tokio::select! {_=gates[index].notified()=>{},_=socket.read(&mut byte)=>{signals.canceled.send(index).unwrap();gates[index].notified().await;}}
    }
    let body = body_bytes(step);
    let status = step["status"].as_u64().unwrap();
    let mut response = format!(
        "HTTP/1.1 {status} Owned\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(location) = step["location"].as_str() {
        response.push_str(&format!("Location: {location}\r\n"));
    }
    response.push_str("\r\n");
    let mut response = response.into_bytes();
    response.extend_from_slice(&body);
    let _ = socket.write_all(&response).await;
    let _ = socket.shutdown().await;
    if blocked {
        let _ = signals.finished.send(index);
    }
}
async fn control_request(socket: &mut TcpStream) -> io::Result<(Value, String)> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0];
        socket.read_exact(&mut byte).await?;
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid owned control request");
    let text = String::from_utf8(bytes).map_err(|_| invalid())?;
    let mut lines = text.split("\r\n");
    let first = lines
        .next()
        .ok_or_else(invalid)?
        .split_whitespace()
        .collect::<Vec<_>>();
    if first.len() != 3 {
        return Err(invalid());
    }
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let length = headers
        .get("content-length")
        .ok_or_else(invalid)?
        .parse::<usize>()
        .map_err(|_| invalid())?;
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await?;
    let body = String::from_utf8(body).map_err(|_| invalid())?;
    let get = |name: &str| headers.get(name).cloned().unwrap_or_default();
    Ok((
        json!({"method":first[0],"path":first[1],"body":body,"content_length":length,"accept":get("accept"),"content_type":get("content-type"),"cookie":get("cookie"),"origin":get("origin"),"referer":get("referer")}),
        first[2].to_owned(),
    ))
}
pub struct StreamControl {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    pub flushed: mpsc::UnboundedReceiver<io::Result<usize>>,
    release: Arc<Notify>,
    released: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl StreamControl {
    pub async fn new(prefix: Vec<u8>, remainder: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let release = Arc::new(Notify::new());
        let released = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let (flushed_tx, flushed) = mpsc::unbounded_channel();
        let recorded = requests.clone();
        let gate = release.clone();
        let done = finished.clone();
        let task = tokio::spawn(async move {
            let setup = async {
                let (mut socket, _) = listener.accept().await?;
                let (request, protocol) = control_request(&mut socket).await?;
                if protocol != "HTTP/1.1" {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "stream control requires ordinary HTTP/1.1",
                    ));
                }
                recorded.lock().unwrap().push(request);
                socket
                    .write_all(b"HTTP/1.1 200 Owned\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
                    .await?;
                let mut chunk = format!("{:x}\r\n", prefix.len()).into_bytes();
                chunk.extend_from_slice(&prefix);
                chunk.extend_from_slice(b"\r\n");
                socket.write_all(&chunk).await?;
                socket.flush().await?;
                Ok::<_, io::Error>(socket)
            }
            .await;
            match setup {
                Ok(mut socket) => {
                    let _ = flushed_tx.send(Ok(prefix.len()));
                    // A peer disconnect must not release the withheld remainder or EOF.
                    gate.notified().await;
                    let mut tail = format!("{:x}\r\n", remainder.len()).into_bytes();
                    tail.extend_from_slice(&remainder);
                    tail.extend_from_slice(b"\r\n0\r\n\r\n");
                    let _ = socket.write_all(&tail).await;
                    let _ = socket.shutdown().await;
                }
                Err(error) => {
                    let _ = flushed_tx.send(Err(error));
                }
            }
            done.store(true, Ordering::SeqCst);
        });
        Self {
            url,
            requests,
            flushed,
            release,
            released,
            finished,
            task: Some(task),
        }
    }
    pub fn completion(&self, prefix_bytes_written: usize) -> Value {
        let released = self.released.load(Ordering::SeqCst);
        json!({"prefix_flushed":true,"prefix_bytes_written":prefix_bytes_written,"return_before_release":!released,"remainder_released_before_return":released,"handler_finished_before_return":self.finished.load(Ordering::SeqCst)})
    }
    pub fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            self.release.notify_one();
        }
    }
    pub async fn finish(&mut self) -> std::result::Result<(), String> {
        self.release();
        let Some(mut task) = self.task.take() else {
            return Ok(());
        };
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut task).await {
            Ok(result) => result.map_err(|error| format!("owned stream handler failed: {error}")),
            Err(_) => {
                task.abort();
                let _ = task.await;
                Err("five-second failure guard: owned stream handler did not drain".into())
            }
        }
    }
}
impl Drop for StreamControl {
    fn drop(&mut self) {
        self.release();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
pub async fn receive(receiver: &mut mpsc::UnboundedReceiver<usize>) -> usize {
    tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap()
}
pub async fn registered(probe: &Probe) {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        probe.registered.notified(),
    )
    .await
    .unwrap();
}
