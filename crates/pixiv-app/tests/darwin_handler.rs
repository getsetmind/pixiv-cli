#![cfg(unix)]
use pixiv_app::{
    callback_handler::PreviousHandler,
    darwin_handler::{
        DARWIN_BUNDLE_ID, DARWIN_INFO_PLIST, DARWIN_LSREGISTER, DARWIN_SOURCE_VERSION,
        DARWIN_SWIFT_SOURCE, DarwinEnvironment, DarwinHandler, save_bundle_manifest,
    },
    handler_manifest::{HandlerManifest, HandlerManifestStore},
    host_context::ContextHostProcess,
    host_process::{HostPlatform, HostProcess, HostProcessError, ProcessOutput, ProcessStdio},
    lifecycle::Context,
};
use serde_json::Value;
use std::{
    ffi::{OsStr, OsString},
    fs, io,
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/darwin_handler.json")).unwrap()
}
struct Environment {
    home: PathBuf,
    temporary: PathBuf,
    executable: Mutex<PathBuf>,
    events: Arc<Mutex<Vec<String>>>,
    fail_executable: bool,
    fail_home: bool,
}
impl DarwinEnvironment for Environment {
    fn home_directory(&self) -> io::Result<PathBuf> {
        self.events.lock().unwrap().push("home".into());
        if self.fail_home {
            Err(io::Error::other("synthetic home"))
        } else {
            Ok(self.home.clone())
        }
    }
    fn executable_path(&self) -> io::Result<PathBuf> {
        self.events.lock().unwrap().push("executable".into());
        if self.fail_executable {
            Err(io::Error::other("synthetic executable"))
        } else {
            Ok(self.executable.lock().unwrap().clone())
        }
    }
    fn temporary_directory(&self) -> PathBuf {
        self.temporary.clone()
    }
}
struct State {
    current: String,
    failure: String,
    trace: Vec<String>,
    calls: Vec<(String, Vec<String>, ProcessStdio, bool)>,
    sources: Vec<PathBuf>,
    block_save: Option<PathBuf>,
    block_open: bool,
    canceled_open: bool,
}
struct Process {
    state: Mutex<State>,
    temporary: PathBuf,
    events: Arc<Mutex<Vec<String>>>,
}
fn output(stdout: &[u8]) -> ProcessOutput {
    ProcessOutput {
        status: std::process::ExitStatus::from_raw(0),
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    }
}
fn combined_failure() -> HostProcessError {
    HostProcessError::Captured {
        source: Box::new(HostProcessError::Native(io::Error::other("synthetic exit"))),
        stdout: b" \nstdout\nstderr\n \t".to_vec(),
        stderr: Vec::new(),
    }
}
impl HostProcess for Process {
    fn platform(&self) -> HostPlatform {
        HostPlatform::Darwin
    }
    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        let label = format!("find:{}", program.to_string_lossy());
        self.events.lock().unwrap().push(label.clone());
        let mut state = self.state.lock().unwrap();
        state.trace.push(label);
        if state.failure == program.to_string_lossy() {
            return Err(HostProcessError::Native(io::Error::other(format!(
                "synthetic missing {}",
                program.to_string_lossy()
            ))));
        }
        Ok(Path::new("/synthetic/bin").join(program))
    }
    fn run(
        &self,
        _: &OsStr,
        _: &[OsString],
        _: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        panic!("Darwin processes must use Context")
    }
    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("previous bundle must use explicit open -b")
    }
}
impl ContextHostProcess for Process {
    fn run_context(
        &self,
        context: &Context,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        let arguments = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let name = program.to_string_lossy();
        let mut state = self.state.lock().unwrap();
        state.calls.push((
            name.into_owned(),
            arguments.clone(),
            stdio,
            context.error().is_some(),
        ));
        if program == "swiftc" {
            assert_eq!(stdio, ProcessStdio::Combined);
            assert_eq!(arguments[1], "-o");
            state.trace.push("compile".into());
            let source = PathBuf::from(&args[0]);
            assert_eq!(source.parent().unwrap().parent().unwrap(), self.temporary);
            assert!(
                source
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("pixiv-cli-url-handler-")
            );
            assert_eq!(
                fs::metadata(source.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::symlink_metadata(&source).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert!(fs::symlink_metadata(&source).unwrap().file_type().is_file());
            assert_eq!(
                fs::read(&source).unwrap(),
                fixture()["swift_source"].as_str().unwrap().as_bytes()
            );
            state.sources.push(source);
            if let Some(reason) = context.error() {
                return Err(HostProcessError::Context(reason));
            }
            if state.failure == "compile" {
                return Err(combined_failure());
            }
            let executable = Path::new(&args[2]);
            if executable.is_dir() {
                fs::remove_dir(executable).unwrap();
            }
            fs::write(executable, b"compiled").unwrap();
            return Ok(output(b""));
        }
        if program == DARWIN_LSREGISTER {
            assert_eq!(stdio, ProcessStdio::Combined);
            assert_eq!(arguments[0], "-f");
            state.trace.push("register".into());
            return if state.failure == "register" {
                Err(combined_failure())
            } else {
                Ok(output(b""))
            };
        }
        if program == "swift" {
            assert_eq!(arguments[0], "-e");
            if arguments[1] == fixture()["query_script"].as_str().unwrap() {
                assert_eq!(stdio, ProcessStdio::CaptureStdout);
                state.trace.push("query".into());
                return if state.failure == "query" {
                    Err(HostProcessError::Native(io::Error::other(
                        "synthetic query",
                    )))
                } else {
                    Ok(output(format!(" \n{}\n\u{00a0}", state.current).as_bytes()))
                };
            }
            assert_eq!(stdio, ProcessStdio::Combined);
            let template = fixture()["set_script_template"]
                .as_str()
                .unwrap()
                .to_owned();
            let (prefix, suffix) = template.split_once("{quoted_bundle_id}").unwrap();
            let quoted = arguments[1]
                .strip_prefix(prefix)
                .unwrap()
                .strip_suffix(suffix)
                .unwrap();
            let bundle: String = serde_json::from_str(quoted).unwrap();
            state.trace.push(format!("set:{bundle}"));
            if state.failure == "set" && (bundle == DARWIN_BUNDLE_ID || state.trace.len() == 2) {
                return Err(combined_failure());
            }
            state.current = bundle;
            if let Some(path) = state.block_save.take() {
                fs::create_dir_all(path).unwrap();
            }
            return Ok(output(b""));
        }
        assert_eq!(program, "open");
        assert_eq!(stdio, ProcessStdio::Discard);
        assert_eq!(arguments[0], "-b");
        assert_eq!(arguments[2], fixture()["callback_url"].as_str().unwrap());
        state.trace.push("open".into());
        if state.block_open {
            drop(state);
            while context.error().is_none() {
                std::thread::sleep(Duration::from_millis(1));
            }
            self.state.lock().unwrap().canceled_open = true;
            return Err(HostProcessError::Context(context.error().unwrap()));
        }
        if state.failure == "open" {
            Err(combined_failure())
        } else {
            Ok(output(b""))
        }
    }
}
struct Harness {
    _root: TempDir,
    home: PathBuf,
    environment: Arc<Environment>,
    process: Arc<Process>,
    handler: DarwinHandler,
    store: HandlerManifestStore,
    endpoint: PathBuf,
}
impl Harness {
    fn new(previous: &str, failure: &str) -> Self {
        let root = TempDir::new().unwrap();
        let home = root.path().join("home");
        let temporary = root.path().join("tmp");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir(&temporary).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let environment = Arc::new(Environment {
            home: home.clone(),
            temporary: temporary.clone(),
            executable: Mutex::new(PathBuf::from("/synthetic/current-pixiv")),
            events: events.clone(),
            fail_executable: false,
            fail_home: false,
        });
        let store =
            HandlerManifestStore::new(home.join(".pixiv-cli/url-handler/handler-manifest.json"));
        let endpoint = home.join(".pixiv-cli/url-handler-endpoint");
        let process = Arc::new(Process {
            state: Mutex::new(State {
                current: previous.into(),
                failure: failure.into(),
                trace: Vec::new(),
                calls: Vec::new(),
                sources: Vec::new(),
                block_save: None,
                block_open: false,
                canceled_open: false,
            }),
            temporary,
            events,
        });
        let handler = DarwinHandler::new(
            environment.clone(),
            process.clone(),
            store.clone(),
            &endpoint,
        );
        Self {
            _root: root,
            home,
            environment,
            process,
            handler,
            store,
            endpoint,
        }
    }
    fn seed(&self, previous: &str) {
        self.store
            .save(&HandlerManifest {
                version: 1,
                executable_path: "/old/pixiv".into(),
                previous_handler: previous.into(),
                ..HandlerManifest::default()
            })
            .unwrap();
    }
    fn source_cleaned(&self) {
        for source in &self.process.state.lock().unwrap().sources {
            assert!(!source.parent().unwrap().exists());
        }
    }
}
#[test]
fn persistent_temporary_and_delegation_flows_match_frozen_go() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let operation = case["operation"].as_str().unwrap();
        let previous = case["previous"].as_str().unwrap();
        let failure = case["failure"].as_str().unwrap();
        let harness = Harness::new(previous, failure);
        let context = Context::new();
        if matches!(operation, "disable" | "delegate") {
            harness.seed(previous);
            harness.process.state.lock().unwrap().current =
                case["current"].as_str().unwrap().into();
        }
        if operation == "repeat" {
            harness.handler.ensure_persistent(&context).unwrap();
            harness.process.state.lock().unwrap().trace.clear();
            *harness.environment.executable.lock().unwrap() = "/updated/pixiv".into();
        }
        if failure == "save" {
            harness.process.state.lock().unwrap().block_save = Some(harness.store.path().into());
        }
        let result = match operation {
            "ensure" | "repeat" => harness.handler.ensure_persistent(&context),
            "disable" | "disable-missing" => harness.handler.disable_persistent(&context),
            "delegate" | "delegate-missing" => harness
                .handler
                .delegate_previous(&context, fixture["callback_url"].as_str().unwrap()),
            "install" => {
                let relay = if failure == "endpoint" {
                    "invalid endpoint"
                } else {
                    fixture["endpoint_url"].as_str().unwrap()
                };
                harness
                    .handler
                    .install(&context, relay)
                    .map(|mut installation| {
                        assert!(harness.endpoint.exists());
                        harness.process.state.lock().unwrap().current = "external.choice".into();
                        installation.cleanup();
                        installation.cleanup();
                    })
            }
            value => panic!("unknown operation {value}"),
        };
        let actual = result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        if failure == "save" {
            assert!(!actual.is_empty(), "{name}");
        } else {
            assert_eq!(actual, case["error"].as_str().unwrap(), "{name}");
        }
        let state = harness.process.state.lock().unwrap();
        assert_eq!(
            state.trace,
            case["trace"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            "{name}"
        );
        assert_eq!(
            harness.store.path().is_file(),
            case["manifest_exists"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(
            harness.endpoint.exists(),
            case["endpoint_exists"].as_bool().unwrap(),
            "{name}"
        );
        drop(state);
        if result_is_successful_persistent(operation, failure) {
            let manifest = harness.store.load().unwrap().unwrap();
            assert_eq!(manifest.previous_handler, previous, "{name}");
            assert_eq!(
                manifest.home_directory,
                harness.home.to_string_lossy(),
                "{name}"
            );
            assert_eq!(
                manifest.executable_path,
                harness
                    .environment
                    .executable
                    .lock()
                    .unwrap()
                    .to_string_lossy(),
                "{name}"
            );
            let bundle = harness
                .handler
                .app_path()
                .unwrap()
                .join("Contents/Resources/handler-manifest.json");
            assert_eq!(
                serde_json::from_slice::<Value>(&fs::read(bundle).unwrap()).unwrap(),
                serde_json::to_value(manifest).unwrap()
            );
        }
        harness.source_cleaned();
    }
}
fn result_is_successful_persistent(operation: &str, failure: &str) -> bool {
    matches!(operation, "ensure" | "repeat") && failure.is_empty()
}
#[test]
fn embedded_assets_and_bundle_zero_version_match_frozen_go_bytes() {
    let fixture = fixture();
    assert_eq!(DARWIN_BUNDLE_ID, fixture["bundle_id"]);
    assert_eq!(DARWIN_SOURCE_VERSION, fixture["source_version"]);
    assert_eq!(DARWIN_INFO_PLIST, fixture["plist"]);
    assert_eq!(DARWIN_SWIFT_SOURCE, fixture["swift_source"]);
    let harness = Harness::new("old.bundle", "");
    assert_eq!(
        harness.handler.app_path().unwrap(),
        harness
            .home
            .join(fixture["app_relative_path"].as_str().unwrap())
    );
    let manifest: HandlerManifest =
        serde_json::from_value(fixture["bundle_manifest_zero_version"].clone()).unwrap();
    let app = harness.handler.app_path().unwrap();
    save_bundle_manifest(&app, &manifest).unwrap();
    let path = app.join("Contents/Resources/handler-manifest.json");
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
        fixture["bundle_manifest_zero_version"]
    );
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    harness.store.save(&manifest).unwrap();
    assert_eq!(harness.store.load().unwrap().unwrap().version, 1);
}
#[test]
fn compiler_source_is_private_random_cleaned_and_legacy_symlink_is_untouched() {
    for failure in ["", "compile", "cancel"] {
        let harness = Harness::new("old.bundle", failure);
        let canary = harness.environment.temporary.join("canary");
        fs::write(&canary, b"untouched").unwrap();
        std::os::unix::fs::symlink(
            &canary,
            harness
                .environment
                .temporary
                .join("pixiv-cli-url-handler.swift"),
        )
        .unwrap();
        let context = Context::new();
        if failure == "cancel" {
            context.cancel();
        }
        let app = harness.handler.app_path().unwrap();
        let result = harness.handler.ensure_app(&context, &app);
        if failure == "compile" {
            assert_eq!(
                result.unwrap_err().to_string(),
                "compile pixiv:// callback helper: synthetic exit: stdout\nstderr"
            );
        } else if failure == "cancel" {
            assert_eq!(
                result.unwrap_err().to_string(),
                "compile pixiv:// callback helper: context canceled: "
            );
        } else {
            result.unwrap();
            assert_eq!(
                fs::read(app.join("Contents/Info.plist")).unwrap(),
                DARWIN_INFO_PLIST.as_bytes()
            );
            assert_eq!(
                fs::read(app.join("Contents/Resources/source-version")).unwrap(),
                b"6\n"
            );
            harness.handler.ensure_app(&context, &app).unwrap();
            assert_eq!(harness.process.state.lock().unwrap().sources.len(), 1);
        }
        assert_eq!(fs::read(canary).unwrap(), b"untouched");
        harness.source_cleaned();
    }
}
#[test]
fn cached_bundle_kind_and_version_follow_go_stat_semantics() {
    for case in fixture()["compiler_cache_cases"].as_array().unwrap() {
        let harness = Harness::new("old.bundle", "");
        let app = harness.handler.app_path().unwrap();
        harness.handler.ensure_app(&Context::new(), &app).unwrap();
        harness.process.state.lock().unwrap().trace.clear();
        let executable = app.join("Contents/MacOS/PixivCLIURLHandler");
        let plist = app.join("Contents/Info.plist");
        let version = app.join("Contents/Resources/source-version");
        match case["kind"].as_str().unwrap() {
            "trim" => fs::write(&version, b" \t6\n").unwrap(),
            "symlink" => {
                for path in [&executable, &plist] {
                    let target = path.with_extension("target");
                    fs::rename(path, &target).unwrap();
                    std::os::unix::fs::symlink(target, path).unwrap();
                }
            }
            "exec-directory" => {
                fs::remove_file(&executable).unwrap();
                fs::create_dir(&executable).unwrap();
            }
            "plist-directory" => {
                fs::remove_file(&plist).unwrap();
                fs::create_dir(&plist).unwrap();
            }
            "missing" => fs::remove_file(&executable).unwrap(),
            kind => panic!("unknown cache kind {kind}"),
        }
        let _ = harness.handler.ensure_app(&Context::new(), &app);
        assert_eq!(
            harness
                .process
                .state
                .lock()
                .unwrap()
                .trace
                .contains(&"compile".into()),
            case["compile"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        harness.source_cleaned();
    }
}
#[test]
fn delegated_bundle_quotes_are_escaped_by_production_swift_command() {
    let fixture = fixture();
    let case = &fixture["native_script_cases"][1];
    let previous = case["bundle_id"].as_str().unwrap();
    let harness = Harness::new(DARWIN_BUNDLE_ID, "");
    harness.seed(previous);
    harness.handler.disable_persistent(&Context::new()).unwrap();
    let state = harness.process.state.lock().unwrap();
    assert_eq!(
        state.calls[1].1[1],
        case["set_script"].as_str().unwrap().replacen(
            "scheme=\"pixiv\\\"\\\\\\n\"",
            "scheme=\"pixiv\"",
            1
        )
    );
}
#[tokio::test]
async fn previous_handler_offloads_and_cancellation_reaches_blocking_process() {
    let harness = Harness::new("old.bundle", "");
    harness.seed("old.bundle");
    harness.process.state.lock().unwrap().block_open = true;
    let cancellation = CancellationToken::new();
    let task_token = cancellation.clone();
    let handler = harness.handler.clone();
    let task = tokio::spawn(async move {
        handler
            .delegate(fixture()["callback_url"].as_str().unwrap(), &task_token)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if harness
                .process
                .state
                .lock()
                .unwrap()
                .trace
                .contains(&"open".into())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .to_string(),
        "could not open previous Pixiv URL handler"
    );
}

#[test]
fn installation_looks_up_both_tools_before_home_and_persistent_starts_with_executable() {
    let harness = Harness::new("old.bundle", "swiftc");
    harness
        .handler
        .install(&Context::new(), "invalid endpoint")
        .err()
        .unwrap();
    assert_eq!(*harness.environment.events.lock().unwrap(), ["find:swiftc"]);
    let harness = Harness::new("old.bundle", "swift");
    harness
        .handler
        .install(&Context::new(), "invalid endpoint")
        .err()
        .unwrap();
    assert_eq!(
        *harness.environment.events.lock().unwrap(),
        ["find:swiftc", "find:swift"]
    );
    let harness = Harness::new("old.bundle", "compile");
    fs::create_dir_all(harness.store.path().parent().unwrap()).unwrap();
    fs::write(harness.store.path(), b"invalid manifest").unwrap();
    assert_eq!(
        harness
            .handler
            .ensure_persistent(&Context::new())
            .unwrap_err()
            .to_string(),
        "compile pixiv:// callback helper: synthetic exit: stdout\nstderr"
    );
    assert_eq!(
        *harness.environment.events.lock().unwrap(),
        ["executable", "home", "home"]
    );
    harness.source_cleaned();
}

#[test]
fn dropping_temporary_installation_removes_endpoint_and_restores_previous_bundle() {
    let harness = Harness::new("old.bundle", "");
    let installation = harness
        .handler
        .install(&Context::new(), fixture()["endpoint_url"].as_str().unwrap())
        .unwrap();
    assert!(harness.endpoint.exists());
    drop(installation);
    assert!(!harness.endpoint.exists());
    assert_eq!(harness.process.state.lock().unwrap().current, "old.bundle");
    assert!(!harness.store.path().exists());
}

#[tokio::test]
async fn dropping_previous_handler_future_cancels_the_offloaded_context() {
    let harness = Harness::new("old.bundle", "");
    harness.seed("old.bundle");
    harness.process.state.lock().unwrap().block_open = true;
    let handler = harness.handler.clone();
    let task = tokio::spawn(async move {
        handler
            .delegate(
                fixture()["callback_url"].as_str().unwrap(),
                &CancellationToken::new(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if harness
                .process
                .state
                .lock()
                .unwrap()
                .trace
                .contains(&"open".into())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if harness.process.state.lock().unwrap().canceled_open {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn temporary_cleanup_removes_empty_replacement_directory_and_preserves_nonempty_one() {
    for nonempty in [false, true] {
        let harness = Harness::new("old.bundle", "");
        let mut installation = harness
            .handler
            .install(&Context::new(), fixture()["endpoint_url"].as_str().unwrap())
            .unwrap();
        fs::remove_file(&harness.endpoint).unwrap();
        fs::create_dir(&harness.endpoint).unwrap();
        if nonempty {
            fs::write(harness.endpoint.join("retained"), b"do not remove").unwrap();
        }
        installation.cleanup();
        installation.cleanup();
        assert_eq!(harness.endpoint.exists(), nonempty);
        if nonempty {
            assert_eq!(
                fs::read(harness.endpoint.join("retained")).unwrap(),
                b"do not remove"
            );
        }
        let state = harness.process.state.lock().unwrap();
        assert_eq!(
            state
                .trace
                .iter()
                .filter(|event| event.starts_with("set:"))
                .cloned()
                .collect::<Vec<_>>(),
            [
                format!("set:{DARWIN_BUNDLE_ID}"),
                "set:old.bundle".into(),
                "set:old.bundle".into()
            ]
        );
    }
}
