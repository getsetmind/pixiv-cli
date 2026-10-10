#![allow(dead_code)]

use pixiv_sdk::{
    context::Context,
    diagnostics::{Event, Scope},
    fanbox::transport::{
        BodyFuture, ExternalError, Headers, RawBody, RawRead, RawRequest, RawResponse,
        RawTransport, TransportFuture,
    },
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

#[derive(Default)]
pub struct Observation {
    pub calls: Vec<Value>,
    pub requests: Vec<Value>,
    pub responses: Vec<Value>,
    pub trace: Vec<String>,
    pub diagnostics: Vec<Value>,
    pub lease_opens: usize,
    pub lease_closes: usize,
    pub live: usize,
    pub max_live_leases: usize,
}

pub struct Harness {
    pub expected: Value,
    pub observation: Mutex<Observation>,
    pub started: Semaphore,
    pub release_first: Semaphore,
    pub release_second: Semaphore,
    pub completed: Semaphore,
}
impl Harness {
    pub fn new(expected: Value) -> Arc<Self> {
        Arc::new(Self {
            expected,
            observation: Mutex::new(Observation::default()),
            started: Semaphore::new(0),
            release_first: Semaphore::new(0),
            release_second: Semaphore::new(0),
            completed: Semaphore::new(0),
        })
    }
    pub fn trace(&self, text: impl Into<String>) {
        self.observation.lock().unwrap().trace.push(text.into());
    }
    pub fn context(self: &Arc<Self>) -> Context {
        let harness = self.clone();
        Context::background().with_scope(Scope::new(Some(Arc::new(move |event: Event| {
            assert!(event.duration_ns >= 0);
            let completed = event.module == "FANBOX MCP server" && matches!(event.kind.as_str(), "completed" | "failed");
            harness.observation.lock().unwrap().diagnostics.push(json!({"module":event.module,"kind":event.kind,"operation":event.operation,"resource":event.resource,"route":event.route,"target":event.target,"proxy":event.proxy,"user_agent":event.user_agent,"reason":event.reason,"status":event.status,"count":event.count,"request_id":event.request_id,"duration_nonnegative":event.duration_ns>=0}));
            if completed { harness.completed.add_permits(1); }
        })), "FANBOX CLI", 999))
    }
    pub async fn await_started(&self) {
        self.started.acquire().await.unwrap().forget();
    }
    pub async fn await_completed(&self) {
        self.completed.acquire().await.unwrap().forget();
    }
}

pub struct Transport(pub Arc<Harness>);
impl RawTransport for Transport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let h = &self.0;
            let header = |name: &str| {
                request
                    .headers
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .and_then(|(_, values)| values.first())
                    .cloned()
                    .unwrap_or_default()
            };
            let sent = json!({"method":request.method,"url":request.url,"cookie":header("Cookie"),"accept":header("Accept"),"origin":header("Origin"),"referer":header("Referer"),"user_agent":header("User-Agent")});
            let n = {
                let mut observation = h.observation.lock().unwrap();
                observation.requests.push(sent);
                let n = observation.requests.len();
                observation.trace.push(format!("request/{n}"));
                n
            };
            let scenario = h.expected["scenario"].as_str().unwrap();
            if scenario == "cancel_reuse" && n == 1 {
                h.started.add_permits(1);
                let error = request.context.cancelled().await;
                h.trace("request.canceled/1");
                return Err(Box::new(error) as ExternalError);
            }
            if scenario == "independent_accounts" {
                h.started.add_permits(1);
                let release = if header("Cookie").contains("secret-9") {
                    &h.release_second
                } else {
                    &h.release_first
                };
                tokio::select! { _ = release.acquire() => {}, error = request.context.cancelled() => return Err(Box::new(error) as ExternalError) }
            }
            if scenario == "eof_pending" && n == 1 {
                h.started.add_permits(1);
                tokio::select! { _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => h.trace("owned.pending.request.released/1"), error = request.context.cancelled() => {h.trace("request.canceled/1"); return Err(Box::new(error) as ExternalError);} }
            }
            let response = h.expected["responses"]
                .as_array()
                .unwrap()
                .iter()
                .find(|response| response["request"].as_u64() == Some(n as u64))
                .cloned();
            let Some(response) = response else {
                h.trace(format!("transport.error/{n}"));
                return Err(Box::new(std::io::Error::other(
                    "fixture-session-secret unsafe transport",
                )) as ExternalError);
            };
            h.observation
                .lock()
                .unwrap()
                .responses
                .push(response.clone());
            let media = response["content_type"] == "image/png";
            let mut headers = Headers::new();
            if scenario != "media_missing_headers" || !media {
                headers.insert(
                    "Content-Type".into(),
                    vec![response["content_type"].as_str().unwrap().into()],
                );
            }
            if media {
                if scenario != "media_missing_headers" {
                    headers.insert("Content-Length".into(), vec!["4".into()]);
                }
                headers.insert("Set-Cookie".into(), vec!["fixture-session-secret".into()]);
                headers.insert("X-Unsafe".into(), vec!["fixture-signed-secret".into()]);
            }
            Ok(Some(RawResponse {
                status: response["status"].as_u64().unwrap() as u16,
                headers,
                content_length: 0,
                body: Some(Box::new(Body {
                    h: h.clone(),
                    n,
                    bytes: response["body"].as_str().unwrap().as_bytes().to_vec(),
                    offset: 0,
                    read_failure: response["read_failure"].as_bool().unwrap(),
                    close_failure: response["close_failure"].as_bool().unwrap(),
                })),
            }))
        })
    }
}
struct Body {
    h: Arc<Harness>,
    n: usize,
    bytes: Vec<u8>,
    offset: usize,
    read_failure: bool,
    close_failure: bool,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            if self.read_failure {
                self.h.trace(format!("body.read/{}/0/failure", self.n));
                return RawRead {
                    count: 0,
                    eof: false,
                    error: Some(Box::new(std::io::Error::other(
                        "fixture-signed-secret unsafe read",
                    ))),
                };
            }
            let count = output.len().min(self.bytes.len() - self.offset);
            output[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
            self.offset += count;
            let eof = count == 0;
            self.h.trace(format!(
                "body.read/{}/{count}/{}",
                self.n,
                if eof { "EOF" } else { "nil" }
            ));
            RawRead {
                count,
                eof,
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            self.h.trace(format!("body.close/{}", self.n));
            if self.close_failure {
                Err(
                    Box::new(std::io::Error::other("fixture-signed-secret unsafe close"))
                        as ExternalError,
                )
            } else {
                Ok(())
            }
        })
    }
}

