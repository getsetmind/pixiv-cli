use pixiv_app::{
    config::{ConfigError, ConfigFiles, Store, SystemConfigFiles},
    database::Database,
    fanbox_account::{Account, DefaultStore, Repository},
    fanbox_account_service::{AccountService, AccountSummary},
    lifecycle::Context,
    scheduler::SchedulerError,
    sessions::ClientOpen,
};
use pixiv_sdk::{
    context::ContextKey,
    fanbox::{
        Client, Options, SessionCredentials,
        transport::{
            BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
            TransportFuture,
        },
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-saved-accounts.json")).unwrap();
    assert_eq!(
        fixture["source_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["service"]["cases"].as_array().unwrap().len(), 162);
    fixture
}
fn rows(fixture: &Value) -> &[Value] {
    fixture["service"]["cases"].as_array().unwrap()
}
fn row<'a>(fixture: &'a Value, name: &str) -> &'a Value {
    rows(fixture)
        .iter()
        .find(|row| row["name"] == name)
        .unwrap()
}
fn message(value: impl Into<String>) -> SchedulerError {
    SchedulerError::Message(value.into())
}
fn assert_error(error: Option<&SchedulerError>, expected: &Value, name: &str) {
    assert_eq!(
        error.map(ToString::to_string).unwrap_or_default(),
        expected["text"],
        "{name}"
    );
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
    let not_found = error.is_some_and(|error| {
        let mut current: &(dyn std::error::Error + 'static) = error;
        loop {
            if current
                .downcast_ref::<pixiv_app::database::FanboxAccountError>()
                .is_some_and(|error| {
                    matches!(error, pixiv_app::database::FanboxAccountError::NotFound)
                })
            {
                return true;
            }
            if let Some(source) = current.source() {
                current = source;
            } else {
                return false;
            }
        }
    });
    assert_eq!(
        not_found,
        expected["not_found"].as_bool().unwrap(),
        "{name}"
    );
}
fn summary(value: &AccountSummary) -> Value {
    json!({"UserID":value.user_id,"DisplayName":value.display_name,"CreatorID":value.creator_id,"Default":value.default})
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
fn seed(database: &Database) {
    let connection = rusqlite::Connection::open(database.path()).unwrap();
    for (order, id) in [(1, 7), (2, 9)] {
        connection.execute("INSERT INTO fanbox_account(user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at) VALUES(?1,?2,?3,'',?4,1,30,11,22)", rusqlite::params![id,order,format!("fixture-{id}"),format!("fixture-session-{id}").as_bytes()]).unwrap();
    }
}
fn snapshot(database: &Mutex<Database>, lower: i64, upper: i64) -> Value {
    let stamp = |value: i64| {
        if value > 100 {
            assert!(
                value >= lower && value <= upper,
                "timestamp {value} outside operation [{lower},{upper}]"
            );
            json!("current-time")
        } else {
            json!(value)
        }
    };
    Value::Array(database.lock().unwrap().list_fanbox().unwrap().iter().map(|account| json!({"user_id":account.user_id,"sort_order":account.sort_order,"display_name":account.display_name,"creator_id":account.creator_id,"session":String::from_utf8(account.session_id_copy()).unwrap(),"credential_revision":account.credential_revision,"validated_at":stamp(account.validated_at),"created_at":stamp(account.created_at),"updated_at":stamp(account.updated_at)})).collect())
}
fn caller_key() -> ContextKey {
    ContextKey::new("fanbox-owned-service-caller")
}
fn context(mode: &str) -> Context {
    let context = if mode == "deadline" {
        Context::with_deadline(Instant::now() - Duration::from_secs(1))
    } else {
        Context::new()
    }
    .with_value(caller_key(), Arc::new(String::from("caller")));
    if mode == "canceled" || mode == "cancel_before" {
        context.cancel();
    }
    context
}
struct Ports {
    database: Arc<Mutex<Database>>,
    store: Arc<Store>,
    trace: Arc<Mutex<Vec<String>>>,
    calls: Mutex<BTreeMap<String, usize>>,
    failure: String,
    nth: usize,
    no_set: bool,
}
impl Ports {
    fn before(&self, operation: &str, context: Option<&Context>) -> Result<(), SchedulerError> {
        if let Some(context) = context {
            let marker = context.value::<String>(&caller_key()).unwrap();
            self.trace.lock().unwrap().push(format!(
                "repository.{operation}/context={marker}/err={}",
                context
                    .error()
                    .map(|error| error.to_string())
                    .unwrap_or_default()
            ));
        }
        let mut calls = self.calls.lock().unwrap();
        let count = calls.entry(operation.into()).or_default();
        *count += 1;
        if self.failure == operation && (self.nth == 0 || self.nth == *count) {
            return Err(message(format!(
                "owned fixture {} failure",
                operation.replace('_', " ")
            )));
        }
        Ok(())
    }
}
impl Repository for Ports {
    fn rotate_session(
        &self,
        _context: &Context,
        _user_id: i64,
        _expected_revision: i64,
        _session: &[u8],
        _validated_at: i64,
    ) -> Result<(), SchedulerError> {
        panic!("authentication management must not rotate an existing session")
    }
    fn list(&self, context: &Context) -> Result<Vec<Account>, SchedulerError> {
        self.before("list", Some(context))?;
        self.database.list(context)
    }
    fn get(&self, context: &Context, id: i64) -> Result<Account, SchedulerError> {
        self.before("get", Some(context))?;
        self.database.get(context, id)
    }
    fn save_credential(&self, context: &Context, account: &Account) -> Result<(), SchedulerError> {
        self.before("save", Some(context))?;
        self.database.save_credential(context, account)
    }
    fn remove(&self, context: &Context, id: i64) -> Result<(), SchedulerError> {
        self.before("remove", Some(context))?;
        self.database.remove(context, id)
    }
}
impl DefaultStore for Ports {
    fn read(&self) -> Result<Option<i64>, SchedulerError> {
        self.trace.lock().unwrap().push("defaults.read".into());
        self.before("default_read", None)?;
        self.store.read_fanbox_default_user_id().map_err(Into::into)
    }
    fn set(&self, id: i64) -> Result<(), SchedulerError> {
        self.trace
            .lock()
            .unwrap()
            .push(format!("defaults.set/{id}"));
        self.before("default_set", None)?;
        if self.no_set {
            return Ok(());
        }
        self.store
            .set_fanbox_default_user_id(id)
            .map_err(Into::into)
    }
    fn clear(&self) -> Result<(), SchedulerError> {
        self.trace.lock().unwrap().push("defaults.clear".into());
        self.before("default_clear", None)?;
        self.store
            .clear_fanbox_default_user_id()
            .map_err(Into::into)
    }
}
struct Harness {
    _directory: tempfile::TempDir,
    ports: Arc<Ports>,
    service: AccountService,
    lower: i64,
}
impl Harness {
    fn new(config: &str, seeded: bool, failure: &str, nth: usize, no_set: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path()).unwrap();
        if seeded {
            seed(&database);
        }
        let store = Arc::new(Store::new(directory.path().join("config.toml")));
        std::fs::write(store.path(), config).unwrap();
        let ports = Arc::new(Ports {
            database: Arc::new(Mutex::new(database)),
            store,
            trace: Arc::new(Mutex::new(Vec::new())),
            calls: Mutex::new(BTreeMap::new()),
            failure: failure.into(),
            nth,
            no_set,
        });
        let service = AccountService::new(Some(ports.clone()), Some(ports.clone()));
        Self {
            _directory: directory,
            ports,
            service,
            lower: now(),
        }
    }
    fn assert_state(&self, case: &Value) {
        let name = case["name"].as_str().unwrap();
        assert_eq!(
            snapshot(&self.ports.database, self.lower, now()),
            case["rows"],
            "{name}"
        );
        assert_eq!(
            std::fs::read_to_string(self.ports.store.path()).unwrap(),
            case["config"],
            "{name}"
        );
        assert_eq!(
            json!(*self.ports.trace.lock().unwrap()),
            case["trace"],
            "{name}"
        );
    }
}
struct Body {
    data: std::io::Cursor<Vec<u8>>,
    mode: String,
    closes: Arc<AtomicUsize>,
    trace: Arc<Mutex<Vec<String>>>,
    close_trace: Arc<Mutex<Vec<Vec<String>>>>,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            if self.mode == "read_failure" {
                return RawRead {
                    count: 0,
                    eof: false,
                    error: Some(
                        std::io::Error::other("owned fixture identity read failure").into(),
                    ),
                };
            }
            let count = self.data.read(output).unwrap();
            RawRead {
                count,
                eof: count == 0,
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.close_trace
            .lock()
            .unwrap()
            .push(self.trace.lock().unwrap().clone());
        Box::pin(async move {
            if self.mode == "close_failure" {
                Err(std::io::Error::other("owned fixture identity close failure").into())
            } else {
                Ok(())
            }
        })
    }
}
struct Transport {
    mode: String,
    trace: Arc<Mutex<Vec<String>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    idle_closes: AtomicUsize,
    body_closes: Arc<AtomicUsize>,
    body_close_trace: Arc<Mutex<Vec<Vec<String>>>>,
    idle_close_trace: Mutex<Vec<Vec<String>>>,
    cancel: Option<Context>,
}
impl Transport {
    fn new(mode: &str, trace: Arc<Mutex<Vec<String>>>, context: &Context) -> Arc<Self> {
        Arc::new(Self {
            mode: mode.into(),
            trace,
            requests: Arc::new(Mutex::new(Vec::new())),
            idle_closes: AtomicUsize::new(0),
            body_closes: Arc::new(AtomicUsize::new(0)),
            body_close_trace: Arc::new(Mutex::new(Vec::new())),
            idle_close_trace: Mutex::new(Vec::new()),
            cancel: (mode == "cancel_during").then(|| context.clone()),
        })
    }
    fn assert_observations(&self, case: &Value) {
        let name = case["name"].as_str().unwrap();
        assert_eq!(
            json!(*self.requests.lock().unwrap()),
            case["requests"],
            "{name}"
        );
        assert_eq!(
            self.idle_closes.load(Ordering::SeqCst),
            case["idle_closes"].as_u64().unwrap() as usize,
            "{name}"
        );
        assert_eq!(
            self.body_closes.load(Ordering::SeqCst),
            case["body_closes"].as_u64().unwrap() as usize,
            "{name}"
        );
        for trace in self.idle_close_trace.lock().unwrap().iter() {
            assert_eq!(
                json!(trace),
                case["trace"],
                "client closes after the final service operation: {name}"
            );
        }
        for trace in self.body_close_trace.lock().unwrap().iter() {
            assert_eq!(
                trace.last().unwrap(),
                "verify.request",
                "identity body closes before account save: {name}"
            );
        }
    }
}
impl RawTransport for Transport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        let header = |key: &str| {
            request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .and_then(|(_, value)| value.first())
                .cloned()
                .unwrap_or_default()
        };
        let marker = request
            .context
            .value(&caller_key())
            .unwrap()
            .downcast::<String>()
            .unwrap();
        self.trace.lock().unwrap().push("verify.request".into());
        self.requests.lock().unwrap().push(json!({"method":request.method,"url":request.url,"cookie":header("Cookie"),"user_agent":header("User-Agent"),"accept":header("Accept"),"context":*marker,"context_error":request.context.error().map(|error| error.to_string()).unwrap_or_default()}));
        if let Some(context) = &self.cancel {
            context.cancel();
        }
        Box::pin(async move {
            if self.mode == "transport_error" {
                return Err(std::io::Error::other("owned fixture transport failure").into());
            }
            if let Some(error) = request.context.error() {
                return Err(error.into());
            }
            let identity = match self.mode.as_str() {
                "zero_identity" => r#"{"context":{"user":{"userId":0,"name":"fixture"}}}"#,
                "malformed" => "{",
                _ => {
                    r#"{"context":{"user":{"userId":42,"name":"  fixture verified  ","creatorId":"fixture-creator"}}}"#
                }
            };
            Ok(Some(RawResponse {status:match self.mode.as_str() {"unauthorized"=>401,"forbidden"=>403,_=>200},headers:Default::default(),content_length:-1,body:Some(Box::new(Body {data:std::io::Cursor::new(format!("<html><head><meta name=\"metadata\" content='{identity}'></head></html>").into_bytes()),mode:self.mode.clone(),closes:self.body_closes.clone(),trace:self.trace.clone(),close_trace:self.body_close_trace.clone()}))}))
        })
    }
    fn close_idle_connections(&self) {
        self.idle_closes.fetch_add(1, Ordering::SeqCst);
        self.idle_close_trace
            .lock()
            .unwrap()
            .push(self.trace.lock().unwrap().clone());
    }
}
fn client(value: &str, transport: Arc<Transport>) -> Arc<Client> {
    Arc::new(
        Client::open_with(
            SessionCredentials {
                fanbox_sessid: value.into(),
            },
            Options {
                http_client: Some(transport),
                ..Default::default()
            },
        )
        .unwrap(),
    )
}

