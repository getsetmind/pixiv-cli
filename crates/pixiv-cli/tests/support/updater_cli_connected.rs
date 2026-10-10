use pixiv_app::{
    callback_handler::CallbackResult,
    config::Store,
    lifecycle::Context,
    reverse_search::http::{HttpRequest, HttpTransport},
    update::{
        BuildInfo, CallerContext, Command, CommandRunner, ExternalError, InstallSource, Release,
        ReleaseCache, ReleaseCheckOptions, ReleaseCheckResult, ReleaseChecker, ReleaseInstaller,
        SourceDetector, UpdateFuture,
        coordinator::{AutomaticChecker, AutomaticCheckerOptions, Coordinator, CoordinatorOptions},
        release::{GitHubReleaseClient, ReleaseClientOptions},
    },
};
use pixiv_cli_rs::{
    CommandError,
    startup::StartupHooks,
    update::{
        AutomaticCheckHost, AutomaticCommand, AutomaticRuntime, UpdateHost, UpdateOptions,
        UpdatePreparation, UpdateRuntime,
    },
};
use pixiv_sdk::{
    context::ContextKey,
    fanbox::transport::{BodyFuture, RawBody, RawRead, RawResponse},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::{Arc, Mutex},
};

pub fn fixture() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("../fixtures/updater-cli-owner.json")).unwrap()["cases"].as_array().unwrap().clone()
}
pub fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).into()).collect()
}
pub fn arguments(input: &Value) -> Vec<String> {
    input["args"]
        .as_array()
        .map(|args| {
            args.iter()
                .map(|arg| arg.as_str().unwrap().into())
                .collect()
        })
        .unwrap_or_default()
}

pub fn build(input: &Value) -> BuildInfo {
    BuildInfo {
        version: input["version"].as_str().unwrap().into(),
    }
}
pub fn library_update_row(row: &Value) -> bool {
    let name = row["name"].as_str().unwrap();
    let input = &row["input"];
    if row["evidence_class"] != "behavioral"
        || arguments(input).first().map(String::as_str) != Some("update")
    {
        return false;
    }
    !matches!(
        input["failure"].as_str(),
        Some("production-factory" | "ensure-config")
    ) && !name.contains("diagnostic-")
        && !name.contains("diagnostic-and-")
}
pub fn library_automatic_row(row: &Value) -> bool {
    let name = row["name"].as_str().unwrap();
    row["evidence_class"] == "behavioral"
        && name.starts_with("automatic/")
        && !name.starts_with("automatic/eligible/")
        && !name.starts_with("automatic/excluded/")
        && !name.starts_with("automatic/close-")
        && !name.contains("production")
        && name != "automatic/invalid-proxy-warning"
        && name != "automatic/proxy/--proxy=x --no-proxy=false"
}
pub fn assert_flags(options: &UpdateOptions, flags: &Value, name: &Value) {
    let flags = flags.as_array().unwrap();
    for (flag, value) in [
        ("check", options.check),
        ("prerelease", options.include_prerelease),
        ("json", options.json),
    ] {
        assert_eq!(
            value.to_string(),
            flags.iter().find(|f| f["name"] == flag).unwrap()["value"],
            "{name} --{flag}"
        );
    }
    let proxy = flags.iter().find(|f| f["name"] == "proxy").unwrap();
    assert_eq!(options.proxy.is_some(), proxy["changed"].as_bool().unwrap());
    if let Some(value) = &options.proxy {
        assert_eq!(value, proxy["value"].as_str().unwrap());
    }
}
pub fn automatic_metadata(input: &Value) -> AutomaticCommand {
    let args = arguments(input);
    let mut names = vec!["pixiv".to_owned()];
    let mut command = AutomaticCommand::default();
    for arg in args {
        if let Some(proxy) = arg.strip_prefix("--proxy=") {
            command.proxy = Some(proxy.into());
        } else if arg.starts_with("--no-proxy") {
            command.no_proxy_changed = true;
        } else if arg.starts_with("--help") || arg == "-h" {
            command.help_changed = true;
        } else if !arg.starts_with('-')
            && matches!(
                arg.as_str(),
                "config"
                    | "get"
                    | "path"
                    | "auth"
                    | "list"
                    | "fanbox"
                    | "detail"
                    | "import"
                    | "export"
                    | "mcp"
                    | "update"
                    | "_callback"
            )
        {
            names.push(arg);
        }
    }
    command.names = names;
    command
}

