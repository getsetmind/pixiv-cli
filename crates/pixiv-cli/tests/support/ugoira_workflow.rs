use pixiv_app::{database::Database, execution::Execution, lifecycle::Context};
use pixiv_sdk::{
    Error, Reason,
    error::RetryAdvice,
    transport::{HttpTransport, JsonResponse, Request, Response, Transport, encode_query_pairs},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::Notify;

#[derive(Deserialize)]
pub struct Fixture {
    pub reference_commit: String,
    pub source_sha256: BTreeMap<String, String>,
    pub metadata_fixture_sha256: String,
    pub cases: Vec<Case>,
}
#[derive(Deserialize)]
pub struct Case {
    pub input: Input,
    pub outcome: Outcome,
    pub reuse: Option<Outcome>,
}
#[derive(Clone, Deserialize)]
pub struct Input {
    pub name: String,
    pub mode: String,
    pub scenario: String,
    pub config: String,
    pub detail_body: Value,
    pub metadata_body: Value,
}
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct RequestObservation {
    pub method: String,
    pub host: String,
    pub uri: String,
    pub account: i64,
    pub revision: i64,
    pub persisted: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub grant_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub form_keys: Vec<String>,
}
#[derive(Debug, Deserialize, PartialEq)]
pub struct State {
    pub id: i64,
    pub revision: i64,
    pub rotated: bool,
    pub frozen: bool,
    pub selected: bool,
}
#[derive(Debug, Deserialize, PartialEq)]
pub struct ErrorObservation {
    pub message: String,
    pub product: String,
    pub operation: String,
    pub reason: String,
    pub detail: String,
    pub transport: String,
    pub http_status: u16,
    pub retry_safe: bool,
    pub retry_has_after: bool,
    pub canceled: bool,
    pub deadline: bool,
    pub broken_pipe: bool,
}
#[derive(Deserialize)]
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub exit: i32,
    pub error: Option<ErrorObservation>,
    pub requests: Vec<RequestObservation>,
    pub states: Vec<State>,
    pub config_unchanged: bool,
}
#[derive(Default)]
pub struct Observed {
    pub requests: Vec<RequestObservation>,
    pub opens: Vec<i64>,
    pub closes: usize,
    pub active: usize,
    pub writes: usize,
    pub all_writes_under_lease: bool,
    pub output_visible_at_close: bool,
    pub output: Vec<u8>,
    pub counts: BTreeMap<(i64, String), usize>,
    pub reuse: bool,
}