#[test]
fn use_auto_and_remove_match_sealed_selection_failure_and_caller_context_state() {
    let fixture = fixture();
    let mut compared = 0;
    for case in rows(&fixture).iter().filter(|case| {
        ["use/", "auto/", "remove/"]
            .iter()
            .any(|prefix| case["name"].as_str().unwrap().starts_with(prefix))
    }) {
        let name = case["name"].as_str().unwrap();
        if !case["panic"].as_str().unwrap().is_empty() {
            continue;
        }
        let input = &case["input"];
        let selection = input["selection"].as_str().unwrap_or("");
        let mut config = if !selection.is_empty() {
            "# preserved fixture\n[unrelated]\nvalue = 'kept'\n".into()
        } else {
            String::new()
        };
        if selection == "explicit" {
            config += "[fanbox.auth]\ndefault_user_id = 9\n";
        }
        if selection == "missing" {
            config += "[fanbox.auth]\ndefault_user_id = 99\n";
        }
        let mut harness = Harness::new(
            &config,
            selection != "empty",
            input["failure"].as_str().unwrap_or(""),
            input["nth"].as_u64().unwrap_or(0) as usize,
            false,
        );
        if selection == "nil_defaults" {
            harness.service.defaults = None;
        }
        if input["port"] == "nil_repository" {
            harness.service.repository = None;
        }
        let context = context(name.rsplit('/').next().unwrap());
        let id = input["id"].as_i64().unwrap_or(7);
        let result = match name.split('/').next().unwrap() {
            "use" => harness.service.use_account(&context, id),
            "auto" => harness.service.use_auto(),
            _ => harness.service.remove_account(&context, id),
        };
        assert_error(result.as_ref().err(), &case["error"], name);
        harness.assert_state(case);
        compared += 1;
    }
    assert_eq!(compared, 29);
}

