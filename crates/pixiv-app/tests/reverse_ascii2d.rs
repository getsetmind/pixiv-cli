use futures_util::FutureExt;
use pixiv_app::reverse_search::{
    ASCII2DClient, CallerContext, Error, ErrorCode, Loader, Provider, ProviderResponse,
    RedirectDecision, RedirectHook, SourceLoader, SourceLoaderOptions,
    ascii2d::{Client, FlareSolverrOptions, Options},
    http::{HttpRequest, HttpTransport},
};
use pixiv_sdk::{
    context::{Context, ContextError, ContextKey},
    fanbox::transport::{
        BodyFuture, ExternalError, Headers, RawBody, RawRead, RawResponse, TransportFuture,
    },
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

#[tokio::test]
async fn ascii2d_public_constructor_and_preflight_validate_the_provider() {
    let error = match Client::new(Options {
        endpoint: "ftp://ascii2d.invalid".into(),
        ..Options::default()
    }) {
        Ok(_) => panic!("non-HTTP endpoint accepted"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::InvalidRequest);
    assert_eq!(error.to_string(), "ascii2d endpoint is invalid");
    let transport = Arc::new(FixtureTransport::new(&[], Context::background()));
    let client = Client::new(Options {
        endpoint: "https://ascii2d.invalid/base".into(),
        http_transport: Some(transport.clone()),
        ..Options::default()
    })
    .unwrap();
    client
        .preflight(Arc::new(Context::background()))
        .await
        .unwrap();
    client.close().await.unwrap();
    client.close().await.unwrap();
    assert_eq!(transport.idle.load(Ordering::SeqCst), 1);
}

fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn header<'a>(headers: &'a Headers, name: &str) -> &'a str {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, values)| values.first())
        .map(String::as_str)
        .unwrap_or("")
}
fn canonical_headers(headers: &Headers) -> Value {
    let mut object = serde_json::Map::new();
    for (key, values) in headers {
        object.insert(key.to_ascii_lowercase(), json!(values));
    }
    Value::Object(object)
}
struct FixtureTransport {
    responses: Mutex<VecDeque<(usize, Value)>>,
    requests: Mutex<Vec<Value>>,
    wires: Mutex<Vec<Vec<u8>>>,
    closes: Arc<Vec<AtomicUsize>>,
    idle: AtomicUsize,
    cancel: Context,
}
impl FixtureTransport {
    fn new(responses: &[Value], cancel: Context) -> Self {
        Self {
            responses: Mutex::new(responses.iter().cloned().enumerate().collect()),
            requests: Mutex::new(Vec::new()),
            wires: Mutex::new(Vec::new()),
            closes: Arc::new(responses.iter().map(|_| AtomicUsize::new(0)).collect()),
            idle: AtomicUsize::new(0),
            cancel,
        }
    }
}
impl HttpTransport for FixtureTransport {
    fn send(
        &self,
        mut request: HttpRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let (index, spec) = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected provider request");
            assert_eq!(request.method, spec["method"].as_str().unwrap());
            assert_eq!(
                url::Url::parse(&request.url).unwrap().path(),
                spec["path"].as_str().unwrap()
            );
            let mut bytes = Vec::new();
            let body_nil = request.body.is_none();
            if !spec["close_upload"].as_bool().unwrap_or(false)
                && let Some(body) = &mut request.body
            {
                let _ = body.read_to_end(&mut bytes);
            }
            let mut headers = canonical_headers(&request.headers);
            let content_type = header(&request.headers, "content-type");
            let boundary = content_type.strip_prefix("multipart/form-data; boundary=");
            let wire = if let Some(boundary) = boundary {
                assert_eq!(boundary.len(), 60);
                assert!(
                    boundary
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                );
                headers["content-type"] =
                    json!(["multipart/form-data; boundary=<GENERATED_BOUNDARY>"]);
                replace_bytes(&bytes, boundary.as_bytes(), b"<GENERATED_BOUNDARY>")
            } else {
                bytes.clone()
            };
            let body_json = if content_type == "application/json" {
                Some(serde_json::from_slice::<Value>(&bytes).unwrap())
            } else {
                None
            };
            self.wires.lock().unwrap().push(wire);
            self.requests.lock().unwrap().push(json!({"method":request.method,"url":request.url,"headers":headers,"body_nil":body_nil,"content_length":request.content_length,"wire_length":bytes.len(),"json":body_json,"context_value":request.context.value(&ContextKey::new("ascii2d-fixture".to_owned())).and_then(|value| value.downcast::<String>().ok()).map(|value| value.as_ref().clone()).unwrap_or_default(),"context_canceled":request.context.error()==Some(ContextError::Canceled),"context_deadline":request.context.error()==Some(ContextError::DeadlineExceeded)}));
            if spec["cancel"] == "transport" {
                self.cancel.cancel();
            }
            if spec["cancel"] == "wait-context" {
                self.cancel.cancel();
                return Err(Box::new(request.context.cancelled().await) as ExternalError);
            }
            if !spec["error"].is_null() {
                return Err(
                    Box::new(std::io::Error::other("synthetic transport failure")) as ExternalError,
                );
            }
            let headers = if spec["headers"].is_null() {
                Headers::new()
            } else {
                serde_json::from_value(spec["headers"].clone()).unwrap()
            };
            Ok(Some(RawResponse {
                status: spec["status"].as_u64().unwrap() as u16,
                content_length: spec["body"].as_str().unwrap().len() as i64,
                headers,
                body: Some(Box::new(FixtureBody {
                    bytes: spec["body"].as_str().unwrap().as_bytes().to_vec(),
                    position: 0,
                    read_error: spec["read_error"].as_bool().unwrap_or(false),
                    cancel_read: spec["cancel"] == "read",
                    cancel: self.cancel.clone(),
                    index,
                    closes: self.closes.clone(),
                })),
            }))
        })
    }
    fn close_idle_connections(&self) {
        self.idle.fetch_add(1, Ordering::SeqCst);
    }
}
fn replace_bytes(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at..].starts_with(from) {
            output.extend_from_slice(to);
            at += from.len();
        } else {
            output.push(bytes[at]);
            at += 1;
        }
    }
    output
}
struct FixtureBody {
    bytes: Vec<u8>,
    position: usize,
    read_error: bool,
    cancel_read: bool,
    cancel: Context,
    index: usize,
    closes: Arc<Vec<AtomicUsize>>,
}
impl RawBody for FixtureBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            if self.cancel_read {
                self.cancel.cancel();
            }
            if self.read_error {
                return RawRead {
                    count: 0,
                    eof: false,
                    error: Some(Box::new(std::io::Error::other("synthetic read failure"))),
                };
            }
            let count = output.len().min(self.bytes.len() - self.position);
            output[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
            self.position += count;
            RawRead {
                count,
                eof: self.position == self.bytes.len(),
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        self.closes[self.index].fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}
struct FixtureRedirect {
    mode: String,
    calls: Arc<AtomicUsize>,
}
impl RedirectHook for FixtureRedirect {
    fn check(&self, _next_url: &str, _via: &[String]) -> Result<RedirectDecision, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.mode == "last" {
            Ok(RedirectDecision::Stop)
        } else {
            Err(Error::external(std::io::Error::other(
                "synthetic redirect failure",
            )))
        }
    }
}
fn context(mode: &str) -> Context {
    let context = if mode == "deadline" {
        Context::with_deadline(Instant::now())
    } else {
        Context::background().child()
    };
    if mode == "canceled" {
        context.cancel();
    }
    context.with_value(
        ContextKey::new("ascii2d-fixture".to_owned()),
        Arc::new("synthetic-context-value".to_owned()),
    )
}
fn outcome(
    stage: &str,
    session: bool,
    response: Option<ProviderResponse>,
    error: Option<Error>,
) -> Value {
    let response = response
        .map(|response| json!({"provider":response.provider,"matches":response.matches}))
        .or_else(|| {
            stage
                .starts_with("search:")
                .then(|| json!({"provider":"","matches":null}))
        });
    let error = error.map(|error| {
        let mut chain=Vec::new();let mut cause:Option<&(dyn std::error::Error+'static)>=Some(&error);
        while let Some(value)=cause {chain.push(value.to_string());cause=value.source();}
        json!({"code":error.code(),"message":error.to_string(),"chain":chain,"canceled":error.context_error()==Some(ContextError::Canceled),"deadline":error.context_error()==Some(ContextError::DeadlineExceeded),"challenge":chain.iter().any(|value|value=="ascii2d challenge detected"),"solver_unavailable":chain.iter().any(|value|value=="ascii2d: FlareSolverr service unavailable"),"solver_failed":chain.iter().any(|value|value=="ascii2d: FlareSolverr could not solve challenge"),"malformed_solver":chain.iter().any(|value|value=="ascii2d: malformed FlareSolverr response")})
    });
    json!({"stage":stage,"session":session,"response":response,"error":error})
}
fn comparable_outcome(mut value: Value) -> Value {
    if let Some(matches) = value["response"]["matches"].as_array() {
        let matches: Vec<pixiv_app::reverse_search::Match> =
            serde_json::from_value(Value::Array(matches.clone())).unwrap();
        value["response"]["matches"] = serde_json::to_value(matches).unwrap();
    }
    value
}

#[tokio::test]
async fn ascii2d_public_ports_replay_the_sealed_go_protocol() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../pixiv-cli/tests/fixtures/reverse-ascii2d.json"
    ))
    .unwrap();
    let mut checked = 0;
    let mut failures = Vec::new();
    for row in fixture["cases"].as_array().unwrap() {
        let input = &row["input"];
        let name = row["name"].as_str().unwrap();
        if input["operation"] == "nil-client"
            || input["operation"] == "zero-client"
            || input["operation"] == "nil-session"
            || input["context"] == "nil"
            || input["search_context"] == "nil"
            || input["snapshot"] == "nil"
        {
            continue;
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            std::panic::AssertUnwindSafe(async {
                let caller = context(input["context"].as_str().unwrap());
                let transport = Arc::new(FixtureTransport::new(
                    input["responses"].as_array().unwrap(),
                    caller.clone(),
                ));
                let solver = Arc::new(FixtureTransport::new(
                    input["solver_responses"].as_array().unwrap(),
                    caller.clone(),
                ));
                let redirect_calls = Arc::new(AtomicUsize::new(0));
                let redirect_hook = (!input["redirect"].as_str().unwrap().is_empty()).then(|| {
                    Arc::new(FixtureRedirect {
                        mode: input["redirect"].as_str().unwrap().to_owned(),
                        calls: redirect_calls.clone(),
                    }) as Arc<dyn RedirectHook>
                });
                let options = Options {
                    endpoint: input["endpoint"].as_str().unwrap().to_owned(),
                    proxy_url: input["proxy"].as_str().unwrap().to_owned(),
                    user_agent: input["user_agent"].as_str().unwrap().to_owned(),
                    http_transport: Some(transport.clone()),
                    redirect_hook,
                    flaresolverr: (!input["solver_url"].as_str().unwrap().is_empty()).then(|| {
                        FlareSolverrOptions {
                            url: input["solver_url"].as_str().unwrap().to_owned(),
                            proxy_url: input["solver_proxy"].as_str().unwrap().to_owned(),
                            http_transport: Some(solver.clone()),
                        }
                    }),
                };
                let mut actual = Vec::new();
                match Client::new(options) {
                    Err(error) => actual.push(outcome("new", false, None, Some(error))),
                    Ok(client) => {
                        actual.push(outcome("new", true, None, None));
                        if input["close_before"] == true {
                            actual.push(outcome(
                                "close-before",
                                false,
                                None,
                                client.close().await.err(),
                            ));
                        }
                        if input["operation"] == "preflight" {
                            actual.push(outcome(
                                "preflight",
                                false,
                                None,
                                client.preflight(Arc::new(caller.clone())).await.err(),
                            ));
                        }
                        if input["operation"] == "upload" {
                            let directory = tempfile::tempdir().unwrap();
                            let source = directory.path().join("owned-image");
                            let mut bytes = decode_hex(input["image_hex"].as_str().unwrap());
                            bytes.resize(input["image_size"].as_u64().unwrap() as usize, 0);
                            std::fs::write(&source, &bytes).unwrap();
                            let snapshot = Loader::new(SourceLoaderOptions {
                                temp_dir: directory.path().to_owned(),
                                ..SourceLoaderOptions::default()
                            })
                            .load(Arc::new(Context::background()), source.to_str().unwrap())
                            .await
                            .unwrap();
                            if input["snapshot"] == "closed" {
                                snapshot.close().unwrap();
                            }
                            for _ in 0..input["repeat"].as_u64().unwrap() {
                                match client
                                    .upload(Arc::new(caller.clone()), snapshot.clone())
                                    .await
                                {
                                    Err(error) => {
                                        actual.push(outcome("upload", false, None, Some(error)))
                                    }
                                    Ok(session) => {
                                        actual.push(outcome("upload", true, None, None));
                                        for provider in input["providers"].as_array().unwrap() {
                                            let provider =
                                                Provider::from(provider.as_str().unwrap());
                                            let search_caller = if input["search_context"]
                                                .as_str()
                                                .unwrap()
                                                .is_empty()
                                            {
                                                caller.clone()
                                            } else {
                                                context(input["search_context"].as_str().unwrap())
                                            };
                                            let result = session
                                                .search(Arc::new(search_caller), provider.clone())
                                                .await;
                                            actual.push(match result {
                                                Ok(response) => outcome(
                                                    &format!("search:{}", provider.as_str()),
                                                    false,
                                                    Some(response),
                                                    None,
                                                ),
                                                Err(error) => outcome(
                                                    &format!("search:{}", provider.as_str()),
                                                    false,
                                                    None,
                                                    Some(error),
                                                ),
                                            });
                                        }
                                    }
                                }
                                if input["close_from_session"] == true {
                                    actual.push(outcome(
                                        "close-session-client",
                                        false,
                                        None,
                                        client.close().await.err(),
                                    ));
                                }
                            }
                        }
                        actual.push(outcome("close", false, None, client.close().await.err()));
                        actual.push(outcome(
                            "close-again",
                            false,
                            None,
                            client.close().await.err(),
                        ));
                    }
                }
                let expected: Vec<_> = row["outcomes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .cloned()
                    .map(comparable_outcome)
                    .collect();
                assert_eq!(actual, expected, "outcomes: {name}");
                check_requests(
                    name,
                    &transport.requests.lock().unwrap(),
                    &transport.wires.lock().unwrap(),
                    row["requests"].as_array().unwrap(),
                    false,
                );
                check_requests(
                    name,
                    &solver.requests.lock().unwrap(),
                    &solver.wires.lock().unwrap(),
                    row["solver_requests"].as_array().unwrap(),
                    true,
                );
                assert_eq!(
                    transport
                        .closes
                        .iter()
                        .map(|count| count.load(Ordering::SeqCst))
                        .collect::<Vec<_>>(),
                    row["response_closes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|n| n.as_u64().unwrap() as usize)
                        .collect::<Vec<_>>(),
                    "response closes: {name}"
                );
                assert_eq!(
                    solver
                        .closes
                        .iter()
                        .map(|count| count.load(Ordering::SeqCst))
                        .collect::<Vec<_>>(),
                    row["solver_response_closes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|n| n.as_u64().unwrap() as usize)
                        .collect::<Vec<_>>(),
                    "solver response closes: {name}"
                );
                assert_eq!(
                    transport.idle.load(Ordering::SeqCst),
                    row["idle_closes"].as_u64().unwrap() as usize,
                    "idle: {name}"
                );
                assert_eq!(
                    solver.idle.load(Ordering::SeqCst),
                    row["solver_idle_closes"].as_u64().unwrap() as usize,
                    "solver idle: {name}"
                );
                assert_eq!(
                    redirect_calls.load(Ordering::SeqCst),
                    row["redirect_calls"].as_u64().unwrap() as usize,
                    "redirect hook: {name}"
                );
            })
            .catch_unwind(),
        )
        .await;
        if !matches!(result, Ok(Ok(()))) {
            failures.push(name.to_owned());
        }
        checked += 1;
    }
    assert_eq!(checked, 176);
    assert!(
        failures.is_empty(),
        "sealed public rows differed: {}",
        failures.join(", ")
    );
}
fn check_requests(
    name: &str,
    actual: &[Value],
    wires: &[Vec<u8>],
    expected: &[Value],
    solver: bool,
) {
    assert_eq!(actual.len(), expected.len(), "request count: {name}");
    let mut session_name = None;
    for ((actual, expected), wire_bytes) in actual.iter().zip(expected).zip(wires) {
        for key in [
            "method",
            "url",
            "body_nil",
            "context_value",
            "context_canceled",
            "context_deadline",
        ] {
            assert_eq!(actual[key], expected[key], "request {key}: {name}");
        }
        let expected_headers: Headers =
            serde_json::from_value(expected["headers"].clone()).unwrap();
        assert_eq!(
            actual["headers"],
            canonical_headers(&expected_headers),
            "request headers: {name}"
        );
        if solver {
            let mut payload = actual["json"].clone();
            let generated = payload["session"].as_str().unwrap().to_owned();
            assert!(generated.starts_with("pixiv-cli-ascii2d-"));
            assert_eq!(generated.len(), 50);
            assert!(
                generated[18..]
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            );
            if let Some(previous) = &session_name {
                assert_eq!(&generated, previous);
            } else {
                session_name = Some(generated);
            }
            payload["session"] = expected["json"]["session"].clone();
            assert_eq!(payload, expected["json"], "solver payload: {name}");
            let actual_wire = wire_bytes;
            let actual_session = actual["json"]["session"].as_str().unwrap();
            let expected_session = expected["json"]["session"].as_str().unwrap();
            assert_eq!(
                replace_bytes(
                    actual_wire,
                    actual_session.as_bytes(),
                    expected_session.as_bytes()
                ),
                decode_hex(expected["json_hex"].as_str().unwrap()),
                "solver exact JSON framing: {name}"
            );
            assert_eq!(
                actual["content_length"].as_i64().unwrap() - 32 + 7,
                expected["content_length"].as_i64().unwrap(),
                "solver body length: {name}"
            );
        } else {
            assert_eq!(
                actual["content_length"], expected["content_length"],
                "request length: {name}"
            );
            if expected["multipart_details"]["body_read"] == true {
                assert_eq!(
                    actual["wire_length"], expected["multipart_details"]["wire_length"],
                    "multipart wire length: {name}"
                );
                if let Some(wire) = expected["multipart_details"]["wire_hex"].as_str() {
                    assert_eq!(
                        wire_bytes.as_slice(),
                        decode_hex(wire).as_slice(),
                        "complete multipart framing: {name}"
                    );
                } else {
                    check_large_wire(name, wire_bytes, expected);
                }
            }
        }
    }
}
fn check_large_wire(name: &str, bytes: &[u8], expected: &Value) {
    use sha2::{Digest, Sha256};
    let mut at = 0;
    for part in expected["parts"].as_array().unwrap() {
        for key in ["delimiter_hex", "raw_headers_hex"] {
            let marker = decode_hex(part[key].as_str().unwrap());
            assert_eq!(
                &bytes[at..at + marker.len()],
                marker.as_slice(),
                "multipart {key}: {name}"
            );
            at += marker.len();
        }
        let size = part["size"].as_u64().unwrap() as usize;
        let digest = Sha256::digest(&bytes[at..at + size])
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            digest,
            part["sha256"].as_str().unwrap(),
            "multipart payload: {name}"
        );
        at += size;
    }
    assert_eq!(
        &bytes[at..],
        decode_hex(
            expected["multipart_details"]["terminator_hex"]
                .as_str()
                .unwrap()
        )
        .as_slice(),
        "multipart terminator: {name}"
    );
}

struct ProtocolTransport {
    requests: Mutex<Vec<Headers>>,
    idle: AtomicUsize,
    challenged: tokio::sync::Notify,
    challenge_count: AtomicUsize,
    rechallenge: AtomicUsize,
}
impl Default for ProtocolTransport {
    fn default() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            idle: AtomicUsize::new(0),
            challenged: tokio::sync::Notify::new(),
            challenge_count: AtomicUsize::new(0),
            rechallenge: AtomicUsize::new(0),
        }
    }
}
fn synthetic_response(status: u16, headers: Headers, bytes: Vec<u8>) -> RawResponse {
    RawResponse {
        status,
        headers,
        content_length: bytes.len() as i64,
        body: Some(Box::new(FixtureBody {
            bytes,
            position: 0,
            read_error: false,
            cancel_read: false,
            cancel: Context::background(),
            index: 0,
            closes: Arc::new(vec![AtomicUsize::new(0)]),
        })),
    }
}
impl HttpTransport for ProtocolTransport {
    fn send(
        &self,
        mut request: HttpRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request.headers.clone());
            let mut headers = Headers::new();
            if !header(&request.headers, "Cookie").contains("cf_clearance=shared-clearance") {
                headers.insert("Cf-Mitigated".into(), vec!["challenge".into()]);
                self.challenge_count.fetch_add(1, Ordering::SeqCst);
                self.challenged.notify_waiters();
                return Ok(Some(synthetic_response(403, headers, Vec::new())));
            }
            if self
                .rechallenge
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                    count.checked_sub(1)
                })
                .is_ok()
            {
                headers.insert("Cf-Mitigated".into(), vec!["challenge".into()]);
                return Ok(Some(synthetic_response(403, headers, Vec::new())));
            }
            if request.method == "POST" {
                let mut bytes = Vec::new();
                request
                    .body
                    .as_mut()
                    .unwrap()
                    .read_to_end(&mut bytes)
                    .unwrap();
                headers.insert(
                    "Location".into(),
                    vec!["/search/color/0123456789abcdef0123456789abcdef".into()],
                );
                Ok(Some(synthetic_response(302, headers, Vec::new())))
            } else {
                let form=b"<form id=file_upload action=/search/file method=post enctype=multipart/form-data><input name=authenticity_token type=hidden value=owned-csrf><input name=file type=file></form>";
                Ok(Some(synthetic_response(200, headers, form.to_vec())))
            }
        })
    }
    fn close_idle_connections(&self) {
        self.idle.fetch_add(1, Ordering::SeqCst);
    }
}
struct SolverTransport {
    requests: Mutex<Vec<Value>>,
    contexts: Mutex<Vec<CallerContext>>,
    get_count: AtomicUsize,
    expiry: Mutex<Value>,
    canceled: AtomicUsize,
    get_started: tokio::sync::Notify,
    get_release: tokio::sync::Notify,
    released: std::sync::atomic::AtomicBool,
    destroy_started: tokio::sync::Notify,
    destroy_count: AtomicUsize,
    destroy_release: tokio::sync::Notify,
    destroy_released: std::sync::atomic::AtomicBool,
    idle: AtomicUsize,
}
impl Default for SolverTransport {
    fn default() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            contexts: Mutex::new(Vec::new()),
            get_count: AtomicUsize::new(0),
            expiry: Mutex::new(json!("2030-01-02T03:04:05Z")),
            canceled: AtomicUsize::new(0),
            get_started: tokio::sync::Notify::new(),
            get_release: tokio::sync::Notify::new(),
            released: std::sync::atomic::AtomicBool::new(false),
            destroy_started: tokio::sync::Notify::new(),
            destroy_count: AtomicUsize::new(0),
            destroy_release: tokio::sync::Notify::new(),
            destroy_released: std::sync::atomic::AtomicBool::new(true),
            idle: AtomicUsize::new(0),
        }
    }
}
impl HttpTransport for SolverTransport {
    fn send(
        &self,
        mut request: HttpRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let mut bytes = Vec::new();
            request
                .body
                .as_mut()
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            let payload: Value = serde_json::from_slice(&bytes).unwrap();
            self.requests.lock().unwrap().push(payload.clone());
            self.contexts.lock().unwrap().push(request.context.clone());
            let response = match payload["cmd"].as_str().unwrap() {
                "request.get" => {
                    self.get_count.fetch_add(1, Ordering::SeqCst);
                    self.get_started.notify_waiters();
                    loop {
                        let notified = self.get_release.notified();
                        tokio::pin!(notified);
                        notified.as_mut().enable();
                        if self.released.load(Ordering::SeqCst) {
                            break;
                        }
                        tokio::select! {_=&mut notified=>{},_=request.context.cancelled()=>{self.canceled.fetch_add(1,Ordering::SeqCst);return Err(Box::new(std::io::Error::other("owned solver canceled")) as ExternalError);}}
                    }
                    json!({"status":"ok","solution":{"userAgent":"Mozilla/5.0 (X11; Linux x86_64) Chrome/146.0.0.0","cookies":[{"name":"discarded","value":"not-transmitted"},{"name":"cf_clearance","value":"shared-clearance","expires":self.expiry.lock().unwrap().clone()}]}})
                }
                "sessions.destroy" => {
                    self.destroy_count.fetch_add(1, Ordering::SeqCst);
                    self.destroy_started.notify_waiters();
                    loop {
                        let notified = self.destroy_release.notified();
                        tokio::pin!(notified);
                        notified.as_mut().enable();
                        if self.destroy_released.load(Ordering::SeqCst) {
                            break;
                        }
                        notified.await;
                    }
                    json!({"status":"ok"})
                }
                "sessions.create" => json!({"status":"ok"}),
                other => panic!("unexpected solver command: {other}"),
            };
            Ok(Some(synthetic_response(
                200,
                Headers::new(),
                serde_json::to_vec(&response).unwrap(),
            )))
        })
    }
    fn close_idle_connections(&self) {
        self.idle.fetch_add(1, Ordering::SeqCst);
    }
}
async fn wait_count(count: &AtomicUsize, notify: &tokio::sync::Notify, wanted: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if count.load(Ordering::SeqCst) >= wanted {
                return;
            }
            notified.await;
        }
    })
    .await
    .expect("owned protocol progress");
}
async fn owned_snapshot(directory: &tempfile::TempDir) -> Arc<pixiv_app::reverse_search::Snapshot> {
    let source = directory.path().join("image");
    std::fs::write(&source, b"\x89PNG\r\n\x1a\nowned-image").unwrap();
    Loader::new(SourceLoaderOptions {
        temp_dir: directory.path().to_owned(),
        ..SourceLoaderOptions::default()
    })
    .load(Arc::new(Context::background()), source.to_str().unwrap())
    .await
    .unwrap()
}
fn protocol_client(provider: Arc<ProtocolTransport>, solver: Arc<SolverTransport>) -> Arc<Client> {
    Arc::new(
        Client::new(Options {
            endpoint: "https://ascii2d.invalid".into(),
            http_transport: Some(provider),
            flaresolverr: Some(FlareSolverrOptions {
                url: "http://solver.invalid".into(),
                proxy_url: "socks5://browser-proxy.invalid:1080".into(),
                http_transport: Some(solver),
            }),
            ..Options::default()
        })
        .unwrap(),
    )
}
#[tokio::test]
async fn canceling_one_upload_preserves_shared_solver_and_cached_clearance() {
    let directory = tempfile::tempdir().unwrap();
    let snapshot = owned_snapshot(&directory).await;
    let provider = Arc::new(ProtocolTransport::default());
    let solver = Arc::new(SolverTransport::default());
    let client = protocol_client(provider.clone(), solver.clone());
    let first_context = context("");
    let second_context = context("");
    let first = {
        let client = client.clone();
        let snapshot = snapshot.clone();
        let context = first_context.clone();
        tokio::spawn(async move { client.upload(Arc::new(context), snapshot).await })
    };
    wait_count(&solver.get_count, &solver.get_started, 1).await;
    let second = {
        let client = client.clone();
        let snapshot = snapshot.clone();
        tokio::spawn(async move { client.upload(Arc::new(second_context), snapshot).await })
    };
    wait_count(&provider.challenge_count, &provider.challenged, 2).await;
    tokio::task::yield_now().await;
    first_context.cancel();
    assert_eq!(
        first.await.unwrap().err().unwrap().context_error(),
        Some(ContextError::Canceled)
    );
    assert_eq!(solver.canceled.load(Ordering::SeqCst), 0);
    solver.released.store(true, Ordering::SeqCst);
    solver.get_release.notify_waiters();
    second.await.unwrap().unwrap();
    client
        .upload(Arc::new(context("")), snapshot)
        .await
        .unwrap();
    assert_eq!(solver.get_count.load(Ordering::SeqCst), 1);
    {
        let contexts = solver.contexts.lock().unwrap();
        assert!(contexts.iter().all(|context| context.deadline().is_none()));
        assert!(contexts.iter().all(|context| {
            context
                .value(&ContextKey::new("ascii2d-fixture".to_owned()))
                .is_some()
        }));
    }
    assert!(
        provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|headers| !header(headers, "Cookie").contains("not-transmitted"))
    );
    client.close().await.unwrap();
    assert_eq!(solver.destroy_count.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn canceled_last_upload_releases_solver_and_next_upload_resolves_again() {
    let directory = tempfile::tempdir().unwrap();
    let snapshot = owned_snapshot(&directory).await;
    let provider = Arc::new(ProtocolTransport::default());
    let solver = Arc::new(SolverTransport::default());
    let client = protocol_client(provider, solver.clone());
    let caller = context("");
    let upload = {
        let client = client.clone();
        let snapshot = snapshot.clone();
        let caller = caller.clone();
        tokio::spawn(async move { client.upload(Arc::new(caller), snapshot).await })
    };
    wait_count(&solver.get_count, &solver.get_started, 1).await;
    caller.cancel();
    assert_eq!(
        upload.await.unwrap().err().unwrap().context_error(),
        Some(ContextError::Canceled)
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while solver.canceled.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    solver.released.store(true, Ordering::SeqCst);
    solver.get_release.notify_waiters();
    client
        .upload(Arc::new(context("")), snapshot)
        .await
        .unwrap();
    assert_eq!(solver.get_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        solver
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["cmd"] == "sessions.create")
            .count(),
        1
    );
    client.close().await.unwrap();
}
#[tokio::test]
async fn dropped_close_waiter_keeps_background_destroy_once() {
    let directory = tempfile::tempdir().unwrap();
    let snapshot = owned_snapshot(&directory).await;
    let provider = Arc::new(ProtocolTransport::default());
    let solver = Arc::new(SolverTransport::default());
    solver.released.store(true, Ordering::SeqCst);
    solver.destroy_released.store(false, Ordering::SeqCst);
    let client = protocol_client(provider.clone(), solver.clone());
    client
        .upload(Arc::new(context("")), snapshot)
        .await
        .unwrap();
    let close = {
        let client = client.clone();
        tokio::spawn(async move { client.close().await })
    };
    wait_count(&solver.destroy_count, &solver.destroy_started, 1).await;
    close.abort();
    solver.destroy_released.store(true, Ordering::SeqCst);
    solver.destroy_release.notify_waiters();
    client.close().await.unwrap();
    client.close().await.unwrap();
    assert_eq!(solver.destroy_count.load(Ordering::SeqCst), 1);
    assert_eq!(solver.idle.load(Ordering::SeqCst), 1);
    assert_eq!(provider.idle.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn expired_clearance_is_used_for_its_upload_but_not_reused_by_later_upload() {
    let directory = tempfile::tempdir().unwrap();
    let snapshot = owned_snapshot(&directory).await;
    let provider = Arc::new(ProtocolTransport::default());
    let solver = Arc::new(SolverTransport::default());
    solver.released.store(true, Ordering::SeqCst);
    *solver.expiry.lock().unwrap() = json!(1);
    let client = protocol_client(provider, solver.clone());
    client
        .upload(Arc::new(context("")), snapshot.clone())
        .await
        .unwrap();
    client
        .upload(Arc::new(context("")), snapshot)
        .await
        .unwrap();
    assert_eq!(solver.get_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        solver
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["cmd"] == "sessions.create")
            .count(),
        1
    );
    client.close().await.unwrap();
}
#[tokio::test]
async fn rechallenged_recovery_invalidates_clearance_before_next_upload() {
    let directory = tempfile::tempdir().unwrap();
    let snapshot = owned_snapshot(&directory).await;
    let provider = Arc::new(ProtocolTransport::default());
    provider.rechallenge.store(1, Ordering::SeqCst);
    let solver = Arc::new(SolverTransport::default());
    solver.released.store(true, Ordering::SeqCst);
    let client = protocol_client(provider, solver.clone());
    assert_eq!(
        client
            .upload(Arc::new(context("")), snapshot.clone())
            .await
            .err()
            .unwrap()
            .code(),
        ErrorCode::SolverFailed
    );
    client
        .upload(Arc::new(context("")), snapshot)
        .await
        .unwrap();
    assert_eq!(solver.get_count.load(Ordering::SeqCst), 2);
    client.close().await.unwrap();
}
#[tokio::test]
async fn close_cancels_active_shared_solver_without_canceling_its_upload_caller() {
    let directory = tempfile::tempdir().unwrap();
    let snapshot = owned_snapshot(&directory).await;
    let provider = Arc::new(ProtocolTransport::default());
    let solver = Arc::new(SolverTransport::default());
    let client = protocol_client(provider, solver.clone());
    let caller = context("");
    let upload = {
        let client = client.clone();
        let caller = caller.clone();
        tokio::spawn(async move { client.upload(Arc::new(caller), snapshot).await })
    };
    wait_count(&solver.get_count, &solver.get_started, 1).await;
    client.close().await.unwrap();
    let error = upload.await.unwrap().err().unwrap();
    assert_eq!(error.code(), ErrorCode::Unknown);
    assert_eq!(error.to_string(), "ascii2d: solver state cache is closed");
    assert_eq!(caller.error(), None);
    assert_eq!(solver.canceled.load(Ordering::SeqCst), 1);
    assert_eq!(solver.destroy_count.load(Ordering::SeqCst), 1);
}