pub struct Sink {
    mode: String,
    bytes: Vec<u8>,
    writes: Vec<Value>,
}
impl Sink {
    pub fn new(input: &Value, key: &str) -> Self {
        Self {
            mode: input[key].as_str().unwrap_or("").into(),
            bytes: Vec::new(),
            writes: Vec::new(),
        }
    }
    pub fn text(&self) -> String {
        String::from_utf8(self.bytes.clone()).unwrap()
    }
    pub fn writes(&self) -> Value {
        json!(self.writes)
    }
}
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let (n, error) = match self.mode.as_str() {
            "error" => (0, Some(io::Error::other("owned writer failure"))),
            "epipe" => (0, Some(io::Error::from(io::ErrorKind::BrokenPipe))),
            "short" => (0, None),
            "partial-error" => (
                bytes.len() / 2,
                Some(io::Error::other("owned writer failure")),
            ),
            "" => (bytes.len(), None),
            other => panic!("unsupported writer mode {other}"),
        };
        self.bytes.extend_from_slice(&bytes[..n]);
        self.writes.push(json!({"length": bytes.len(), "n": n, "error": error.as_ref().map(ToString::to_string).unwrap_or_default()}));
        match error {
            Some(error) => Err(error),
            None => Ok(n),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Observation {
    trace: Vec<String>,
    detections: Vec<BuildInfo>,
    requests: Vec<Value>,
    proxies: Vec<String>,
    automatic_proxies: Vec<String>,
    commands: Vec<Command>,
    installs: Vec<Release>,
    runtime_calls: usize,
    context_seen: bool,
}
struct Ports {
    input: Value,
    context: CallerContext,
    key: ContextKey,
    cache: Mutex<Option<Vec<u8>>>,
    observed: Arc<Mutex<Observation>>,
}
pub struct Host {
    pub directory: tempfile::TempDir,
    pub context: CallerContext,
    pub startup_context: Context,
    input: Value,
    store: Store,
    ports: Arc<Ports>,
}
impl Host {
    pub fn new(input: Value) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().join("config.toml"));
        if let Some(config) = input["config_before"].as_str() {
            std::fs::write(store.path(), config).unwrap();
        }
        let key = ContextKey::new("updater-cli-owned-context");
        let concrete = Context::new().with_value(
            key.clone(),
            Arc::new(String::from("synthetic context value")),
        );
        if input["canceled"].as_bool().unwrap_or(false) {
            concrete.cancel();
        }
        let startup_context = concrete.clone();
        let context: CallerContext = Arc::new(concrete);
        let ports = Arc::new(Ports {
            input: input.clone(),
            context: context.clone(),
            key,
            cache: Mutex::new(
                input["cache_before"]
                    .as_str()
                    .map(|text| text.as_bytes().to_vec()),
            ),
            observed: Arc::new(Mutex::new(Observation::default())),
        });
        Self {
            directory,
            context,
            startup_context,
            input,
            store,
            ports,
        }
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn proxies(&self) -> Vec<String> {
        self.ports.observed.lock().unwrap().proxies.clone()
    }
    pub fn automatic_proxies(&self) -> Vec<String> {
        self.ports
            .observed
            .lock()
            .unwrap()
            .automatic_proxies
            .clone()
    }
    pub fn commands(&self) -> Vec<Command> {
        self.ports.observed.lock().unwrap().commands.clone()
    }
    pub fn installs(&self) -> Vec<Release> {
        self.ports.observed.lock().unwrap().installs.clone()
    }
    pub fn trace(&self) -> Vec<String> {
        self.ports.observed.lock().unwrap().trace.clone()
    }
    pub fn detections(&self) -> Vec<BuildInfo> {
        self.ports.observed.lock().unwrap().detections.clone()
    }
    pub fn requests(&self) -> Vec<Value> {
        self.ports.observed.lock().unwrap().requests.clone()
    }
    pub fn context_seen(&self) -> bool {
        self.ports.observed.lock().unwrap().context_seen
    }
    pub fn cache_text(&self) -> Value {
        self.ports
            .cache
            .lock()
            .unwrap()
            .as_ref()
            .map(|bytes| Value::String(String::from_utf8(bytes.clone()).unwrap()))
            .unwrap_or(Value::Null)
    }
    fn runtime(&self) -> Result<pixiv_app::config::RuntimeConfig, CommandError> {
        let mut observed = self.ports.observed.lock().unwrap();
        observed.runtime_calls += 1;
        observed.trace.push(format!(
            "runtime.load/config_exists={}",
            self.store.path().exists()
        ));
        if observed.runtime_calls == self.input["runtime_error_nth"].as_u64().unwrap_or(0) as usize
        {
            return Err(CommandError::Message("owned runtime failure"));
        }
        drop(observed);
        let env: BTreeMap<String, String> =
            serde_json::from_value(self.input["environment"].clone()).unwrap();
        self.store
            .current_with_environment(env)
            .and_then(|snapshot| snapshot.runtime())
            .map_err(|error| CommandError::State(Box::new(error)))
    }
    fn checker(&self) -> Arc<dyn ReleaseChecker> {
        if self.input["failure"] == "checker" {
            return self.ports.clone();
        }
        Arc::new(
            GitHubReleaseClient::new(ReleaseClientOptions {
                transport: Some(Arc::new(pixiv_app::update::http::ClientTransport::new(
                    self.ports.clone(),
                ))),
                cache: Some(self.ports.clone()),
                now: Some(Arc::new(|| "2026-10-10T12:00:00Z".parse().unwrap())),
                ..ReleaseClientOptions::default()
            })
            .unwrap(),
        )
    }
}
impl StartupHooks for Host {
    fn cleanup_pending_update(&self) -> CallbackResult<()> {
        self.ports
            .observed
            .lock()
            .unwrap()
            .trace
            .push("startup.cleanup".into());
        if self.input["failure"] == "startup" {
            return Err(Box::new(io::Error::other("owned startup failure")));
        }
        Ok(())
    }
    fn automatic_supported(&self) -> bool {
        self.ports
            .observed
            .lock()
            .unwrap()
            .trace
            .push("startup.supported".into());
        self.input["relay_supported"].as_bool().unwrap_or(false)
    }
    fn ensure_if_needed(&self, _: &Context) -> CallbackResult<()> {
        self.ports
            .observed
            .lock()
            .unwrap()
            .trace
            .push("startup.relay".into());
        Err(Box::new(io::Error::other("owned relay failure")))
    }
}
impl UpdatePreparation for Host {
    fn ensure_update_config(&self) -> Result<(), CommandError> {
        self.store
            .ensure_defaults()
            .map_err(|error| CommandError::State(Box::new(error)))
    }
    fn start_update_diagnostics(&self) -> Result<(), CommandError> {
        self.runtime().map(|_| ())
    }
}
impl UpdateHost for Host {
    fn load_update_runtime_config(&self) -> Result<UpdateRuntime, CommandError> {
        self.runtime().map(|runtime| UpdateRuntime {
            https_proxy: runtime.https_proxy,
        })
    }
    fn new_update_coordinator(&self, proxy: &str) -> Result<Coordinator, CommandError> {
        self.ports
            .observed
            .lock()
            .unwrap()
            .trace
            .push("update.factory".into());
        self.ports
            .observed
            .lock()
            .unwrap()
            .proxies
            .push(proxy.into());
        if self.input["failure"] == "factory" {
            return Err(CommandError::Message("owned coordinator factory failure"));
        }
        Coordinator::new(CoordinatorOptions {
            source_detector: Some(self.ports.clone()),
            release_checker: Some(self.checker()),
            command_runner: Some(self.ports.clone()),
            release_installer: Some(self.ports.clone()),
        })
        .map_err(CommandError::State)
    }
}
impl AutomaticCheckHost for Host {
    fn load_automatic_update_runtime_config(&self) -> Result<AutomaticRuntime, CommandError> {
        self.runtime().map(|runtime| AutomaticRuntime {
            enabled: runtime.update_check_enabled,
            https_proxy: runtime.https_proxy,
        })
    }
    fn new_automatic_update_checker(&self, proxy: &str) -> Result<AutomaticChecker, CommandError> {
        self.ports
            .observed
            .lock()
            .unwrap()
            .trace
            .push("automatic.factory".into());
        self.ports
            .observed
            .lock()
            .unwrap()
            .automatic_proxies
            .push(proxy.into());
        if self.input["failure"] == "automatic-factory" {
            return Err(CommandError::Message("owned automatic factory failure"));
        }
        AutomaticChecker::new(AutomaticCheckerOptions {
            source_detector: Some(self.ports.clone()),
            release_checker: Some(self.checker()),
        })
        .map_err(CommandError::State)
    }
}
impl SourceDetector for Ports {
    fn detect(&self, info: &BuildInfo) -> Result<InstallSource, ExternalError> {
        {
            let mut observed = self.observed.lock().unwrap();
            observed.trace.push("update.detect".into());
            observed.detections.push(info.clone());
        }
        if self.input["failure"] == "detector" {
            return Err(Box::new(io::Error::other("owned detector failure")));
        }
        Ok(if info.is_development() {
            InstallSource::Development
        } else {
            InstallSource::from(self.input["source"].as_str().unwrap())
        })
    }
}
impl ReleaseChecker for Ports {
    fn check(
        &self,
        _: CallerContext,
        _: ReleaseCheckOptions,
    ) -> UpdateFuture<'_, Result<ReleaseCheckResult, ExternalError>> {
        Box::pin(async {
            Err(Box::new(io::Error::other("owned release checker failure")) as ExternalError)
        })
    }
}
impl ReleaseCache for Ports {
    fn read(&self, _: CallerContext) -> UpdateFuture<'_, Result<Option<Vec<u8>>, ExternalError>> {
        Box::pin(async { Ok(self.cache.lock().unwrap().clone()) })
    }
    fn write(
        &self,
        _: CallerContext,
        data: Vec<u8>,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            *self.cache.lock().unwrap() = Some(data);
            Ok(())
        })
    }
}
impl CommandRunner for Ports {
    fn run(
        &self,
        _: CallerContext,
        command: Command,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            {
                let mut observed = self.observed.lock().unwrap();
                observed.trace.push("update.command".into());
                observed.commands.push(command);
            }
            Ok(())
        })
    }
}
impl ReleaseInstaller for Ports {
    fn install(
        &self,
        _: CallerContext,
        release: Release,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            {
                let mut observed = self.observed.lock().unwrap();
                observed.trace.push("update.install".into());
                observed.installs.push(release);
            }
            Ok(())
        })
    }
}
impl HttpTransport for Ports {
    fn send(
        &self,
        request: HttpRequest,
    ) -> UpdateFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            {
                let mut observed = self.observed.lock().unwrap();
                observed.trace.push("update.http".into());
                observed.requests.push(json!({ "method":request.method,"url":request.url,"headers":request.headers,"has_deadline":request.context.deadline().is_some(),"canceled":request.context.error().is_some() }));
            }
            assert_eq!(request.method, "GET");
            assert_eq!(
                request.url,
                "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases"
            );
            assert_eq!(
                request.headers.get("User-Agent"),
                Some(&vec!["pixiv-cli".into()])
            );
            assert_eq!(
                request
                    .context
                    .value(&self.key)
                    .unwrap()
                    .downcast::<String>()
                    .unwrap()
                    .as_str(),
                "synthetic context value"
            );
            self.observed.lock().unwrap().context_seen =
                Arc::ptr_eq(&request.context, &self.context);
            if self.input["failure"] == "http" {
                return Err(Box::new(io::Error::other("owned HTTP failure")) as ExternalError);
            }
            let body = if self.input["release_tag"] == "" { json!([]) } else { json!([{"tag_name":self.input["release_tag"],"prerelease":self.input["release_prerelease"],"draft":false,"assets":[]}]) }.to_string().into_bytes();
            Ok(Some(RawResponse {
                status: 200,
                headers: [
                    ("Content-Type".into(), vec!["application/json".into()]),
                    ("Etag".into(), vec!["owned-etag".into()]),
                ]
                .into(),
                content_length: body.len() as i64,
                body: Some(Box::new(Body {
                    bytes: body,
                    offset: 0,
                })),
            }))
        })
    }
}
struct Body {
    bytes: Vec<u8>,
    offset: usize,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let n = output.len().min(self.bytes.len() - self.offset);
            output[..n].copy_from_slice(&self.bytes[self.offset..self.offset + n]);
            self.offset += n;
            RawRead {
                count: n,
                eof: self.offset == self.bytes.len(),
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async { Ok(()) })
    }
}

pub fn assert_installs(actual: &[Release], expected: &Value, name: &Value) {
    let expected = expected.as_array().unwrap();
    assert_eq!(actual.len(), expected.len(), "{name} installs");
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.tag_name, expected["TagName"]);
        assert_eq!(actual.version, expected["Version"]);
        assert_eq!(actual.prerelease, expected["Prerelease"]);
        assert!(
            expected["Assets"].is_null(),
            "{name} Go selected-release assets representation changed"
        );
        assert!(
            actual.assets.is_empty(),
            "{name} Rust selected-release assets changed"
        );
    }
}