pub struct ApiTransport {
    pub input: Input,
    pub database: Arc<Mutex<Database>>,
    pub observed: Arc<Mutex<Observed>>,
    pub opened: AtomicBool,
    pub pending_url: Option<String>,
    pub http: HttpTransport,
}
impl Drop for ApiTransport {
    fn drop(&mut self) {
        if self.opened.load(Ordering::SeqCst) {
            let mut observed = self.observed.lock().unwrap();
            observed.closes += 1;
            observed.active -= 1;
            observed.output_visible_at_close |= !observed.output.is_empty();
        }
    }
}
impl ApiTransport {
    async fn wire(&self, mut request: Request) -> pixiv_sdk::Result<JsonResponse> {
        let oauth = request.operation == "Open";
        let url = url::Url::parse(&request.url).unwrap();
        let path = url.path().to_owned();
        let host = url.host_str().unwrap().to_owned();
        let token = if oauth {
            request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1
                .clone()
        } else {
            request
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
                .unwrap()
                .1
                .clone()
        };
        let id: i64 = token.rsplit('-').next().unwrap().parse().unwrap();
        let account = self.database.lock().unwrap().get_pixiv(id).unwrap();
        let query = if oauth {
            String::new()
        } else {
            encode_query_pairs(&request.parameters)
        };
        let uri = if query.is_empty() {
            path.clone()
        } else {
            format!("{path}?{query}")
        };
        let persisted = !oauth
            && account.credential_revision >= 2
            && account.refresh_token_copy() == format!("fixture-rotated-{id}").as_bytes();
        let mut observation = RequestObservation {
            method: request.method.to_string(),
            host: host.clone(),
            uri,
            account: id,
            revision: account.credential_revision,
            persisted,
            grant_type: String::new(),
            form_keys: vec![],
        };
        let (status, retry_after, body, pending) = {
            let mut observed = self.observed.lock().unwrap();
            let mut status = 200;
            let mut retry_after = None;
            let mut pending = false;
            let mut body;
            if oauth {
                assert_eq!(host, "oauth.secure.pixiv.net");
                assert_eq!(path, "/auth/token");
                assert_eq!(request.method.as_str(), "POST");
                assert_eq!(token.as_bytes(), account.refresh_token_copy());
                observation.grant_type = request
                    .parameters
                    .iter()
                    .find(|(key, _)| key == "grant_type")
                    .unwrap()
                    .1
                    .clone();
                observation.form_keys = request
                    .parameters
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect();
                observation.form_keys.sort();
                body = json!({"access_token":format!("fixture-access-{id}"),"refresh_token":format!("fixture-rotated-{id}"),"expires_in":3600,"user":{"id":id}});
                if self.input.scenario == "refresh_401" {
                    status = 401;
                } else {
                    observed.opens.push(id);
                    observed.active += 1;
                    self.opened.store(true, Ordering::SeqCst);
                }
            } else {
                assert_eq!(host, "app-api.pixiv.net");
                assert_eq!(request.method.as_str(), "GET");
                assert_eq!(query, "illust_id=42");
                assert_eq!(token, format!("Bearer fixture-access-{id}"));
                assert!(persisted, "API request preceded credential persistence");
                let stage = match path.as_str() {
                    "/v1/illust/detail" => "detail",
                    "/v1/ugoira/metadata" => "metadata",
                    _ => panic!("unexpected media/API request {path}"),
                };
                body = if stage == "detail" {
                    self.input.detail_body.clone()
                } else {
                    self.input.metadata_body.clone()
                };
                let count = observed.counts.entry((id, stage.into())).or_default();
                *count += 1;
                let count = *count;
                if !observed.reuse {
                    match self.input.scenario.as_str() {
                        "detail_401" | "metadata_401"
                            if self.input.scenario == format!("{stage}_401") =>
                        {
                            status = 401
                        }
                        "detail_malformed" if stage == "detail" => body = json!({}),
                        "detail_retry" | "metadata_retry"
                            if self.input.scenario == format!("{stage}_retry") && count == 1 =>
                        {
                            status = 429;
                            retry_after = Some(chrono::TimeDelta::zero());
                        }
                        "detail_replay"
                        | "metadata_replay"
                        | "metadata_replay_kind"
                        | "metadata_replay_malformed"
                        | "all_rate_limited" => {
                            let failing_stage = if self.input.scenario == "detail_replay" {
                                "detail"
                            } else {
                                "metadata"
                            };
                            if stage == failing_stage
                                && (id == 42 || self.input.scenario == "all_rate_limited")
                            {
                                status = 429;
                                retry_after = Some(chrono::TimeDelta::seconds(if count == 1 {
                                    0
                                } else {
                                    120
                                }));
                            }
                            if id == 43
                                && self.input.scenario == "metadata_replay_kind"
                                && stage == "detail"
                            {
                                body["illust"]["type"] = json!("illust");
                            }
                            if id == 43
                                && self.input.scenario == "metadata_replay_malformed"
                                && stage == "metadata"
                            {
                                body = json!({});
                            }
                        }
                        _ => {}
                    }
                    pending = self.input.scenario == format!("{stage}_cancel")
                        || self.input.scenario == format!("{stage}_deadline");
                }
            }
            observed.requests.push(observation);
            (status, retry_after, body, pending)
        };
        if pending {
            // The outer SDK URL remains observable; only this owned test transport routes the real HTTP body to loopback.
            request.url.clone_from(self.pending_url.as_ref().unwrap());
            return self.http.send_json(request).await;
        }
        Ok(JsonResponse {
            status,
            retry_after,
            body: serde_json::to_vec(&body).unwrap(),
        })
    }
}
impl Transport for ApiTransport {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let response = self.wire(request).await?;
        Ok(Response {
            status: response.status,
            retry_after: response.retry_after,
            body: serde_json::from_slice(&response.body).unwrap(),
        })
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        self.wire(request).await
    }
}