use pixiv_app::{
    config::Store,
    database::Database,
    fanbox_account::{Account as SavedAccount, DefaultStore, Repository},
    fanbox_account_service::AccountService,
    fanbox_facade::{Facade, OpenRequest},
    scheduler::SchedulerError,
};
use pixiv_mcp::fanbox::{SdkPorts, Server};
use pixiv_sdk::fanbox::{Client, Options};
use std::collections::BTreeMap;

struct TracedRepository {
    database: Arc<Mutex<Database>>,
    harness: Arc<Harness>,
}
impl Repository for TracedRepository {
    fn list(&self, context: &Context) -> Result<Vec<SavedAccount>, SchedulerError> {
        self.harness.trace("repository.list");
        self.database.list(context)
    }
    fn get(&self, context: &Context, id: i64) -> Result<SavedAccount, SchedulerError> {
        self.harness.trace(format!("repository.get/{id}"));
        self.database.get(context, id)
    }
}
struct TracedDefaults {
    store: Arc<Store>,
    harness: Arc<Harness>,
}
impl DefaultStore for TracedDefaults {
    fn read(&self) -> Result<Option<i64>, SchedulerError> {
        self.harness.trace("defaults.read");
        self.store.read_fanbox_default_user_id().map_err(Into::into)
    }
}

