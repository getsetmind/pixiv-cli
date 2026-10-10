use pixiv_app::{
    config::Store,
    database::Database,
    fanbox_account::{Account, DefaultStore, Repository},
    fanbox_account_service::AccountService,
    fanbox_facade::{Facade, OpenRequest, UseCallback},
    lifecycle::{Context, ContextError},
    scheduler::SchedulerError,
    sessions::ClientOpen,
};
use pixiv_sdk::fanbox::{
    Client, Options, SessionCredentials,
    transport::{ExternalError, RawRequest, RawResponse, RawTransport, TransportFuture},
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

fn fixture() -> Value {
    let value: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-saved-accounts.json")).unwrap();
    assert_eq!(
        value["source_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(value["repository"]["cases"].as_array().unwrap().len(), 75);
    assert_eq!(value["service"]["cases"].as_array().unwrap().len(), 162);
    assert_eq!(value["leases"]["cases"].as_array().unwrap().len(), 84);
    value
}
fn row<'a>(fixture: &'a Value, section: &str, name: &str) -> &'a Value {
    fixture[section]["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name || row["id"] == name)
        .unwrap()
}
fn text(error: Option<&SchedulerError>) -> String {
    error.map(ToString::to_string).unwrap_or_default()
}
fn assert_service_error(error: Option<&SchedulerError>, expected: &Value, name: &str) {
    assert_eq!(text(error), expected["text"], "{name}");
    assert_eq!(
        error.is_some_and(SchedulerError::is_canceled),
        expected["canceled"].as_bool().unwrap(),
        "{name}"
    );
    assert_eq!(
        error.is_some_and(SchedulerError::is_deadline_exceeded),
        expected["deadline"].as_bool().unwrap(),
        "{name}"
    );
    let classified = error.and_then(SchedulerError::classified).map(|error| json!({"product":error.product,"operation":error.operation,"reason":error.code.as_str(),"detail":error.detail.clone().unwrap_or_default()}));
    assert_eq!(json!(classified), expected["classified"], "{name}");
}
fn summary(a: &pixiv_app::fanbox_account_service::AccountSummary) -> Value {
    json!({"UserID":a.user_id,"DisplayName":a.display_name,"CreatorID":a.creator_id,"Default":a.default})
}
fn account(a: &Account) -> Value {
    json!({"user_id":a.user_id,"sort_order":a.sort_order,"display_name":a.display_name,"creator_id":a.creator_id,"session":String::from_utf8(a.session_id_copy()).unwrap(),"session_nil":false,"has_session":a.has_session(),"credential_revision":a.credential_revision,"validated_at":a.validated_at,"created_at":a.created_at,"updated_at":a.updated_at})
}
fn seed(database: &Database, rows: &Value) {
    let connection = rusqlite::Connection::open(database.path()).unwrap();
    for row in rows.as_array().unwrap() {
        connection.execute("INSERT INTO fanbox_account(user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",rusqlite::params![row["user_id"].as_i64().unwrap(),row["sort_order"].as_i64().unwrap(),row["display_name"].as_str().unwrap(),row["creator_id"].as_str().unwrap(),row["session"].as_str().unwrap().as_bytes(),row["credential_revision"].as_i64().unwrap(),row["validated_at"].as_i64().unwrap(),row["created_at"].as_i64().unwrap(),row["updated_at"].as_i64().unwrap()]).unwrap();
    }
}
#[derive(Default)]
struct Transport {
    closes: AtomicUsize,
    requests: AtomicUsize,
}
impl RawTransport for Transport {
    fn send(
        &self,
        _request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { panic!("saved-client opening must not perform a request") })
    }
    fn close_idle_connections(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}
struct TracedRepository {
    database: Arc<Mutex<Database>>,
    trace: Arc<Mutex<Vec<String>>>,
}
impl Repository for TracedRepository {
    fn list(&self, context: &Context) -> Result<Vec<Account>, SchedulerError> {
        self.trace
            .lock()
            .unwrap()
            .push("repository.list/context=caller/err=".into());
        self.database.list(context)
    }
    fn get(&self, context: &Context, id: i64) -> Result<Account, SchedulerError> {
        self.trace
            .lock()
            .unwrap()
            .push("repository.get/context=caller/err=".into());
        self.database.get(context, id)
    }
}
struct TracedDefaults {
    store: Arc<Store>,
    trace: Arc<Mutex<Vec<String>>>,
}
impl DefaultStore for TracedDefaults {
    fn read(&self) -> Result<Option<i64>, SchedulerError> {
        self.trace.lock().unwrap().push("defaults.read".into());
        self.store.read_fanbox_default_user_id().map_err(Into::into)
    }
}
#[test]
fn selected_real_sqlite_reads_match_the_sealed_go_rows() {
    let fixture = fixture();
    for name in [
        "empty list",
        "missing get",
        "stored account",
        "sort not renumbered",
        "negative get",
    ] {
        let case = row(&fixture, "repository", name);
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(dir.path()).unwrap();
        seed(&database, &case["rows"]);
        let result = if case["input"]["action"] == "list" {
            database
                .list_fanbox()
                .map(|values| Value::Array(values.iter().map(account).collect()))
        } else {
            database
                .get_fanbox(case["input"]["id"].as_i64().unwrap())
                .map(|a| account(&a))
        };
        if case["error"]["text"] == "" {
            assert_eq!(result.unwrap(), case["result"], "{name}");
        } else {
            assert_eq!(
                result.unwrap_err().to_string(),
                case["error"]["text"],
                "{name}"
            );
        }
    }
}
#[test]
fn saved_selection_list_status_and_client_open_match_actual_go_trace() {
    let fixture = fixture();
    for action in ["list", "status", "open"] {
        for selection in ["empty", "auto", "explicit", "missing", "nil_defaults"] {
            let name = format!("{action}/{selection}");
            let case = row(&fixture, "service", &name);
            let dir = tempfile::tempdir().unwrap();
            let database = Database::open(dir.path()).unwrap();
            seed(&database, &case["rows"]);
            let store = Arc::new(Store::new(dir.path().join("config.toml")));
            std::fs::write(store.path(), case["config"].as_str().unwrap()).unwrap();
            let before = std::fs::read(store.path()).unwrap();
            let database = Arc::new(Mutex::new(database));
            let trace = Arc::new(Mutex::new(Vec::<String>::new()));
            let transport = Arc::new(Transport::default());
            let mut service = AccountService::new(
                Some(Arc::new(TracedRepository {
                    database,
                    trace: trace.clone(),
                })),
                if selection == "nil_defaults" {
                    None
                } else {
                    Some(Arc::new(TracedDefaults {
                        store: store.clone(),
                        trace: trace.clone(),
                    }))
                },
            );
            let loading = trace.clone();
            let injected = transport.clone();
            service.load_options = Some(Arc::new(move || {
                loading.lock().unwrap().push("options.load".into());
                Ok(Options {
                    http_client: Some(injected.clone()),
                    ..Default::default()
                })
            }));
            let (result, error) = match action {
                "list" => match service.list_accounts(&Context::new()) {
                    Ok(v) => (Value::Array(v.iter().map(summary).collect()), None),
                    Err(e) => (Value::Null, Some(e)),
                },
                "status" => match service.status(&Context::new()) {
                    Ok(v) => (summary(&v), None),
                    Err(e) => (Value::Null, Some(e)),
                },
                _ => {
                    let opened = service.open_client_with_proxy(&Context::new(), None);
                    let present = opened.client.is_some();
                    if let Some(client) = opened.client {
                        trace.lock().unwrap().push("caller.client.close".into());
                        client.close_idle_connections();
                    }
                    (json!({"client_present":present}), opened.error)
                }
            };
            assert_eq!(result, case["result"], "{name}");
            assert_service_error(error.as_ref(), &case["error"], &name);
            assert_eq!(json!(*trace.lock().unwrap()), case["trace"], "{name}");
            assert_eq!(
                transport.closes.load(Ordering::SeqCst),
                case["idle_closes"].as_u64().unwrap() as usize,
                "{name}"
            );
            assert_eq!(transport.requests.load(Ordering::SeqCst), 0, "{name}");
            assert_eq!(std::fs::read(store.path()).unwrap(), before, "{name}");
        }
    }
}
#[tokio::test]
async fn facade_uses_one_child_attempt_and_one_owned_close_with_go_error_order() {
    let fixture = fixture();
    for name in [
        "use/success",
        "use/use_error",
        "use/close_error",
        "use/use_close_error",
        "use/precanceled_success",
        "use/committed_success",
        "use/committed_use_error",
    ] {
        let case = &row(&fixture, "leases", name)["observation"];
        let transport = Arc::new(Transport::default());
        let client = Arc::new(
            Client::open_with(
                SessionCredentials {
                    fanbox_sessid: "fixture-session".into(),
                },
                Options {
                    http_client: Some(transport.clone()),
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let retained = Arc::new(Mutex::new(None::<Context>));
        let attempts = Arc::new(Mutex::new(None));
        let mut service = AccountService::new(None, None);
        let log = events.clone();
        let held = retained.clone();
        let opened = client.clone();
        service.open_client = Some(Arc::new(move |context| {
            log.lock().unwrap().push("open".into());
            *held.lock().unwrap() = Some(context.clone());
            ClientOpen {
                client: Some(opened.clone()),
                error: None,
            }
        }));
        let log = events.clone();
        let closes = Arc::new(AtomicUsize::new(0));
        let count = closes.clone();
        let close_error = name.contains("close_error");
        let facade = Facade::with_close_client(
            Some(Arc::new(service)),
            Some(Arc::new(move |_| {
                log.lock().unwrap().push("close".into());
                count.fetch_add(1, Ordering::SeqCst);
                if close_error {
                    Err(SchedulerError::Message("synthetic close failure".into()))
                } else {
                    Ok(())
                }
            })),
        );
        let log = events.clone();
        let attempt_slot = attempts.clone();
        let use_error = name.ends_with("use_error") || name.ends_with("use_close_error");
        let commit = name.contains("committed");
        let callback: Arc<UseCallback> = Arc::new(move |_context, received, attempt| {
            assert!(Arc::ptr_eq(&received, &client));
            log.lock().unwrap().push("use".into());
            if commit {
                attempt.commit();
            }
            *attempt_slot.lock().unwrap() = Some(attempt);
            Box::pin(async move {
                if use_error {
                    Err(SchedulerError::Message("synthetic use failure".into()))
                } else {
                    Ok(())
                }
            })
        });
        let parent = Context::new();
        if name.contains("precanceled") {
            parent.cancel();
        }
        let result = facade
            .use_client(Some(&parent), OpenRequest::default(), Some(callback))
            .await;
        assert_eq!(
            text(result.as_ref().err()),
            case["error"]["message"],
            "{name}"
        );
        assert_eq!(json!(*events.lock().unwrap()), case["events"], "{name}");
        assert_eq!(closes.load(Ordering::SeqCst), 1, "{name}");
        assert_eq!(
            retained.lock().unwrap().as_ref().unwrap().error(),
            Some(ContextError::Canceled),
            "{name}"
        );
        assert_eq!(
            attempts.lock().unwrap().as_ref().unwrap().committed(),
            case["attempt"]["retained_committed"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(
            parent.error().is_some(),
            name.contains("precanceled"),
            "{name}"
        );
    }
}

struct IdentityBody {
    body: std::io::Cursor<Vec<u8>>,
    closes: Arc<AtomicUsize>,
}
impl pixiv_sdk::fanbox::transport::RawBody for IdentityBody {
    fn read<'a>(
        &'a mut self,
        output: &'a mut [u8],
    ) -> pixiv_sdk::fanbox::transport::BodyFuture<'a, pixiv_sdk::fanbox::transport::RawRead> {
        Box::pin(async move {
            let count = std::io::Read::read(&mut self.body, output).unwrap();
            pixiv_sdk::fanbox::transport::RawRead {
                count,
                eof: count == 0,
                error: None,
            }
        })
    }
    fn close(&mut self) -> pixiv_sdk::fanbox::transport::BodyFuture<'_, Result<(), ExternalError>> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}
struct IdentityTransport {
    trace: Arc<Mutex<Vec<String>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    closes: Arc<AtomicUsize>,
    bodies: Arc<AtomicUsize>,
}
impl RawTransport for IdentityTransport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        let header = |key: &str| {
            request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .and_then(|(_, values)| values.first())
                .cloned()
                .unwrap_or_default()
        };
        self.trace.lock().unwrap().push("verify.request".into());
        self.requests.lock().unwrap().push(json!({"method":request.method,"url":request.url,"cookie":header("Cookie"),"user_agent":header("User-Agent"),"accept":header("Accept"),"context":"caller","context_error":request.context.error().map(|e|e.to_string()).unwrap_or_default()}));
        let bodies = self.bodies.clone();
        Box::pin(async move {
            Ok(Some(RawResponse {status:200,headers:Default::default(),content_length:-1,body:Some(Box::new(IdentityBody {body:std::io::Cursor::new(br#"<html><head><meta name="metadata" content='{"context":{"user":{"userId":42,"name":"  fixture verified  ","creatorId":"fixture-creator"}}}'></head></html>"#.to_vec()),closes:bodies}))}))
        })
    }
    fn close_idle_connections(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}
#[tokio::test]
async fn lazy_public_options_loading_and_proxy_presence_match_go_sdk_requests() {
    let fixture = fixture();
    for suffix in [
        "loaded options",
        "override proxy only",
        "explicit empty proxy disables",
        "invalid loaded proxy",
        "override masks invalid loaded proxy",
        "invalid override",
        "invalid user agent",
        "loader error",
        "injected client bypasses loader selection and proxy",
        "injected nil success preserved",
        "injected partial error preserved",
    ] {
        let name = format!("options/{suffix}");
        let case = row(&fixture, "service", &name);
        let input = &case["input"];
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(dir.path()).unwrap();
        seed(&database, &case["rows"]);
        let store = Arc::new(Store::new(dir.path().join("config.toml")));
        std::fs::write(store.path(), "").unwrap();
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let closes = Arc::new(AtomicUsize::new(0));
        let bodies = Arc::new(AtomicUsize::new(0));
        let transport = Arc::new(IdentityTransport {
            trace: trace.clone(),
            requests: requests.clone(),
            closes: closes.clone(),
            bodies: bodies.clone(),
        });
        let mode = input["mode"].as_str().unwrap().to_owned();
        let injected = mode.starts_with("injected");
        let mut service = AccountService::new(
            if injected {
                None
            } else {
                Some(Arc::new(TracedRepository {
                    database: Arc::new(Mutex::new(database)),
                    trace: trace.clone(),
                }))
            },
            if injected {
                None
            } else {
                Some(Arc::new(TracedDefaults {
                    store,
                    trace: trace.clone(),
                }))
            },
        );
        let log = trace.clone();
        let network = transport.clone();
        let captured = input.clone();
        let loaded_mode = mode.clone();
        service.load_options = Some(Arc::new(move || {
            assert!(!injected, "injected public opener bypasses options loader");
            log.lock().unwrap().push("options.load".into());
            if loaded_mode == "loader_error" {
                return Err(SchedulerError::Message(
                    "owned fixture option loader failure".into(),
                ));
            }
            Ok(Options {
                http_client: Some(network.clone()),
                proxy_url: captured["proxy"].as_str().unwrap().into(),
                user_agent: captured["user_agent"].as_str().unwrap().into(),
                flare_solverr: captured["solver"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|url| pixiv_sdk::fanbox::FlareSolverrOptions {
                        url: url.into(),
                        proxy_url: captured["solver_proxy"].as_str().unwrap().into(),
                    }),
            })
        }));
        if injected {
            let log = trace.clone();
            let network = transport.clone();
            service.open_client = Some(Arc::new(move |_| {
                log.lock()
                    .unwrap()
                    .push("client.factory/context=caller".into());
                if mode == "injected_nil" {
                    return ClientOpen {
                        client: None,
                        error: None,
                    };
                }
                ClientOpen {
                    client: Some(Arc::new(
                        Client::open_with(
                            SessionCredentials {
                                fanbox_sessid: "fixture-injected-session".into(),
                            },
                            Options {
                                http_client: Some(network.clone()),
                                ..Default::default()
                            },
                        )
                        .unwrap(),
                    )),
                    error: (mode == "injected_partial").then(|| {
                        SchedulerError::Message("owned fixture injected client failure".into())
                    }),
                }
            }));
        }
        let proxy = input["has_override"]
            .as_bool()
            .unwrap()
            .then(|| input["override"].as_str().unwrap());
        let mut opened = service.open_client_with_proxy(&Context::new(), proxy);
        assert_eq!(
            json!({"client_present":opened.client.is_some()}),
            case["result"],
            "{name}"
        );
        if let Some(client) = opened.client {
            if opened.error.is_none() {
                opened.error = client
                    .current_user(
                        Arc::new(Context::new()),
                        pixiv_sdk::fanbox::CurrentUserRequest {},
                    )
                    .await
                    .err()
                    .map(Into::into);
            }
            trace.lock().unwrap().push("caller.client.close".into());
            client.close_idle_connections();
        }
        assert_service_error(opened.error.as_ref(), &case["error"], &name);
        assert_eq!(json!(*trace.lock().unwrap()), case["trace"], "{name}");
        assert_eq!(json!(*requests.lock().unwrap()), case["requests"], "{name}");
        assert_eq!(
            closes.load(Ordering::SeqCst),
            case["idle_closes"].as_u64().unwrap() as usize,
            "{name}"
        );
        assert_eq!(
            bodies.load(Ordering::SeqCst),
            case["body_closes"].as_u64().unwrap() as usize,
            "{name}"
        );
    }
}

type ProxyClientOpener = dyn Fn(&Context, Option<&str>) -> ClientOpen<Client> + Send + Sync;

struct OpenPort {
    open: Arc<ProxyClientOpener>,
}
impl pixiv_app::fanbox_facade::AccountOpener for OpenPort {
    fn open_client_with_proxy(&self, context: &Context, proxy: Option<&str>) -> ClientOpen<Client> {
        (self.open)(context, proxy)
    }
}
#[tokio::test]
async fn facade_open_partial_errors_nil_ports_proxy_presence_and_default_close_match_go() {
    let fixture = fixture();
    for name in [
        "open/nil_context",
        "open/nil_context_missing_facade",
        "open/missing_facade",
        "open/missing_accounts",
        "open/nil_client_success",
        "open/open_error",
        "open/partial_open_error",
        "open/partial_open_close_error",
        "open/partial_open_close_canceled",
        "open/partial_open_close_deadline",
        "open/success",
        "open/proxy_empty",
        "open/proxy_value",
        "open/precanceled_success",
        "open/expired_deadline_success",
        "open/cancel_parent_in_open",
        "open/cancel_parent_in_open_error",
        "open/default_close",
        "open/default_nil_closer",
        "open/literal_nil_closer",
        "open/default_partial_open_error",
        "open/default_nil_closer_partial_open_error",
        "open/literal_nil_closer_partial_open_error",
    ] {
        let case = &row(&fixture, "leases", name)["observation"];
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let transport = Arc::new(Transport::default());
        let client = Arc::new(
            Client::open_with(
                SessionCredentials {
                    fanbox_sessid: "fixture-session".into(),
                },
                Options {
                    http_client: Some(transport.clone()),
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        let parent = if name.contains("expired_deadline") {
            Context::with_deadline(std::time::Instant::now() - std::time::Duration::from_secs(1))
        } else {
            Context::new()
        };
        if name.contains("precanceled") {
            parent.cancel();
        }
        let log = events.clone();
        let count = calls.clone();
        let opened_client = client.clone();
        let owned_name = name.to_owned();
        let received = Arc::new(Mutex::new(None::<Option<String>>));
        let capture = received.clone();
        let opener = OpenPort {
            open: Arc::new(move |context, proxy| {
                log.lock().unwrap().push("open".into());
                count.fetch_add(1, Ordering::SeqCst);
                *capture.lock().unwrap() = Some(proxy.map(str::to_owned));
                if owned_name.contains("cancel_parent_in_open") {
                    context.cancel();
                }
                ClientOpen {
                    client: if owned_name == "open/nil_client_success"
                        || owned_name == "open/open_error"
                        || owned_name == "open/cancel_parent_in_open_error"
                    {
                        None
                    } else {
                        Some(opened_client.clone())
                    },
                    error: if owned_name.contains("partial_open")
                        || owned_name == "open/open_error"
                        || owned_name == "open/cancel_parent_in_open_error"
                    {
                        Some(SchedulerError::Message("synthetic open failure".into()))
                    } else {
                        None
                    },
                }
            }),
        };
        let missing = name.contains("missing_facade") || name == "open/missing_accounts";
        let accounts = if missing {
            None
        } else {
            Some(Arc::new(opener) as Arc<dyn pixiv_app::fanbox_facade::AccountOpener>)
        };
        let close_calls = Arc::new(AtomicUsize::new(0));
        let closing = close_calls.clone();
        let log = events.clone();
        let same = client.clone();
        let close_name = name.to_owned();
        let default = name.contains("default_") || name.contains("literal_nil_closer");
        let facade = Facade::with_close_client(
            accounts,
            if default {
                None
            } else {
                Some(Arc::new(move |received: &Client| {
                    assert!(std::ptr::eq(received, Arc::as_ptr(&same)));
                    log.lock().unwrap().push("close".into());
                    closing.fetch_add(1, Ordering::SeqCst);
                    match close_name.as_str() {
                        "open/partial_open_close_error" => {
                            Err(SchedulerError::Message("synthetic close failure".into()))
                        }
                        "open/partial_open_close_canceled" => Err(SchedulerError::Canceled),
                        "open/partial_open_close_deadline" => Err(SchedulerError::DeadlineExceeded),
                        _ => Ok(()),
                    }
                }))
            },
        );
        let proxy = if name == "open/proxy_empty" {
            Some(String::new())
        } else if name == "open/proxy_value" {
            Some(case["proxy"]["received_value"].as_str().unwrap().into())
        } else {
            None
        };
        let result = facade
            .open(
                if name.contains("nil_context") {
                    None
                } else {
                    Some(&parent)
                },
                OpenRequest {
                    proxy_override: proxy,
                },
            )
            .await;
        let error = match result {
            Ok(lease) => {
                assert!(Arc::ptr_eq(lease.value(), &client));
                let closed = lease.close();
                closed.err().map(SchedulerError::Shared)
            }
            Err(error) => Some(error),
        };
        assert_eq!(text(error.as_ref()), case["error"]["message"], "{name}");
        assert_eq!(json!(*events.lock().unwrap()), case["events"], "{name}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            case["open_calls"].as_u64().unwrap() as usize,
            "{name}"
        );
        assert_eq!(
            close_calls.load(Ordering::SeqCst),
            case["close_calls"].as_u64().unwrap() as usize,
            "{name}"
        );
        assert_eq!(
            transport.closes.load(Ordering::SeqCst),
            case["idle_close_calls"].as_u64().unwrap() as usize,
            "{name}"
        );
        if let Some(proxy) = received.lock().unwrap().as_ref() {
            assert_eq!(
                proxy.is_none(),
                case["proxy"]["received_nil"].as_bool().unwrap(),
                "{name}"
            );
            assert_eq!(
                proxy.as_deref().unwrap_or_default(),
                case["proxy"]["received_value"].as_str().unwrap(),
                "{name}"
            );
        }
    }
}
#[tokio::test]
async fn facade_use_preserves_nil_validation_and_panic_cleanup_from_go() {
    use futures_util::FutureExt;
    let fixture = fixture();
    for name in [
        "use/nil_context",
        "use/nil_context_missing_facade",
        "use/nil_context_nil_callback",
        "use/nil_callback",
        "use/nil_callback_missing_facade",
        "use/missing_facade",
        "use/missing_accounts",
        "use/use_panic",
        "use/committed_use_panic",
        "use/use_panic_close_error",
        "use/close_panic",
        "use/use_panic_close_panic",
    ] {
        let case = &row(&fixture, "leases", name)["observation"];
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let retained = Arc::new(Mutex::new(None::<Context>));
        let close_calls = Arc::new(AtomicUsize::new(0));
        let transport = Arc::new(Transport::default());
        let client = Arc::new(
            Client::open_with(
                SessionCredentials {
                    fanbox_sessid: "fixture-session".into(),
                },
                Options {
                    http_client: Some(transport),
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        let mut service = AccountService::new(None, None);
        let log = events.clone();
        let kept = retained.clone();
        service.open_client = Some(Arc::new(move |context| {
            log.lock().unwrap().push("open".into());
            *kept.lock().unwrap() = Some(context.clone());
            ClientOpen {
                client: Some(client.clone()),
                error: None,
            }
        }));
        let accounts = if name.contains("missing_facade") || name == "use/missing_accounts" {
            None
        } else {
            Some(Arc::new(service) as Arc<dyn pixiv_app::fanbox_facade::AccountOpener>)
        };
        let log = events.clone();
        let closing = close_calls.clone();
        let mode = name.to_owned();
        let facade = Facade::with_close_client(
            accounts,
            Some(Arc::new(move |_| {
                log.lock().unwrap().push("close".into());
                closing.fetch_add(1, Ordering::SeqCst);
                if mode.contains("close_panic") {
                    panic!("synthetic close panic");
                }
                if mode.contains("close_error") {
                    Err(SchedulerError::Message("synthetic close failure".into()))
                } else {
                    Ok(())
                }
            })),
        );
        let log = events.clone();
        let mode = name.to_owned();
        let callback: Arc<UseCallback> = Arc::new(move |_, _, attempt| {
            log.lock().unwrap().push("use".into());
            if mode.contains("committed") {
                attempt.commit();
            }
            let panic = mode.contains("use_panic");
            Box::pin(async move {
                if panic {
                    panic!("synthetic use panic");
                }
                Ok(())
            })
        });
        let parent = Context::new();
        let result = std::panic::AssertUnwindSafe(facade.use_client(
            if name.contains("nil_context") {
                None
            } else {
                Some(&parent)
            },
            OpenRequest::default(),
            if name.contains("nil_callback") {
                None
            } else {
                Some(callback)
            },
        ))
        .catch_unwind()
        .await;
        let (error, panic) = match result {
            Ok(result) => (result.err(), String::new()),
            Err(panic) => (
                None,
                panic
                    .downcast_ref::<&str>()
                    .copied()
                    .map(str::to_owned)
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap(),
            ),
        };
        assert_eq!(text(error.as_ref()), case["error"]["message"], "{name}");
        assert_eq!(panic, case["panic"], "{name}");
        assert_eq!(json!(*events.lock().unwrap()), case["events"], "{name}");
        assert_eq!(
            close_calls.load(Ordering::SeqCst),
            case["close_calls"].as_u64().unwrap() as usize,
            "{name}"
        );
        if let Some(child) = retained.lock().unwrap().as_ref() {
            assert_eq!(child.error(), Some(ContextError::Canceled), "{name}");
        }
        assert!(parent.error().is_none(), "{name}");
    }
}

#[test]
fn fanbox_account_copies_credentials_and_serializes_the_safe_go_identity() {
    let fixture = fixture();
    let case = row(
        &fixture,
        "service",
        "account/value/defensive_copy_formatting_and_json",
    );
    let mut session = b"fixture-secret-canary".to_vec();
    let a = Account::new(7, "fixture-name", "fixture-creator", &session);
    session.fill(b'x');
    let mut copied = a.session_id_copy();
    copied.fill(b'y');
    assert_eq!(
        String::from_utf8(a.session_id_copy()).unwrap(),
        case["result"]["session"]
    );
    assert_eq!(
        a.has_session(),
        case["result"]["has_session"].as_bool().unwrap()
    );
    assert_eq!(serde_json::to_string(&a).unwrap(), case["result"]["json"]);
    assert_eq!(a.to_string(), case["result"]["formatted"][0]);
    assert_eq!(format!("{a:?}"), case["result"]["formatted"][0]);
}
#[test]
fn production_runtime_options_preserve_service_presence_and_independent_solver() {
    let fixture = fixture();
    for suffix in [
        "global proxy",
        "service proxy",
        "service empty proxy",
        "independent solver",
        "proxy wrong type",
        "agent wrong type",
        "solver missing URL",
        "invalid default does not affect Runtime",
    ] {
        let name = format!("config/runtime/{suffix}");
        let case = row(&fixture, "service", &name);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("config.toml"));
        std::fs::write(store.path(), case["input"]["config"].as_str().unwrap()).unwrap();
        let loaded = store
            .current_with_environment([])
            .and_then(|snapshot| snapshot.runtime());
        match loaded {
            Err(error) => assert_eq!(error.to_string(), case["error"]["text"], "{name}"),
            Ok(runtime) => {
                let options = pixiv_app::fanbox_account_service::options_from_runtime(&runtime);
                let option_value = |value: &Option<String>| match value {
                    Some(value) if !value.is_empty() => json!({"present": true, "value": value}),
                    Some(_) => json!({"present": true}),
                    None => json!({"present": false}),
                };
                let network = json!({"proxy_url":option_value(&runtime.fanbox_network.proxy_url),"user_agent":option_value(&runtime.fanbox_network.user_agent)});
                let solver = runtime
                    .fanbox_flaresolverr
                    .as_ref()
                    .map(|solver| json!({"url":solver.url,"proxy_url":solver.proxy_url}));
                assert_eq!(
                    json!({"global_proxy":runtime.https_proxy,"fanbox_network":network,"fanbox_solver":solver}),
                    case["result"],
                    "{name}"
                );
                assert_eq!(
                    options.proxy_url,
                    runtime
                        .fanbox_network
                        .proxy_url
                        .as_ref()
                        .unwrap_or(&runtime.https_proxy)
                        .clone(),
                    "{name}"
                );
                assert_eq!(
                    options
                        .flare_solverr
                        .map(|solver| json!({"url":solver.url,"proxy_url":solver.proxy_url})),
                    solver,
                    "{name}"
                );
            }
        }
    }
}

#[tokio::test]
async fn actual_facade_leases_cache_concurrent_close_errors_and_panics_once() {
    let fixture = fixture();
    for name in [
        "lease/repeated_close_success",
        "lease/repeated_close_error",
        "lease/repeated_close_panic",
        "lease/concurrent_close_success",
        "lease/concurrent_close_error",
        "lease/concurrent_close_panic",
        "lease/default_close",
    ] {
        let case = &row(&fixture, "leases", name)["observation"];
        let transport = Arc::new(Transport::default());
        let client = Arc::new(
            Client::open_with(
                SessionCredentials {
                    fanbox_sessid: "fixture-session".into(),
                },
                Options {
                    http_client: Some(transport.clone()),
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        let mut service = AccountService::new(None, None);
        service.open_client = Some(Arc::new(move |_| ClientOpen {
            client: Some(client.clone()),
            error: None,
        }));
        let count = Arc::new(AtomicUsize::new(0));
        let closes = count.clone();
        let mode = name.to_owned();
        let facade = Facade::with_close_client(
            Some(Arc::new(service)),
            if name == "lease/default_close" {
                None
            } else {
                Some(Arc::new(move |_| {
                    closes.fetch_add(1, Ordering::SeqCst);
                    if mode.ends_with("panic") {
                        panic!("synthetic close panic");
                    }
                    if mode.ends_with("error") {
                        Err(SchedulerError::Message("synthetic close failure".into()))
                    } else {
                        Ok(())
                    }
                }))
            },
        );
        let lease = Arc::new(
            facade
                .open(Some(&Context::new()), OpenRequest::default())
                .await
                .unwrap(),
        );
        let expected = case["close_results"].as_array().unwrap();
        let tries = expected.len() + usize::from(name.ends_with("panic"));
        let run = |lease: Arc<pixiv_app::lifecycle::Lease<Arc<Client>, SchedulerError>>| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lease.close()))
        };
        let outcomes = if name.contains("concurrent") {
            let start = Arc::new(std::sync::Barrier::new(tries));
            std::thread::scope(|scope| {
                (0..tries)
                    .map(|_| {
                        let start = start.clone();
                        let lease = lease.clone();
                        scope.spawn(move || {
                            start.wait();
                            run(lease)
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|handle| handle.join().unwrap())
                    .collect::<Vec<_>>()
            })
        } else {
            (0..tries).map(|_| run(lease.clone())).collect::<Vec<_>>()
        };
        let mut results = Vec::new();
        let mut panics = Vec::new();
        for outcome in outcomes {
            match outcome {
                Ok(value) => results.push(value),
                Err(panic) => {
                    panics.push(panic.downcast_ref::<&str>().copied().unwrap().to_owned())
                }
            }
        }
        assert_eq!(json!(panics), case["close_panics"], "{name}");
        assert_eq!(
            results
                .iter()
                .map(|value| value
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|value| value["message"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            "{name}"
        );
        assert!(
            results.iter().all(|value| match (&results[0], value) {
                (Err(first), Err(next)) => Arc::ptr_eq(first, next),
                (Ok(()), Ok(())) => true,
                _ => false,
            }),
            "{name}"
        );
        assert_eq!(
            count.load(Ordering::SeqCst),
            case["close_calls"].as_u64().unwrap() as usize,
            "{name}"
        );
        assert_eq!(
            transport.closes.load(Ordering::SeqCst),
            case["idle_close_calls"].as_u64().unwrap() as usize,
            "{name}"
        );
    }
}

struct ReadPorts {
    database: Arc<Mutex<Database>>,
    store: Store,
    trace: Arc<Mutex<Vec<String>>>,
    calls: Mutex<std::collections::BTreeMap<String, usize>>,
    failure: String,
    nth: usize,
}
impl ReadPorts {
    fn before(&self, operation: &str, context: Option<&Context>) -> Result<(), SchedulerError> {
        let label = if operation == "default_read" {
            "defaults.read".into()
        } else {
            format!(
                "repository.{operation}/context=caller/err={}",
                context
                    .unwrap()
                    .error()
                    .map(|error| error.to_string())
                    .unwrap_or_default()
            )
        };
        self.trace.lock().unwrap().push(label);
        let mut calls = self.calls.lock().unwrap();
        let count = calls.entry(operation.into()).or_default();
        *count += 1;
        if self.failure == operation && *count == self.nth {
            return Err(SchedulerError::Message(format!(
                "owned fixture {} failure",
                if operation == "default_read" {
                    "default read"
                } else {
                    operation
                }
            )));
        }
        Ok(())
    }
}
impl Repository for ReadPorts {
    fn list(&self, context: &Context) -> Result<Vec<Account>, SchedulerError> {
        self.before("list", Some(context))?;
        self.database.list(context)
    }
    fn get(&self, context: &Context, id: i64) -> Result<Account, SchedulerError> {
        self.before("get", Some(context))?;
        self.database.get(context, id)
    }
}
impl DefaultStore for ReadPorts {
    fn read(&self) -> Result<Option<i64>, SchedulerError> {
        self.before("default_read", None)?;
        self.store.read_fanbox_default_user_id().map_err(Into::into)
    }
}
#[test]
fn real_read_failures_preserve_go_port_order_masking_and_context_causes() {
    let fixture = fixture();
    for name in [
        "list/failure/list/1",
        "list/failure/list/2",
        "list/failure/list/3",
        "list/failure/default_read/1",
        "list/failure/default_read/2",
        "status/failure/list/1",
        "status/failure/get/1",
        "status/failure/default_read/1",
        "open/failure/list/1",
        "open/failure/get/1",
        "open/failure/default_read/1",
        "status/explicit_get_failure/1",
        "status/explicit_get_failure/2",
        "open/explicit_get_failure/1",
        "open/explicit_get_failure/2",
        "list/context/canceled",
        "list/context/deadline",
        "status/context/canceled",
        "status/context/deadline",
        "open/context/canceled",
        "open/context/deadline",
    ] {
        let case = row(&fixture, "service", name);
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(dir.path()).unwrap();
        seed(&database, &case["rows"]);
        let store = Store::new(dir.path().join("config.toml"));
        std::fs::write(store.path(), case["config"].as_str().unwrap()).unwrap();
        let before = std::fs::read(store.path()).unwrap();
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let explicit = name.contains("explicit_get_failure");
        let ports = Arc::new(ReadPorts {
            database: Arc::new(Mutex::new(database)),
            store,
            trace: trace.clone(),
            calls: Mutex::new(Default::default()),
            failure: if explicit {
                "get".into()
            } else {
                case["input"]["failure"].as_str().unwrap_or_default().into()
            },
            nth: if explicit {
                name.rsplit('/').next().unwrap().parse().unwrap()
            } else {
                case["input"]["nth"].as_u64().unwrap_or(0) as usize
            },
        });
        let mut service = AccountService::new(Some(ports.clone()), Some(ports.clone()));
        service.load_options = Some(Arc::new(|| {
            panic!("failed account reads must not load network options")
        }));
        let context = if name.ends_with("deadline") {
            Context::with_deadline(std::time::Instant::now() - std::time::Duration::from_secs(1))
        } else {
            Context::new()
        };
        if name.ends_with("canceled") {
            context.cancel();
        }
        let error = if name.starts_with("list/") {
            service.list_accounts(&context).unwrap_err()
        } else if name.starts_with("status/") {
            service.status(&context).unwrap_err()
        } else {
            service
                .open_client_with_proxy(&context, None)
                .error
                .unwrap()
        };
        assert_eq!(error.to_string(), case["error"]["text"], "{name}");
        assert_eq!(
            error.is_canceled(),
            case["error"]["canceled"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(
            error.is_deadline_exceeded(),
            case["error"]["deadline"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(json!(*trace.lock().unwrap()), case["trace"], "{name}");
        assert_eq!(std::fs::read(ports.store.path()).unwrap(), before, "{name}");
        let actual = ports
            .database
            .lock()
            .unwrap()
            .list_fanbox()
            .unwrap()
            .iter()
            .map(|a| {
                let mut value = account(a);
                value.as_object_mut().unwrap().remove("session_nil");
                value.as_object_mut().unwrap().remove("has_session");
                value
            })
            .collect::<Vec<_>>();
        assert_eq!(json!(actual), case["rows"], "{name}");
    }
}

#[test]
fn production_saved_service_opens_real_selected_session_without_pixiv_refresh_or_pool_fallback() {
    let fixture = fixture();
    for suffix in ["empty", "auto", "explicit", "missing"] {
        let name = format!("open/{suffix}");
        let case = row(&fixture, "service", &name);
        let dir = tempfile::tempdir().unwrap();
        let mut database = Database::open(dir.path()).unwrap();
        seed(&database, &case["rows"]);
        database
            .save_pixiv_credential(&pixiv_app::database::PixivAccount::new(
                77,
                "unrelated-pixiv",
                b"unrelated-refresh-canary",
            ))
            .unwrap();
        let pixiv_before = database.get_pixiv(77).unwrap().refresh_token_copy();
        let database = Arc::new(Mutex::new(database));
        let store = Arc::new(Store::new(dir.path().join("config.toml")));
        std::fs::write(
            store.path(),
            format!(
                "{}\n[account_pool]\nenabled=true\n",
                case["config"].as_str().unwrap()
            ),
        )
        .unwrap();
        let before = std::fs::read(store.path()).unwrap();
        assert!(
            store
                .current_with_environment([])
                .unwrap()
                .runtime()
                .unwrap()
                .account_pool
                .enabled
        );
        let mut service = AccountService::from_store(database.clone(), store.clone());
        let loader = service.load_options.take().unwrap();
        let loaded = Arc::new(AtomicUsize::new(0));
        let count = loaded.clone();
        let transport = Arc::new(Transport::default());
        let injected = transport.clone();
        service.load_options = Some(Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            let mut options = loader()?;
            options.http_client = Some(injected.clone());
            Ok(options)
        }));
        let opened = service.open_client_with_proxy(&Context::new(), Some(""));
        assert_eq!(
            json!({"client_present":opened.client.is_some()}),
            case["result"],
            "{name}"
        );
        assert_service_error(opened.error.as_ref(), &case["error"], &name);
        let present = opened.client.is_some();
        if let Some(client) = opened.client {
            client.close_idle_connections();
        }
        assert_eq!(
            loaded.load(Ordering::SeqCst),
            usize::from(present),
            "{name}"
        );
        assert_eq!(transport.requests.load(Ordering::SeqCst), 0, "{name}");
        assert_eq!(
            database
                .lock()
                .unwrap()
                .get_pixiv(77)
                .unwrap()
                .refresh_token_copy(),
            pixiv_before,
            "{name}"
        );
        assert_eq!(std::fs::read(store.path()).unwrap(), before, "{name}");
    }
}