pub struct Output {
    pub execution: Arc<Execution<ApiTransport>>,
    pub scenario: String,
    pub observed: Arc<Mutex<Observed>>,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let execution = self.execution.clone();
        let (sent, received) = std::sync::mpsc::channel();
        tokio::spawn(async move {
            let context = Context::new();
            let blocked = match tokio::time::timeout(
                Duration::from_millis(5),
                execution.open_client(&context, 0, Some("")),
            )
            .await
            {
                Err(_) => true,
                Ok(Ok(lease)) => {
                    let _ = lease.close();
                    false
                }
                Ok(Err(error)) => panic!("writer gate probe failed before waiting: {error}"),
            };
            let _ = sent.send(blocked);
        });
        assert!(
            received
                .recv_timeout(Duration::from_secs(5))
                .expect("writer lease probe did not stop"),
            "genuine Execution gate was free during output"
        );
        let mut observed = self.observed.lock().unwrap();
        observed.writes += 1;
        observed.all_writes_under_lease &= observed.active == 1;
        let count = if self.scenario.starts_with("writer_") {
            bytes.len().min(8)
        } else {
            bytes.len()
        };
        let failure = match self.scenario.as_str() {
            "writer_broken" => Some(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe")),
            "writer_denied" => Some(io::Error::other("fixture output denied")),
            "writer_retryable" if observed.writes == 1 => Some(io::Error::other(
                Error::new(Reason::RateLimited, "fixture_writer").with_retry(RetryAdvice {
                    safe: true,
                    after: Some(chrono::Utc::now() + chrono::TimeDelta::seconds(120)),
                }),
            )),
            _ => None,
        };
        let count = if self.scenario == "writer_retryable" && observed.writes > 1 {
            bytes.len()
        } else {
            count
        };
        observed.output.extend_from_slice(&bytes[..count]);
        match failure {
            Some(error) => Err(error),
            None => Ok(count),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct PendingServer {
    pub url: String,
    pub started: Arc<Notify>,
    pub closed: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl PendingServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/fixture", listener.local_addr().unwrap());
        let started = Arc::new(Notify::new());
        let closed = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_started = started.clone();
        let thread_closed = closed.clone();
        let thread_shutdown = shutdown.clone();
        let thread = thread::spawn(move || {
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if thread_shutdown.load(Ordering::SeqCst) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("owned loopback accept: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let end = Instant::now() + Duration::from_secs(5);
            let mut head = vec![];
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                match socket.read(&mut byte) {
                    Ok(1) => head.push(byte[0]),
                    Ok(0) if thread_shutdown.load(Ordering::SeqCst) => return,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        if thread_shutdown.load(Ordering::SeqCst) {
                            return;
                        }
                    }
                    other => panic!("owned loopback head: {other:?}"),
                }
                assert!(head.len() < 65536 && Instant::now() < end);
            }
            let head = String::from_utf8(head).unwrap();
            assert!(head.starts_with("GET /fixture?illust_id=42 HTTP/1.1\r\n"));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{").unwrap();
            socket.flush().unwrap();
            thread_started.notify_one();
            loop {
                let mut byte = [0];
                match socket.read(&mut byte) {
                    Ok(0) => {
                        thread_closed.store(true, Ordering::SeqCst);
                        return;
                    }
                    Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {
                        thread_closed.store(true, Ordering::SeqCst);
                        return;
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        if thread_shutdown.load(Ordering::SeqCst) {
                            return;
                        }
                        assert!(
                            Instant::now() < end,
                            "owned pending HTTP response was not released"
                        );
                    }
                    other => panic!("owned pending HTTP body: {other:?}"),
                }
            }
        });
        Self {
            url,
            started,
            closed,
            shutdown,
            thread: Some(thread),
        }
    }
    pub async fn assert_closed(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.closed.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("pending HTTP body/socket owner retained after cancel/deadline");
    }
}
impl Drop for PendingServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take()
            && let Err(panic) = thread.join()
            && !thread::panicking()
        {
            std::panic::resume_unwind(panic);
        }
    }
}