#[tokio::test]
async fn all_import_rows_keep_real_sdk_validation_durable_writes_and_source_order() {
    let fixture = fixture();
    let mut compared = 0;
    let mut gaps = Vec::new();
    for case in rows(&fixture)
        .iter()
        .filter(|case| case["name"].as_str().unwrap().starts_with("import/"))
    {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let mode = input["mode"].as_str().unwrap();
        if mode == "nil_context" {
            assert_eq!(
                case["error"]["text"],
                "fanbox:CurrentUser: upstream_error: build FANBOX request"
            );
            assert!(case["requests"].as_array().unwrap().is_empty());
            assert_eq!(case["idle_closes"], 1);
            gaps.push(name);
            continue;
        }
        let (failure, nth, no_set) = match name {
            "import/save failure" => ("save", 0, false),
            "import/default read failure after save" => ("default_read", 1, false),
            "import/default set failure after save" => ("default_set", 0, false),
            "import/final default read failure after save" => ("default_read", 2, false),
            "import/final automatic list failure after save" => ("list", 0, true),
            _ => ("", 0, false),
        };
        let mut harness = Harness::new(
            input["initial_config"].as_str().unwrap(),
            input["seed"].as_bool().unwrap(),
            failure,
            nth,
            no_set,
        );
        let context = context(mode);
        let transport = Transport::new(mode, harness.ports.trace.clone(), &context);
        let network = transport.clone();
        let trace = harness.ports.trace.clone();
        let owned_mode = mode.to_owned();
        harness.service.open_session = Some(Arc::new(move |value| {
            trace
                .lock()
                .unwrap()
                .push(format!("session.factory/{value}"));
            if owned_mode == "factory_error" {
                return ClientOpen {
                    client: None,
                    error: Some(message("owned fixture session factory failure")),
                };
            }
            if owned_mode == "nil_factory" {
                return ClientOpen {
                    client: None,
                    error: None,
                };
            }
            ClientOpen {
                client: Some(client(value, network.clone())),
                error: (owned_mode == "partial_factory_error")
                    .then(|| message("owned fixture session factory failure")),
            }
        }));
        harness.service.load_options = Some(Arc::new(|| {
            panic!("session opener must bypass options loading")
        }));
        let result = harness
            .service
            .import_session(
                &context,
                if mode == "empty" {
                    " \t\r\n "
                } else {
                    "  fixture-import-session  "
                },
                input["set_default"].as_bool().unwrap(),
            )
            .await;
        if mode == "nil_factory" {
            assert!(!case["panic"].as_str().unwrap().is_empty());
            assert_eq!(
                result.unwrap_err().to_string(),
                "fanbox session factory returned no client"
            );
            gaps.push(name);
        } else {
            assert_error(result.as_ref().err(), &case["error"], name);
            assert_eq!(
                result.as_ref().map(summary).unwrap_or_else(
                    |_| json!({"UserID":0,"DisplayName":"","CreatorID":"","Default":false})
                ),
                case["result"],
                "{name}"
            );
            compared += 1;
        }
        harness.assert_state(case);
        transport.assert_observations(case);
    }
    assert_eq!(compared, 24);
    assert_eq!(
        gaps,
        [
            "import/session factory nil success",
            "import/nil verification context"
        ]
    );
}