pub struct Session {
    pub harness: Arc<Harness>,
    pub server: Server,
    pub facade: Arc<Facade>,
    pub database: Arc<Mutex<Database>>,
    pub config: std::path::PathBuf,
    pub _directory: tempfile::TempDir,
}
impl Session {
    pub fn new(expected: Value) -> Self {
        let harness = Harness::new(expected);
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(directory.path().join("auth")).unwrap(),
        ));
        let config = directory.path().join("config.toml");
        std::fs::write(&config, harness.expected["config"].as_str().unwrap()).unwrap();
        if harness.expected["scenario"] != "no_accounts" {
            let connection = rusqlite::Connection::open(database.lock().unwrap().path()).unwrap();
            for (id, sort) in [(7, 1), (9, 2)] {
                connection.execute("INSERT INTO fanbox_account(user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at) VALUES(?1,?2,?3,'',?4,1,30,11,22)",rusqlite::params![id,sort,format!("fixture-{id}"),format!("fixture-session-secret-{id}").as_bytes()]).unwrap();
            }
        }
        let store = Arc::new(Store::new(config.clone()));
        let mut service = AccountService::new(
            Some(Arc::new(TracedRepository {
                database: database.clone(),
                harness: harness.clone(),
            })),
            Some(Arc::new(TracedDefaults {
                store,
                harness: harness.clone(),
            })),
        );
        let h = harness.clone();
        service.load_options = Some(Arc::new(move || {
            h.trace("options.load");
            if h.expected["scenario"] == "options_error" {
                return Err(SchedulerError::Message("owned options failure".into()));
            }
            Ok(Options {
                http_client: Some(Arc::new(Transport(h.clone()))),
                user_agent: "fixture-agent".into(),
                proxy_url: if h.expected["scenario"] == "invalid_proxy" {
                    "http://fixture-user:fixture-password-secret@127.0.0.1:9".into()
                } else {
                    String::new()
                },
                ..Default::default()
            })
        }));
        let service = Arc::new(service);
        let clients = Arc::new(Mutex::new(BTreeMap::<usize, usize>::new()));
        let h = harness.clone();
        let c = clients.clone();
        let facade = Arc::new(Facade::with_close_client(
            Some(service.clone()),
            Some(Arc::new(move |client: &Client| {
                let id = c.lock().unwrap()[&(std::ptr::from_ref(client) as usize)];
                {
                    let mut observation = h.observation.lock().unwrap();
                    observation.live -= 1;
                    observation.lease_closes += 1;
                    observation.trace.push(format!("lease.close/{id}"));
                }
                client.close_idle_connections();
                if h.expected["scenario"]
                    .as_str()
                    .unwrap()
                    .contains("lease_close_failure")
                {
                    Err(SchedulerError::Message(
                        "fixture lease close secret failure".into(),
                    ))
                } else {
                    Ok(())
                }
            })),
        ));
        let exposed_facade = facade.clone();
        let h = harness.clone();
        let port = harness.expected["port"].as_str().unwrap().to_owned();
        let ports = match port.as_str() {
            "none" => SdkPorts::default(),
            "raw" | "raw_nil" => SdkPorts {
                open: Some(Arc::new(
                    move |context: Context, account: pixiv_mcp::fanbox::Account| {
                        let h = h.clone();
                        let service = service.clone();
                        let port = port.clone();
                        Box::pin(async move {
                            if port == "raw_nil" {
                                h.trace("raw.open/nil");
                                return Ok(None);
                            }
                            h.trace(format!(
                                "raw.open/proxy={}",
                                account.https_proxy_override.as_deref().unwrap_or("nil")
                            ));
                            let opened = service.open_client_with_proxy(
                                &context,
                                account.https_proxy_override.as_deref(),
                            );
                            match opened.error {
                                Some(error) => Err(error),
                                None => Ok(opened.client),
                            }
                        })
                    },
                )),
                ..Default::default()
            },
            _ => SdkPorts {
                open_lease: Some(Arc::new(
                    move |context: Context, account: pixiv_mcp::fanbox::Account| {
                        let h = h.clone();
                        let facade = facade.clone();
                        let clients = clients.clone();
                        let port = port.clone();
                        Box::pin(async move {
                            h.trace(format!(
                                "port.open/proxy={}",
                                account.https_proxy_override.as_deref().unwrap_or("nil")
                            ));
                            if port == "open_error" {
                                return Err(SchedulerError::Message("owned opener failure".into()));
                            }
                            if port == "nil_lease" {
                                return Ok(None);
                            }
                            let lease = facade
                                .open(
                                    Some(&context),
                                    OpenRequest {
                                        proxy_override: account.https_proxy_override,
                                    },
                                )
                                .await?;
                            let id = {
                                let mut observation = h.observation.lock().unwrap();
                                observation.lease_opens += 1;
                                observation.live += 1;
                                observation.max_live_leases =
                                    observation.max_live_leases.max(observation.live);
                                let id = observation.lease_opens;
                                observation.trace.push(format!("lease.open/{id}"));
                                id
                            };
                            clients
                                .lock()
                                .unwrap()
                                .insert(Arc::as_ptr(lease.value()) as usize, id);
                            Ok(Some(lease))
                        })
                    },
                )),
                ..Default::default()
            },
        };
        let proxy = harness.expected["proxy"].as_str().map(str::to_owned);
        Self {
            server: Server::new(ports, proxy),
            facade: exposed_facade,
            harness,
            database,
            config,
            _directory: directory,
        }
    }
    pub fn select_nine(&self) {
        std::fs::write(&self.config, "[fanbox.auth]\ndefault_user_id = 9\n").unwrap();
        self.harness.trace("owned.config.select/9");
    }
    pub async fn call(&self, context: &Context, expected: &Value) -> Value {
        let (result, error) = match self
            .server
            .call(
                context,
                expected["tool"].as_str().unwrap(),
                &expected["arguments"],
            )
            .await
        {
            Ok(result) => (result, String::new()),
            Err(error) => (Value::Null, format!("calling \"tools/call\": {error}")),
        };
        json!({"tool":expected["tool"],"arguments":expected["arguments"],"result":result,"error":error})
    }
    pub async fn call_client(self: &Arc<Self>, context: &Context, expected: &Value) -> Value {
        let session = self.clone();
        let handler_context = context.clone();
        let call = expected.clone();
        let mut handler = tokio::spawn(async move { session.call(&handler_context, &call).await });
        tokio::select! {
            biased;
            error = context.cancelled() => {
                let drained = handler.await.unwrap();
                assert_eq!(drained["result"]["isError"], true);
                json!({"tool":expected["tool"],"arguments":expected["arguments"],"result":null,"error":error.to_string()})
            }
            result = &mut handler => result.unwrap(),
        }
    }
    pub fn finish(&self) -> Value {
        let observation = self.harness.observation.lock().unwrap();
        assert_eq!(
            observation.live, 0,
            "{} leaked leases",
            self.harness.expected["name"]
        );
        let database=self.database.lock().unwrap().list_fanbox().unwrap().iter().map(|a|json!({"user_id":a.user_id,"sort_order":a.sort_order,"session":String::from_utf8(a.session_id_copy()).unwrap(),"credential_revision":a.credential_revision,"validated_at":a.validated_at,"created_at":a.created_at,"updated_at":a.updated_at})).collect::<Vec<_>>();
        json!({"name":self.harness.expected["name"],"scenario":self.harness.expected["scenario"],"port":self.harness.expected["port"],"config":self.harness.expected["config"],"proxy":self.harness.expected["proxy"],"calls":observation.calls,"requests":observation.requests,"responses":observation.responses,"trace":observation.trace,"diagnostics":observation.diagnostics,"database":database,"config_after":std::fs::read_to_string(&self.config).unwrap(),"lease_opens":observation.lease_opens,"lease_closes":observation.lease_closes,"max_live_leases":observation.max_live_leases})
    }
}