#[tokio::test]
async fn default_session_opening_is_lazy_and_preserves_injected_session_bypass() {
    let fixture = fixture();
    for suffix in [
        "default import loads options and passes proxy",
        "default import loader failure before verification",
        "default import invalid proxy before verification",
        "default import empty override masks invalid proxy",
        "injected session bypasses loader and proxy",
    ] {
        let name = format!("options/{suffix}");
        let case = row(&fixture, &name);
        let input = &case["input"];
        let mut harness = Harness::new("", true, "", 0, false);
        let context = context("");
        let transport = Transport::new("success", harness.ports.trace.clone(), &context);
        let trace = harness.ports.trace.clone();
        let network = transport.clone();
        let captured = input.clone();
        harness.service.load_options = Some(Arc::new(move || {
            trace.lock().unwrap().push("options.load".into());
            if captured["mode"] == "default_import_loader_error"
                || captured["mode"] == "injected_session"
            {
                return Err(message("owned fixture option loader failure"));
            }
            Ok(Options {
                http_client: Some(network.clone()),
                proxy_url: captured["proxy"].as_str().unwrap().into(),
                user_agent: captured["user_agent"].as_str().unwrap().into(),
                ..Default::default()
            })
        }));
        if input["mode"] == "injected_session" {
            let network = transport.clone();
            let trace = harness.ports.trace.clone();
            harness.service.open_session = Some(Arc::new(move |value| {
                trace
                    .lock()
                    .unwrap()
                    .push(format!("session.factory/{value}"));
                ClientOpen {
                    client: Some(client(value, network.clone())),
                    error: None,
                }
            }));
        }
        let proxy = input["has_override"]
            .as_bool()
            .unwrap()
            .then(|| input["override"].as_str().unwrap());
        let result = harness
            .service
            .import_session_with_proxy(&context, "fixture-injected-session", false, proxy)
            .await;
        assert_error(result.as_ref().err(), &case["error"], &name);
        assert_eq!(
            result.as_ref().map(summary).unwrap_or_else(
                |_| json!({"UserID":0,"DisplayName":"","CreatorID":"","Default":false})
            ),
            case["result"],
            "{name}"
        );
        harness.assert_state(case);
        transport.assert_observations(case);
    }
}

#[test]
fn actual_sparse_config_default_read_and_positive_id_mutations_match_sealed_go() {
    let fixture = fixture();
    for case in rows(&fixture).iter().filter(|case| {
        case["name"]
            .as_str()
            .unwrap()
            .starts_with("config/default_read/")
            || case["name"].as_str().unwrap().starts_with("config/set_id/")
    }) {
        let name = case["name"].as_str().unwrap();
        let mut config = "# preserved fixture\n[unrelated]\nvalue='kept'\n".to_owned();
        let raw = case["input"]["raw"].as_str().unwrap_or("missing");
        if raw != "missing" {
            config += &format!("[fanbox.auth]\ndefault_user_id={raw}\n");
        }
        let harness = Harness::new(&config, false, "", 0, false);
        if name.starts_with("config/set_id/") {
            let id = name.rsplit('/').next().unwrap().parse().unwrap();
            let result = harness
                .ports
                .store
                .set_fanbox_default_user_id(id)
                .map_err(SchedulerError::from);
            assert_error(result.as_ref().err(), &case["error"], name);
        } else {
            let result = harness
                .ports
                .store
                .read_fanbox_default_user_id()
                .map_err(SchedulerError::from);
            assert_error(result.as_ref().err(), &case["error"], name);
            let id = result.ok().flatten();
            assert_eq!(
                json!({"id":id.unwrap_or(0),"present":id.is_some()}),
                case["result"],
                "{name}"
            );
        }
        harness.assert_state(case);
    }
}

#[test]
fn sparse_default_mutations_preserve_other_auth_and_unknown_content_without_ensuring_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().join("nested/config.toml"));
    store.clear_fanbox_default_user_id().unwrap();
    assert_eq!(std::fs::read_to_string(store.path()).unwrap(), "");
    std::fs::write(store.path(), "# retained\n[pixiv.auth]\ndefault_user_id = 7\n[fanbox.auth]\ndefault_user_id = 9\nunknown = 'kept' # trailer\n[fanbox.network]\nproxy_url = 42\n").unwrap();
    store.clear_fanbox_default_user_id().unwrap();
    let body = std::fs::read_to_string(store.path()).unwrap();
    assert!(body.contains("# retained"));
    assert!(body.contains("[pixiv.auth]\ndefault_user_id = 7"));
    assert!(body.contains("unknown = 'kept'  # trailer"));
    assert!(body.contains("[fanbox.network]\nproxy_url = 42"));
    assert_eq!(store.read_fanbox_default_user_id().unwrap(), None);
    store.set_fanbox_default_user_id(42).unwrap();
    assert_eq!(store.read_fanbox_default_user_id().unwrap(), Some(42));
    assert!(
        store
            .current_with_environment([])
            .unwrap()
            .runtime()
            .is_err()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn nil_service_repository_and_context_are_explicit_correspondence_gaps() {
    let fixture = fixture();
    for name in [
        "use/nil_service",
        "remove/nil_service",
        "auto/nil_service",
        "use/nil_repository",
        "remove/nil_repository",
        "use/context/nil_context",
        "remove/context/nil_context",
    ] {
        assert!(
            !row(&fixture, name)["panic"].as_str().unwrap().is_empty(),
            "{name}"
        );
    }
}

#[test]
fn actual_owned_config_io_and_parse_failures_leave_existing_data_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let malformed = Store::new(directory.path().join("malformed.toml"));
    std::fs::write(malformed.path(), "[broken").unwrap();
    assert!(malformed.read_fanbox_default_user_id().is_err());
    assert!(malformed.set_fanbox_default_user_id(42).is_err());
    assert!(malformed.clear_fanbox_default_user_id().is_err());
    assert_eq!(
        std::fs::read_to_string(malformed.path()).unwrap(),
        "[broken"
    );
    let not_file = Store::new(directory.path().join("directory.toml"));
    std::fs::create_dir(not_file.path()).unwrap();
    assert!(not_file.read_fanbox_default_user_id().is_err());
    assert!(not_file.set_fanbox_default_user_id(42).is_err());
    assert!(not_file.clear_fanbox_default_user_id().is_err());
    assert!(not_file.path().is_dir());
    let parent = directory.path().join("file-parent");
    std::fs::write(&parent, "owned fixture parent canary").unwrap();
    let blocked = Store::new(parent.join("config.toml"));
    assert!(blocked.set_fanbox_default_user_id(42).is_err());
    assert!(blocked.clear_fanbox_default_user_id().is_err());
    assert_eq!(
        std::fs::read_to_string(parent).unwrap(),
        "owned fixture parent canary"
    );
}