pub fn assert_observation(actual: &Value, expected: &Value, name: &str) {
    if actual == expected {
        return;
    }
    fn mismatch(actual: &Value, expected: &Value, path: &str) -> Option<String> {
        if actual == expected {
            return None;
        }
        match (actual, expected) {
            (Value::Object(a), Value::Object(e)) => {
                for key in a.keys().chain(e.keys()) {
                    match (a.get(key), e.get(key)) {
                        (Some(a), Some(e)) => {
                            if let Some(difference) = mismatch(a, e, &format!("{path}/{key}")) {
                                return Some(difference);
                            }
                        }
                        (a, e) => return Some(format!("{path}/{key}: actual {a:?}; frozen {e:?}")),
                    }
                }
            }
            (Value::Array(a), Value::Array(e)) => {
                if a.len() != e.len() {
                    return Some(format!(
                        "{path}: actual length {}; frozen length {}",
                        a.len(),
                        e.len()
                    ));
                }
                for (index, (a, e)) in a.iter().zip(e).enumerate() {
                    if let Some(difference) = mismatch(a, e, &format!("{path}/{index}")) {
                        return Some(difference);
                    }
                }
            }
            _ => return Some(format!("{path}: actual {actual}; frozen {expected}")),
        }
        Some(format!("{path}: values differ"))
    }
    let directory = std::env::temp_dir().join("pixiv-fanbox-mcp-observations");
    std::fs::create_dir_all(&directory).unwrap();
    let artifact = directory.join(format!("{name}.json"));
    std::fs::write(&artifact, serde_json::to_vec_pretty(actual).unwrap()).unwrap();
    panic!(
        "{name}: {}; actual observation {}",
        mismatch(actual, expected, "").unwrap(),
        artifact.display()
    );
}

use std::{
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context as TaskContext, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct FailingInput<R> {
    pub inner: R,
    pub fail: Arc<AtomicBool>,
    pub drops: Arc<AtomicUsize>,
}
impl<R: AsyncRead + Unpin> AsyncRead for FailingInput<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.fail.load(Ordering::SeqCst) {
            return Poll::Ready(Err(std::io::Error::other("owned read failure")));
        }
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}
impl<R> Drop for FailingInput<R> {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
pub struct FailingOutput<W> {
    pub inner: W,
    pub fail: Arc<AtomicBool>,
    pub harness: Arc<Harness>,
}
impl<W: AsyncWrite + Unpin> AsyncWrite for FailingOutput<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.fail.swap(false, Ordering::SeqCst) {
            self.harness.trace("owned.write.failure");
            return Poll::Ready(Err(std::io::Error::other("owned write failure")));
        }
        Pin::new(&mut self.inner).poll_write(context, bytes)
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}