struct ConfigFilePort {
    path: std::path::PathBuf,
    failure: String,
}
impl ConfigFiles for ConfigFilePort {
    fn path(&self) -> Result<std::path::PathBuf, ConfigError> {
        if self.failure == "path" {
            return Err(ConfigError::Invalid(
                "owned fixture config path failure".into(),
            ));
        }
        Ok(self.path.clone())
    }
    fn read_file(&self, path: &std::path::Path) -> Result<Vec<u8>, ConfigError> {
        if self.failure == "read" {
            return Err(ConfigError::Invalid(
                "owned fixture config read failure".into(),
            ));
        }
        std::fs::read(path).map_err(ConfigError::Io)
    }
    fn write_private_file(&self, path: &std::path::Path, body: &[u8]) -> Result<(), ConfigError> {
        if self.failure == "write" {
            return Err(ConfigError::Invalid(
                "owned fixture config write failure".into(),
            ));
        }
        SystemConfigFiles::new(path).write_private_file(path, body)
    }
    fn ensure_private_file(
        &self,
        _path: &std::path::Path,
        _body: &[u8],
    ) -> Result<(), ConfigError> {
        panic!("authentication default operations must not ensure config")
    }
}

#[test]
fn genuine_config_file_port_faults_match_all_twelve_sealed_boundary_rows() {
    let fixture = fixture();
    let mut compared = 0;
    for action in ["read", "set", "clear"] {
        for failure in ["path", "read", "write", "nil_files"] {
            let name = format!("config/{action}/{failure}");
            let case = row(&fixture, &name);
            let harness = Harness::new("[fanbox.auth]\ndefault_user_id=7\n", false, "", 0, false);
            let path = harness.ports.store.path().to_path_buf();
            let store = Store::with_files(
                &path,
                (failure != "nil_files").then(|| {
                    Arc::new(ConfigFilePort {
                        path: path.clone(),
                        failure: failure.into(),
                    }) as Arc<dyn ConfigFiles>
                }),
            );
            let (observed, error) = match action {
                "read" => {
                    let result = store
                        .read_fanbox_default_user_id()
                        .map_err(SchedulerError::from);
                    let id = result.as_ref().ok().copied().flatten();
                    (
                        json!({"id":id.unwrap_or(0),"present":id.is_some()}),
                        result.err(),
                    )
                }
                "set" => (
                    Value::Null,
                    store.set_fanbox_default_user_id(9).err().map(Into::into),
                ),
                _ => (
                    Value::Null,
                    store.clear_fanbox_default_user_id().err().map(Into::into),
                ),
            };
            assert_error(error.as_ref(), &case["error"], &name);
            assert_eq!(observed, case["result"], "{name}");
            harness.assert_state(case);
            assert!(case["requests"].as_array().unwrap().is_empty());
            assert_eq!(case["idle_closes"], 0);
            assert_eq!(case["body_closes"], 0);
            compared += 1;
        }
    }
    assert_eq!(compared, 12);
}

#[tokio::test]
async fn opaque_session_bytes_reject_invalid_utf8_before_any_request_or_save() {
    for session in [
        b"\xff".as_slice(),
        b"fixture-\xff-session".as_slice(),
        b"fixture-session\xa0".as_slice(),
        b"\xe2\x82".as_slice(),
        b"\xed\xa0\x80".as_slice(),
    ] {
        let mut harness = Harness::new("[fanbox.auth]\ndefault_user_id=7\n", true, "", 0, false);
        let context = context("");
        let transport = Transport::new("success", harness.ports.trace.clone(), &context);
        let network = transport.clone();
        let trace = harness.ports.trace.clone();
        harness.service.load_options = Some(Arc::new(move || {
            trace.lock().unwrap().push("options.load".into());
            Ok(Options {
                http_client: Some(network.clone()),
                ..Default::default()
            })
        }));
        harness.service.open_session = Some(Arc::new(|_| {
            panic!("a String-only opener must never receive altered opaque credentials")
        }));
        let before = snapshot(&harness.ports.database, harness.lower, now());
        let error = harness
            .service
            .import_session_bytes_with_proxy(&context, session, false, None)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "fanbox:Open: credentials_expired: FANBOX cookie header contains a malformed cookie pair"
        );
        let classified = error.classified().unwrap();
        assert_eq!(classified.product, "fanbox");
        assert_eq!(classified.operation, "Open");
        assert_eq!(classified.code, pixiv_sdk::Reason::CredentialsExpired);
        assert_eq!(*harness.ports.trace.lock().unwrap(), ["options.load"]);
        assert!(transport.requests.lock().unwrap().is_empty());
        assert_eq!(transport.body_closes.load(Ordering::SeqCst), 0);
        assert_eq!(transport.idle_closes.load(Ordering::SeqCst), 0);
        assert_eq!(
            snapshot(&harness.ports.database, harness.lower, now()),
            before
        );
        assert_eq!(
            std::fs::read_to_string(harness.ports.store.path()).unwrap(),
            "[fanbox.auth]\ndefault_user_id=7\n"
        );
    }
}

#[tokio::test]
async fn invalid_opaque_session_bytes_preserve_loader_and_proxy_error_precedence() {
    for (proxy, override_proxy, loader_failure, expected) in [
        ("", None, true, "owned fixture option loader failure"),
        (
            "ftp://fixture.invalid",
            None,
            false,
            "fanbox:Open: invalid_argument: fanbox: invalid option: FANBOX proxy URL is invalid",
        ),
        (
            "",
            Some("ftp://fixture.invalid"),
            false,
            "fanbox:Open: invalid_argument: fanbox: invalid option: FANBOX proxy URL is invalid",
        ),
        (
            "ftp://fixture.invalid",
            Some(""),
            false,
            "fanbox:Open: credentials_expired: FANBOX cookie header contains a malformed cookie pair",
        ),
    ] {
        let mut harness = Harness::new("", false, "", 0, false);
        let context = context("");
        let transport = Transport::new("success", harness.ports.trace.clone(), &context);
        let network = transport.clone();
        let trace = harness.ports.trace.clone();
        harness.service.load_options = Some(Arc::new(move || {
            trace.lock().unwrap().push("options.load".into());
            if loader_failure {
                return Err(message("owned fixture option loader failure"));
            }
            Ok(Options {
                http_client: Some(network.clone()),
                proxy_url: proxy.into(),
                ..Default::default()
            })
        }));
        harness.service.open_session = Some(Arc::new(|_| {
            panic!("opaque invalid UTF8 cannot enter the String-only custom opener")
        }));
        let error = harness
            .service
            .import_session_bytes_with_proxy(
                &context,
                b"fixture-\xff-session",
                false,
                override_proxy,
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(*harness.ports.trace.lock().unwrap(), ["options.load"]);
        assert!(transport.requests.lock().unwrap().is_empty());
        assert_eq!(transport.body_closes.load(Ordering::SeqCst), 0);
        assert_eq!(transport.idle_closes.load(Ordering::SeqCst), 0);
        assert!(
            harness
                .ports
                .database
                .lock()
                .unwrap()
                .list_fanbox()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            std::fs::read_to_string(harness.ports.store.path()).unwrap(),
            ""
        );
    }
}

#[tokio::test]
async fn valid_opaque_session_bytes_keep_the_original_injected_opener_and_saved_credential() {
    let fixture = fixture();
    let case = row(&fixture, "import/trimmed first import");
    let mut harness = Harness::new("", false, "", 0, false);
    let context = context("");
    let transport = Transport::new("success", harness.ports.trace.clone(), &context);
    let network = transport.clone();
    let trace = harness.ports.trace.clone();
    harness.service.open_session = Some(Arc::new(move |session| {
        assert_eq!(session, "fixture-import-session");
        trace
            .lock()
            .unwrap()
            .push(format!("session.factory/{session}"));
        ClientOpen {
            client: Some(client(session, network.clone())),
            error: None,
        }
    }));
    harness.service.load_options = Some(Arc::new(|| {
        panic!("valid UTF8 retains custom opener precedence over option loading")
    }));
    let imported = harness
        .service
        .import_session_bytes_with_proxy(
            &context,
            b"  fixture-import-session  ",
            false,
            Some("ftp://fixture.invalid"),
        )
        .await
        .unwrap();
    assert_eq!(summary(&imported), case["result"]);
    harness.assert_state(case);
    transport.assert_observations(case);
    assert_eq!(
        harness
            .ports
            .database
            .lock()
            .unwrap()
            .get_fanbox(42)
            .unwrap()
            .session_id_copy(),
        b"fixture-import-session"
    );
}

fn opaque_fixture() -> Value {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../pixiv-sdk/tests/fixtures/fanbox-opaque-credentials.json"
    ))
    .unwrap();
    assert_eq!(
        fixture["source_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 40);
    fixture
}
fn session_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn opaque_options(case: &Value, transport: Arc<Transport>) -> Options {
    Options {
        http_client: Some(transport),
        user_agent: case["user_agent"].as_str().unwrap().into(),
        proxy_url: case["proxy_url"].as_str().unwrap().into(),
        flare_solverr: case["solver_url"]
            .as_str()
            .filter(|url| !url.is_empty())
            .map(|url| pixiv_sdk::fanbox::FlareSolverrOptions {
                url: url.into(),
                proxy_url: case["solver_proxy_url"].as_str().unwrap().into(),
            }),
    }
}
fn assert_opaque_error(error: &SchedulerError, expected: &Value, name: &str) {
    assert_eq!(error.to_string(), expected["message"], "{name}");
    let error = error.classified().unwrap();
    assert_eq!(error.product, "fanbox", "{name}");
    assert_eq!(error.operation, "Open", "{name}");
    assert_eq!(error.code.as_str(), expected["reason"], "{name}");
}

#[tokio::test]
async fn actual_opaque_import_preserves_go_service_trim_and_sdk_byte_error_precedence() {
    let fixture = opaque_fixture();
    let mut compared = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let session = session_hex(case["session_hex"].as_str().unwrap());
        if std::str::from_utf8(&session).is_ok() {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        let mut harness = Harness::new("[fanbox.auth]\ndefault_user_id=7\n", true, "", 0, false);
        let context = context("");
        let transport = Transport::new("success", harness.ports.trace.clone(), &context);
        let network = transport.clone();
        let trace = harness.ports.trace.clone();
        let captured = case.clone();
        harness.service.load_options = Some(Arc::new(move || {
            trace.lock().unwrap().push("options.load".into());
            Ok(opaque_options(&captured, network.clone()))
        }));
        harness.service.open_session = Some(Arc::new(|_| {
            panic!("invalid Go strings cannot enter the String-only opener")
        }));
        let before = snapshot(&harness.ports.database, harness.lower, now());
        let error = harness
            .service
            .import_session_bytes_with_proxy(&context, &session, false, None)
            .await
            .unwrap_err();
        assert_opaque_error(&error, &case["error_after_trim"], name);
        assert_eq!(
            *harness.ports.trace.lock().unwrap(),
            ["options.load"],
            "{name}"
        );
        assert_eq!(
            snapshot(&harness.ports.database, harness.lower, now()),
            before,
            "{name}"
        );
        assert_eq!(
            std::fs::read_to_string(harness.ports.store.path()).unwrap(),
            "[fanbox.auth]\ndefault_user_id=7\n",
            "{name}"
        );
        assert_eq!(
            json!(*transport.requests.lock().unwrap()),
            case["requests_after_trim"],
            "{name}"
        );
        assert_eq!(
            transport.idle_closes.load(Ordering::SeqCst),
            case["idle_closes_after_trim"].as_u64().unwrap() as usize,
            "{name}"
        );
        assert_eq!(transport.body_closes.load(Ordering::SeqCst), 0, "{name}");
        compared += 1;
    }
    assert_eq!(compared, 24);
}

#[test]
fn actual_sqlite_saved_session_bytes_keep_sdk_errors_lazy_selection_and_untouched_blobs() {
    let fixture = opaque_fixture();
    let mut compared = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let session = session_hex(case["session_hex"].as_str().unwrap());
        if std::str::from_utf8(&session).is_ok() {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        let mut harness = Harness::new("[fanbox.auth]\ndefault_user_id=7\n", true, "", 0, false);
        let connection =
            rusqlite::Connection::open(harness.ports.database.lock().unwrap().path()).unwrap();
        connection
            .execute(
                "UPDATE fanbox_account SET session_id=?1 WHERE user_id=7",
                [&session],
            )
            .unwrap();
        drop(connection);
        let before = harness
            .ports
            .database
            .lock()
            .unwrap()
            .get_fanbox(7)
            .unwrap();
        let context = context("");
        let transport = Transport::new("success", harness.ports.trace.clone(), &context);
        let network = transport.clone();
        let trace = harness.ports.trace.clone();
        let captured = case.clone();
        harness.service.load_options = Some(Arc::new(move || {
            trace.lock().unwrap().push("options.load".into());
            Ok(opaque_options(&captured, network.clone()))
        }));
        let opened = harness.service.open_client_with_proxy(&context, None);
        assert!(opened.client.is_none(), "{name}");
        assert_opaque_error(opened.error.as_ref().unwrap(), &case["error"], name);
        assert_eq!(
            *harness.ports.trace.lock().unwrap(),
            [
                "defaults.read",
                "repository.get/context=caller/err=",
                "repository.get/context=caller/err=",
                "options.load"
            ],
            "{name}"
        );
        let after = harness
            .ports
            .database
            .lock()
            .unwrap()
            .get_fanbox(7)
            .unwrap();
        assert_eq!(after.session_id_copy(), session, "{name}");
        assert_eq!(after.session_id_copy(), before.session_id_copy(), "{name}");
        assert_eq!(
            after.credential_revision, before.credential_revision,
            "{name}"
        );
        assert_eq!(after.validated_at, before.validated_at, "{name}");
        assert_eq!(after.created_at, before.created_at, "{name}");
        assert_eq!(after.updated_at, before.updated_at, "{name}");
        assert_eq!(
            std::fs::read_to_string(harness.ports.store.path()).unwrap(),
            "[fanbox.auth]\ndefault_user_id=7\n",
            "{name}"
        );
        assert_eq!(
            json!(*transport.requests.lock().unwrap()),
            case["requests"],
            "{name}"
        );
        assert_eq!(
            transport.idle_closes.load(Ordering::SeqCst),
            case["idle_closes"].as_u64().unwrap() as usize,
            "{name}"
        );
        assert_eq!(transport.body_closes.load(Ordering::SeqCst), 0, "{name}");
        compared += 1;
    }
    assert_eq!(compared, 24);
}
