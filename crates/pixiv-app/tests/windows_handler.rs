use pixiv_app::{
    callback_handler::PreviousHandler,
    handler_manifest::{HandlerManifest, HandlerManifestStore},
    host_context::ContextHostProcess,
    host_process::{HostPlatform, HostProcess, HostProcessError, ProcessOutput, ProcessStdio},
    lifecycle::{Context, ContextError},
    windows_handler::{
        PREVIOUS_WINDOWS_PROG_ID, PREVIOUS_WINDOWS_REGISTRY_KEY, WINDOWS_REGISTRY_KEY,
        WindowsEnvironment, WindowsHandler, WindowsHandlerError, windows_url_handler_command,
    },
    windows_shell::{WindowsShell, WindowsShellError},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/windows_handler.json")).unwrap()
}

fn status(code: i32) -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(code as u32)
    }
}

fn output(stdout: &[u8]) -> ProcessOutput {
    ProcessOutput {
        status: status(0),
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    }
}

fn exit_error() -> HostProcessError {
    HostProcessError::Exit {
        program: "reg.exe".into(),
        output: ProcessOutput {
            status: status(1),
            stdout: b"private registry stdout".to_vec(),
            stderr: b"private registry stderr".to_vec(),
        },
    }
}

fn assert_mode(path: &Path, expected: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            expected
        );
    }
    #[cfg(not(unix))]
    {
        let _ = (path, expected);
    }
}

struct Environment {
    executable: Mutex<PathBuf>,
    temporary: PathBuf,
    fail_executable: Mutex<bool>,
    events: Arc<Mutex<Vec<String>>>,
}
impl WindowsEnvironment for Environment {
    fn executable_path(&self) -> io::Result<PathBuf> {
        self.events.lock().unwrap().push("executable".into());
        if *self.fail_executable.lock().unwrap() {
            Err(io::Error::other("synthetic executable"))
        } else {
            Ok(self.executable.lock().unwrap().clone())
        }
    }
    fn temporary_directory(&self) -> PathBuf {
        self.temporary.clone()
    }
}

#[derive(Clone, Copy)]
enum FaultKind {
    Native,
    Exit,
    CapturedExit,
    Context,
}
struct Fault {
    label: String,
    kind: FaultKind,
    message: String,
}
#[derive(Debug, PartialEq, Eq)]
struct Call {
    args: Vec<String>,
    stdio: ProcessStdio,
    canceled: bool,
}
struct State {
    registry: BTreeMap<String, BTreeMap<String, String>>,
    fault: Option<Fault>,
    trace: Vec<String>,
    calls: Vec<Call>,
    backups: Vec<PathBuf>,
    block_save: Option<PathBuf>,
    owner_output: Option<Vec<u8>>,
    export_no_file: bool,
}
struct Process {
    state: Mutex<State>,
    temporary: PathBuf,
    events: Arc<Mutex<Vec<String>>>,
}

fn registry_label(args: &[String]) -> String {
    match args[0].as_str() {
        "query" if args.len() == 2 => "query".into(),
        "query" => "query-owner".into(),
        "copy" if args[1] == WINDOWS_REGISTRY_KEY => "copy-previous".into(),
        "copy" => "restore-previous".into(),
        "add" if args[1].ends_with(r"\shell\open\command") => "add-command".into(),
        "add" if args[2] == "/v" => "add-url".into(),
        "add" => "add-protocol".into(),
        "delete" if args[1] == PREVIOUS_WINDOWS_REGISTRY_KEY => "delete-previous".into(),
        "delete" => "delete-current".into(),
        operation => operation.into(),
    }
}

fn expected_arguments(label: &str, arguments: &[String]) -> Vec<String> {
    let command_key = format!(r"{WINDOWS_REGISTRY_KEY}\shell\open\command");
    let fixture = fixture();
    let command = fixture["command_cases"][0]["command"].as_str().unwrap();
    let values: Vec<&str> = match label {
        "query" => vec!["query", WINDOWS_REGISTRY_KEY],
        "query-owner" => vec!["query", &command_key, "/ve"],
        "copy-previous" => vec![
            "copy",
            WINDOWS_REGISTRY_KEY,
            PREVIOUS_WINDOWS_REGISTRY_KEY,
            "/s",
            "/f",
        ],
        "restore-previous" => vec![
            "copy",
            PREVIOUS_WINDOWS_REGISTRY_KEY,
            WINDOWS_REGISTRY_KEY,
            "/s",
            "/f",
        ],
        "add-protocol" => vec![
            "add",
            WINDOWS_REGISTRY_KEY,
            "/ve",
            "/t",
            "REG_SZ",
            "/d",
            "URL:Pixiv Protocol",
            "/f",
        ],
        "add-url" => vec![
            "add",
            WINDOWS_REGISTRY_KEY,
            "/v",
            "URL Protocol",
            "/t",
            "REG_SZ",
            "/d",
            "",
            "/f",
        ],
        "add-command" => vec![
            "add",
            &command_key,
            "/ve",
            "/t",
            "REG_SZ",
            "/d",
            command,
            "/f",
        ],
        "delete-current" => vec!["delete", WINDOWS_REGISTRY_KEY, "/f"],
        "delete-previous" => vec!["delete", PREVIOUS_WINDOWS_REGISTRY_KEY, "/f"],
        "export" => vec!["export", WINDOWS_REGISTRY_KEY, &arguments[2], "/y"],
        "import" => vec!["import", &arguments[1]],
        value => panic!("unexpected registry label {value}"),
    };
    values.into_iter().map(str::to_owned).collect()
}

impl HostProcess for Process {
    fn platform(&self) -> HostPlatform {
        HostPlatform::Windows
    }
    fn look_path(&self, _: &OsStr) -> Result<PathBuf, HostProcessError> {
        panic!("URL handler must call the Context process boundary directly")
    }
    fn run(
        &self,
        _: &OsStr,
        _: &[OsString],
        _: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        panic!("registry operations must use Context")
    }
    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("previous handlers must use their explicit class")
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
        assert_eq!(program, "reg.exe");
        let arguments = args
            .iter()
            .map(|arg| arg.to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let label = registry_label(&arguments);
        let expected = expected_arguments(&label, &arguments);
        assert_eq!(arguments, expected, "{label}");
        assert_eq!(
            stdio,
            if label == "query-owner" {
                ProcessStdio::CaptureStdout
            } else {
                ProcessStdio::Discard
            },
            "{label}"
        );
        let canceled = context.error().is_some();
        self.events.lock().unwrap().push(label.clone());
        let mut state = self.state.lock().unwrap();
        state.trace.push(label.clone());
        state.calls.push(Call {
            args: arguments.clone(),
            stdio,
            canceled,
        });
        if let Some(reason) = context.error() {
            return Err(HostProcessError::Context(reason));
        }
        if state
            .fault
            .as_ref()
            .is_some_and(|fault| fault.label == label)
        {
            let fault = state.fault.take().unwrap();
            return Err(match fault.kind {
                FaultKind::Native => HostProcessError::Native(io::Error::other(fault.message)),
                FaultKind::Exit => exit_error(),
                FaultKind::CapturedExit => HostProcessError::Captured {
                    source: Box::new(HostProcessError::Captured {
                        source: Box::new(exit_error()),
                        stdout: b"private inner output".to_vec(),
                        stderr: Vec::new(),
                    }),
                    stdout: b"private outer output".to_vec(),
                    stderr: Vec::new(),
                },
                FaultKind::Context => {
                    context.cancel();
                    HostProcessError::Context(ContextError::Canceled)
                }
            });
        }
        match label.as_str() {
            "query" => {
                assert_eq!(stdio, ProcessStdio::Discard);
                if state.registry.contains_key(&arguments[1]) {
                    Ok(output(b""))
                } else {
                    Err(exit_error())
                }
            }
            "query-owner" => {
                assert_eq!(stdio, ProcessStdio::CaptureStdout);
                let stdout = state.owner_output.clone().or_else(|| {
                    state
                        .registry
                        .get(WINDOWS_REGISTRY_KEY)
                        .and_then(|tree| tree.get("command"))
                        .map(|command| command.as_bytes().to_vec())
                });
                match stdout {
                    Some(stdout) => Ok(output(&stdout)),
                    None => Err(exit_error()),
                }
            }
            "copy-previous" | "restore-previous" => {
                assert_eq!(stdio, ProcessStdio::Discard);
                let tree = state.registry.get(&arguments[1]).cloned();
                match tree {
                    Some(tree) => {
                        state.registry.insert(arguments[2].clone(), tree);
                        Ok(output(b""))
                    }
                    None => Err(exit_error()),
                }
            }
            "add-protocol" | "add-url" | "add-command" => {
                assert_eq!(stdio, ProcessStdio::Discard);
                let field = match label.as_str() {
                    "add-protocol" => "description",
                    "add-url" => "protocol",
                    _ => "command",
                };
                let value =
                    arguments[arguments.iter().position(|arg| arg == "/d").unwrap() + 1].clone();
                state
                    .registry
                    .entry(WINDOWS_REGISTRY_KEY.into())
                    .or_default()
                    .insert(field.into(), value);
                if label == "add-command"
                    && let Some(path) = state.block_save.take()
                {
                    fs::create_dir_all(path).unwrap();
                }
                Ok(output(b""))
            }
            "delete-current" | "delete-previous" => {
                assert_eq!(stdio, ProcessStdio::Discard);
                if state.registry.remove(&arguments[1]).is_some() {
                    Ok(output(b""))
                } else {
                    Err(exit_error())
                }
            }
            "export" => {
                assert_eq!(stdio, ProcessStdio::Discard);
                let backup = PathBuf::from(&args[2]);
                let directory = backup.parent().unwrap();
                assert_eq!(directory.parent().unwrap(), self.temporary);
                assert!(
                    directory
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .starts_with("pixiv-cli-url-handler-registry-")
                );
                assert_eq!(backup.file_name().unwrap(), "pixiv-url-handler.reg");
                assert_mode(directory, 0o700);
                assert!(!backup.exists());
                if state.export_no_file {
                    return Ok(output(b""));
                }
                fs::write(
                    &backup,
                    serde_json::to_vec(state.registry.get(WINDOWS_REGISTRY_KEY).unwrap()).unwrap(),
                )
                .unwrap();
                state.backups.push(backup);
                Ok(output(b""))
            }
            "import" => {
                assert_eq!(stdio, ProcessStdio::Discard);
                let backup = PathBuf::from(&args[1]);
                if !backup.exists() {
                    return Err(exit_error());
                }
                assert_mode(backup.parent().unwrap(), 0o700);
                assert_mode(&backup, 0o600);
                assert!(fs::symlink_metadata(&backup).unwrap().file_type().is_file());
                let tree: BTreeMap<String, String> =
                    serde_json::from_slice(&fs::read(backup).unwrap()).unwrap();
                state.registry.insert(WINDOWS_REGISTRY_KEY.into(), tree);
                Ok(output(b""))
            }
            value => panic!("unexpected registry operation {value}"),
        }
    }
}

#[derive(Default)]
struct ShellState {
    calls: Vec<(String, String)>,
    fail: bool,
    block: bool,
    entered: bool,
    canceled: bool,
}
struct Shell {
    state: Mutex<ShellState>,
    events: Arc<Mutex<Vec<String>>>,
}
impl WindowsShell for Shell {
    fn open_class(
        &self,
        context: &Context,
        class_name: &str,
        raw_url: &str,
    ) -> Result<(), WindowsShellError> {
        self.events.lock().unwrap().push("open".into());
        let mut state = self.state.lock().unwrap();
        state.calls.push((class_name.into(), raw_url.into()));
        state.entered = true;
        if state.block {
            drop(state);
            while context.error().is_none() {
                std::thread::sleep(Duration::from_millis(1));
            }
            self.state.lock().unwrap().canceled = true;
            return Err(WindowsShellError::Context(context.error().unwrap()));
        }
        if let Some(reason) = context.error() {
            return Err(WindowsShellError::Context(reason));
        }
        if state.fail {
            Err(WindowsShellError::Native(io::Error::other(
                "private ShellExecute error",
            )))
        } else {
            Ok(())
        }
    }
}

struct Harness {
    _root: TempDir,
    environment: Arc<Environment>,
    process: Arc<Process>,
    shell: Arc<Shell>,
    handler: WindowsHandler,
    store: HandlerManifestStore,
    endpoint: PathBuf,
}
fn previous_tree() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("description".into(), "previous protocol".into()),
        (
            "command".into(),
            r#""C:\Previous App\pixiv.exe" "%1""#.into(),
        ),
        ("previous/marker".into(), "keep".into()),
        ("binary".into(), "00ff80".into()),
    ])
}
impl Harness {
    fn new(had_previous: bool) -> Self {
        let root = TempDir::new().unwrap();
        let temporary = root.path().join("tmp");
        fs::create_dir(&temporary).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let environment = Arc::new(Environment {
            executable: Mutex::new(PathBuf::from(r"C:\Program Files\pixiv\pixiv.exe")),
            temporary: temporary.clone(),
            fail_executable: Mutex::new(false),
            events: events.clone(),
        });
        let mut registry = BTreeMap::new();
        if had_previous {
            registry.insert(WINDOWS_REGISTRY_KEY.into(), previous_tree());
        }
        let process = Arc::new(Process {
            state: Mutex::new(State {
                registry,
                fault: None,
                trace: Vec::new(),
                calls: Vec::new(),
                backups: Vec::new(),
                block_save: None,
                owner_output: None,
                export_no_file: false,
            }),
            temporary,
            events: events.clone(),
        });
        let shell = Arc::new(Shell {
            state: Mutex::new(ShellState::default()),
            events,
        });
        let store = HandlerManifestStore::new(root.path().join("handler/handler-manifest.json"));
        let endpoint = root.path().join("url-handler-endpoint");
        let handler = WindowsHandler::new(
            environment.clone(),
            process.clone(),
            shell.clone(),
            store.clone(),
            &endpoint,
        );
        Self {
            _root: root,
            environment,
            process,
            shell,
            handler,
            store,
            endpoint,
        }
    }
    fn fault(&self, label: &str, kind: FaultKind) {
        self.process.state.lock().unwrap().fault = Some(Fault {
            label: label.into(),
            kind,
            message: format!("synthetic {label}"),
        });
    }
    fn seed(&self, previous: &str) {
        self.store
            .save(&HandlerManifest {
                version: 1,
                executable_path: r"C:\old\pixiv.exe".into(),
                previous_handler: previous.into(),
                ..HandlerManifest::default()
            })
            .unwrap();
        if !previous.is_empty() {
            self.process
                .state
                .lock()
                .unwrap()
                .registry
                .insert(PREVIOUS_WINDOWS_REGISTRY_KEY.into(), previous_tree());
        }
    }
    fn temporary_cleaned(&self) {
        if self.environment.temporary.is_dir() {
            assert_eq!(
                fs::read_dir(&self.environment.temporary).unwrap().count(),
                0
            );
        }
        for backup in &self.process.state.lock().unwrap().backups {
            assert!(!backup.parent().unwrap().exists());
        }
    }
}

fn io_source(mut error: &(dyn std::error::Error + 'static)) -> bool {
    loop {
        if error.downcast_ref::<io::Error>().is_some() {
            return true;
        }
        match error.source() {
            Some(source) => error = source,
            None => return false,
        }
    }
}

fn assert_result(result: Result<(), WindowsHandlerError>, case: &Value) {
    let name = case["name"].as_str().unwrap();
    if case["error_kind"] == "filesystem" {
        let error = result.unwrap_err();
        assert!(io_source(&error), "{name}: {error}");
    } else {
        assert_eq!(
            result
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default(),
            case["error"].as_str().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn persistent_temporary_and_delegation_flows_match_frozen_go() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let operation = case["operation"].as_str().unwrap();
        let previous = case["previous"].as_bool().unwrap();
        let failure = case["failure"].as_str().unwrap();
        let harness = Harness::new(previous);
        let context = Context::new();
        let initial_manifest = case["initial_manifest"].as_bool().unwrap();
        if initial_manifest {
            harness.seed(if previous {
                PREVIOUS_WINDOWS_PROG_ID
            } else {
                ""
            });
        }
        match case["current"].as_str().unwrap() {
            "ours" => {
                harness.process.state.lock().unwrap().registry.insert(
                    WINDOWS_REGISTRY_KEY.into(),
                    BTreeMap::from([(
                        "command".into(),
                        fixture["command_cases"][0]["command"]
                            .as_str()
                            .unwrap()
                            .into(),
                    )]),
                );
            }
            "other" => {
                harness.process.state.lock().unwrap().registry.insert(
                    WINDOWS_REGISTRY_KEY.into(),
                    BTreeMap::from([("command".into(), r#""other.exe" "%1""#.into())]),
                );
            }
            "absent" => {
                harness
                    .process
                    .state
                    .lock()
                    .unwrap()
                    .registry
                    .remove(WINDOWS_REGISTRY_KEY);
            }
            "" => {}
            value => panic!("unknown current handler {value}"),
        }
        if operation == "repeat" {
            harness.handler.ensure_persistent(&context).unwrap();
            let mut manifest = harness.store.load().unwrap().unwrap();
            manifest.executable_path = r"C:\stale\pixiv.exe".into();
            harness.store.save(&manifest).unwrap();
            harness.environment.events.lock().unwrap().clear();
            let mut state = harness.process.state.lock().unwrap();
            state.trace.clear();
            state.calls.clear();
        }
        match failure {
            "" => {}
            "resolve" => *harness.environment.fail_executable.lock().unwrap() = true,
            "save" => {
                harness.process.state.lock().unwrap().block_save = Some(harness.store.path().into())
            }
            "invalid-manifest" => {
                fs::create_dir_all(harness.store.path().parent().unwrap()).unwrap();
                fs::write(harness.store.path(), b"{}").unwrap();
            }
            "tempdir" => {
                fs::remove_dir(&harness.environment.temporary).unwrap();
                fs::write(&harness.environment.temporary, b"keep temporary blocker").unwrap();
            }
            "export-no-file" => harness.process.state.lock().unwrap().export_no_file = true,
            "open" => harness.shell.state.lock().unwrap().fail = true,
            label => harness.fault(label, FaultKind::Native),
        }
        let result = match operation {
            "ensure" | "repeat" => harness.handler.ensure_persistent(&context),
            "disable" => harness.handler.disable_persistent(&context),
            "delegate" => harness
                .handler
                .delegate_previous(&context, fixture["callback_url"].as_str().unwrap()),
            "install" => harness
                .handler
                .install(&context, fixture["endpoint_url"].as_str().unwrap())
                .map(|mut installation| {
                    assert_eq!(
                        fs::read(&harness.endpoint).unwrap(),
                        format!("{}\n", fixture["endpoint_url"].as_str().unwrap()).as_bytes(),
                        "{name}"
                    );
                    assert_mode(&harness.endpoint, 0o600);
                    for backup in &harness.process.state.lock().unwrap().backups {
                        assert_mode(backup.parent().unwrap(), 0o700);
                        assert_mode(backup, 0o600);
                        assert_eq!(
                            serde_json::from_slice::<BTreeMap<String, String>>(
                                &fs::read(backup).unwrap()
                            )
                            .unwrap(),
                            previous_tree(),
                            "{name}"
                        );
                    }
                    if let Some(replacement) = case["endpoint_replacement"].as_str() {
                        fs::remove_file(&harness.endpoint).unwrap();
                        fs::create_dir(&harness.endpoint).unwrap();
                        if replacement == "nonempty-directory" {
                            fs::write(harness.endpoint.join("retained"), b"keep endpoint child")
                                .unwrap();
                        }
                    }
                    for _ in 0..case["cleanup_count"].as_u64().unwrap_or(1) {
                        installation.cleanup();
                    }
                }),
            value => panic!("unknown operation {value}"),
        };
        assert_result(result, case);
        assert_eq!(
            harness
                .environment
                .events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.as_str() != "executable")
                .cloned()
                .collect::<Vec<_>>(),
            case["trace"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            "{name}"
        );
        assert_eq!(
            harness.store.path().exists(),
            case["manifest_exists"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(
            harness.endpoint.exists(),
            case["endpoint_exists"].as_bool().unwrap(),
            "{name}"
        );
        if (operation == "ensure" || operation == "repeat") && failure.is_empty() {
            let manifest = harness.store.load().unwrap().unwrap();
            assert_eq!(manifest.version, 1, "{name}");
            assert_eq!(manifest.executable_path, fixture["executable"], "{name}");
            assert_eq!(
                manifest.previous_handler,
                if previous {
                    PREVIOUS_WINDOWS_PROG_ID
                } else {
                    ""
                },
                "{name}"
            );
            assert!(manifest.home_directory.is_empty(), "{name}");
            assert!(manifest.linux_mime_snapshots.is_none(), "{name}");
            assert_mode(harness.store.path(), 0o600);
        }
        if operation == "delegate" && initial_manifest && previous {
            assert_eq!(
                harness.shell.state.lock().unwrap().calls,
                [(
                    PREVIOUS_WINDOWS_PROG_ID.into(),
                    fixture["callback_url"].as_str().unwrap().into()
                )],
                "{name}"
            );
            assert!(
                harness.process.state.lock().unwrap().calls.is_empty(),
                "{name}"
            );
        }
        if case["endpoint_replacement"] == "nonempty-directory" {
            assert_eq!(
                fs::read(harness.endpoint.join("retained")).unwrap(),
                b"keep endpoint child"
            );
        }
        assert_registry_residue(&harness, case);
        harness.temporary_cleaned();
    }
}

fn assert_registry_residue(harness: &Harness, case: &Value) {
    let name = case["name"].as_str().unwrap();
    let state = harness.process.state.lock().unwrap();
    let current = state.registry.get(WINDOWS_REGISTRY_KEY);
    let current_tree = match current {
        None => "absent",
        Some(tree)
            if tree
                .get("description")
                .is_some_and(|value| value == "URL:Pixiv Protocol")
                || tree
                    .get("command")
                    .is_some_and(|command| command.contains(" auth _callback ")) =>
        {
            "ours"
        }
        Some(tree) if tree.get("command") == previous_tree().get("command") => "previous",
        Some(_) => "other",
    };
    assert_eq!(
        current_tree,
        case["current_tree"].as_str().unwrap(),
        "{name}"
    );
    assert_eq!(
        state.registry.contains_key(PREVIOUS_WINDOWS_REGISTRY_KEY),
        case["previous_tree"].as_bool().unwrap(),
        "{name}"
    );
    if let Some(tree) = state.registry.get(PREVIOUS_WINDOWS_REGISTRY_KEY) {
        assert_eq!(*tree, previous_tree(), "{name}");
    }
    if current_tree == "previous"
        && (case["operation"] == "disable" || case["operation"] == "install")
    {
        assert_eq!(*current.unwrap(), previous_tree(), "{name}");
    }
}

#[test]
fn registry_constants_and_windows_command_quoting_match_frozen_go() {
    let fixture = fixture();
    assert_eq!(WINDOWS_REGISTRY_KEY, fixture["registry_key"]);
    assert_eq!(
        PREVIOUS_WINDOWS_REGISTRY_KEY,
        fixture["previous_registry_key"]
    );
    assert_eq!(PREVIOUS_WINDOWS_PROG_ID, fixture["previous_prog_id"]);
    for case in fixture["command_cases"].as_array().unwrap() {
        assert_eq!(
            windows_url_handler_command(case["executable"].as_str().unwrap()),
            case["command"].as_str().unwrap(),
            "{}",
            case["name"]
        );
    }
}

fn set_query_fault(harness: &Harness, label: &str, failure: &str) {
    let kind = match failure {
        "" => return,
        "exit" => FaultKind::Exit,
        "wrapped-exit" => FaultKind::CapturedExit,
        "native" => FaultKind::Native,
        value => panic!("unknown query failure {value}"),
    };
    harness.fault(label, kind);
    harness
        .process
        .state
        .lock()
        .unwrap()
        .fault
        .as_mut()
        .unwrap()
        .message = "synthetic native".into();
}

#[test]
fn initial_key_query_treats_only_exit_errors_as_absence() {
    for case in fixture()["key_cases"].as_array().unwrap() {
        let harness = Harness::new(true);
        set_query_fault(&harness, "query", case["failure"].as_str().unwrap());
        let result = harness.handler.ensure_persistent(&Context::new());
        if case["failure"] == "native" {
            assert!(matches!(
                &result,
                Err(WindowsHandlerError::Registry(HostProcessError::Native(_)))
            ));
        }
        assert_result(result, case);
        if case["failure"] != "native" {
            assert_eq!(
                !harness
                    .store
                    .load()
                    .unwrap()
                    .unwrap()
                    .previous_handler
                    .is_empty(),
                case["exists"].as_bool().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
}

#[test]
fn invalid_endpoint_stops_before_executable_and_registry_access() {
    let harness = Harness::new(true);
    *harness.environment.fail_executable.lock().unwrap() = true;
    let error = harness
        .handler
        .install(&Context::new(), "invalid callback endpoint")
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "invalid callback endpoint");
    assert!(harness.environment.events.lock().unwrap().is_empty());
    assert!(!harness.endpoint.exists());
    harness.temporary_cleaned();
}

#[test]
fn canceled_install_restores_the_full_previous_tree_with_a_fresh_context() {
    let harness = Harness::new(true);
    harness.fault("add-command", FaultKind::Context);
    let context = Context::new();
    let error = harness
        .handler
        .install(&context, fixture()["endpoint_url"].as_str().unwrap())
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "run Windows registry command: context canceled"
    );
    assert_eq!(context.error(), Some(ContextError::Canceled));
    let state = harness.process.state.lock().unwrap();
    assert_eq!(
        state.registry.get(WINDOWS_REGISTRY_KEY),
        Some(&previous_tree())
    );
    assert_eq!(
        state.trace[state.trace.len() - 2..],
        ["delete-current", "import"]
    );
    assert!(state.calls.iter().all(|call| !call.canceled));
    drop(state);
    assert!(!harness.endpoint.exists());
    harness.temporary_cleaned();
}

#[test]
fn dropping_temporary_installation_cleans_endpoint_and_restores_once() {
    let harness = Harness::new(true);
    let context = Context::new();
    let installation = harness
        .handler
        .install(&context, fixture()["endpoint_url"].as_str().unwrap())
        .unwrap();
    context.cancel();
    assert!(harness.endpoint.exists());
    drop(installation);
    assert!(!harness.endpoint.exists());
    let state = harness.process.state.lock().unwrap();
    assert_eq!(
        state.registry.get(WINDOWS_REGISTRY_KEY),
        Some(&previous_tree())
    );
    assert_eq!(
        state
            .trace
            .iter()
            .filter(|event| event.as_str() == "import")
            .count(),
        1
    );
    assert!(state.calls.iter().all(|call| !call.canceled));
    drop(state);
    harness.temporary_cleaned();
}

async fn shell_entered(shell: &Shell) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !shell.state.lock().unwrap().entered {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

async fn shell_canceled(shell: &Shell) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !shell.state.lock().unwrap().canceled {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cancellation_reaches_the_running_previous_handler_worker() {
    let harness = Harness::new(true);
    harness.seed(PREVIOUS_WINDOWS_PROG_ID);
    harness.shell.state.lock().unwrap().block = true;
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let handler = harness.handler.clone();
    let task = tokio::spawn(async move {
        handler
            .delegate(
                fixture()["callback_url"].as_str().unwrap(),
                &task_cancellation,
            )
            .await
    });
    shell_entered(&harness.shell).await;
    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "could not open previous Pixiv URL handler"
    );
    shell_canceled(&harness.shell).await;
    assert!(harness.store.path().exists());
    assert!(harness.process.state.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn dropping_previous_handler_future_cancels_the_offloaded_context() {
    let harness = Harness::new(true);
    harness.seed(PREVIOUS_WINDOWS_PROG_ID);
    harness.shell.state.lock().unwrap().block = true;
    let handler = harness.handler.clone();
    let task = tokio::spawn(async move {
        handler
            .delegate(
                fixture()["callback_url"].as_str().unwrap(),
                &CancellationToken::new(),
            )
            .await
    });
    shell_entered(&harness.shell).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    shell_canceled(&harness.shell).await;
    assert!(harness.store.path().exists());
    assert!(harness.process.state.lock().unwrap().calls.is_empty());
}

#[test]
fn ownership_query_preserves_go_case_insensitive_substring_and_error_classification() {
    for case in fixture()["owner_cases"].as_array().unwrap() {
        let harness = Harness::new(true);
        harness.seed("");
        harness.process.state.lock().unwrap().owner_output =
            Some(case["output"].as_str().unwrap().as_bytes().to_vec());
        set_query_fault(&harness, "query-owner", case["failure"].as_str().unwrap());
        let result = harness.handler.disable_persistent(&Context::new());
        if case["failure"] == "native" {
            assert!(matches!(
                &result,
                Err(WindowsHandlerError::Process(HostProcessError::Native(_)))
            ));
        }
        assert_result(result, case);
        assert_eq!(
            harness
                .process
                .state
                .lock()
                .unwrap()
                .trace
                .contains(&"delete-current".into()),
            case["ours"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            harness.store.path().exists(),
            case["failure"] == "native",
            "{}",
            case["name"]
        );
    }
}

#[derive(Default)]
struct NativeArgumentProcess {
    calls: Mutex<Vec<Vec<OsString>>>,
}

impl HostProcess for NativeArgumentProcess {
    fn platform(&self) -> HostPlatform {
        HostPlatform::Windows
    }
    fn look_path(&self, _: &OsStr) -> Result<PathBuf, HostProcessError> {
        panic!("native registry arguments must use the Context process boundary")
    }
    fn run(
        &self,
        _: &OsStr,
        _: &[OsString],
        _: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        panic!("native registry arguments must use Context")
    }
    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("registry installation must not open a URL")
    }
}

impl ContextHostProcess for NativeArgumentProcess {
    fn run_context(
        &self,
        context: &Context,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        assert!(context.error().is_none());
        assert_eq!(program, "reg.exe");
        assert_eq!(stdio, ProcessStdio::Discard);
        self.calls.lock().unwrap().push(args.to_vec());
        if args
            == [
                OsString::from("query"),
                OsString::from(WINDOWS_REGISTRY_KEY),
            ]
        {
            Err(exit_error())
        } else {
            Ok(output(b""))
        }
    }
}

#[cfg(unix)]
#[test]
fn native_executable_bytes_reach_registry_while_manifest_matches_frozen_go_json() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let executable = b"C:\\path\\\xed\xa0\x80\xff\"quoted\"\\pixiv.exe";
    let command = b"\"C:\\path\\\xed\xa0\x80\xff\\\"quoted\\\"\\pixiv.exe\" auth _callback \"%1\"";
    let manifest_executable = "C:\\path\\\u{fffd}\u{fffd}\u{fffd}\u{fffd}\"quoted\"\\pixiv.exe";
    let manifest_json = r#"{"version":1,"executable_path":"C:\\path\\����\"quoted\"\\pixiv.exe"}"#;
    for operation in ["ensure", "install"] {
        let harness = Harness::new(false);
        *harness.environment.executable.lock().unwrap() =
            PathBuf::from(OsString::from_vec(executable.to_vec()));
        let process = Arc::new(NativeArgumentProcess::default());
        let handler = WindowsHandler::new(
            harness.environment.clone(),
            process.clone(),
            harness.shell.clone(),
            harness.store.clone(),
            &harness.endpoint,
        );
        match operation {
            "ensure" => {
                handler.ensure_persistent(&Context::new()).unwrap();
                assert_eq!(
                    harness.store.load().unwrap().unwrap().executable_path,
                    manifest_executable
                );
                assert_eq!(
                    fs::read(harness.store.path()).unwrap(),
                    manifest_json.as_bytes()
                );
            }
            "install" => {
                let mut installation = handler
                    .install(&Context::new(), fixture()["endpoint_url"].as_str().unwrap())
                    .unwrap();
                assert!(harness.endpoint.exists());
                installation.cleanup();
                assert!(!harness.store.path().exists());
                assert!(!harness.endpoint.exists());
                harness.temporary_cleaned();
            }
            _ => unreachable!(),
        }
        assert_native_registry_calls(&process, OsStr::from_bytes(command).into(), operation);
    }
}

fn assert_native_registry_calls(
    process: &NativeArgumentProcess,
    command: OsString,
    operation: &str,
) {
    let command_key = format!(r"{WINDOWS_REGISTRY_KEY}\shell\open\command");
    let mut expected: Vec<Vec<OsString>> = [
        vec!["query", WINDOWS_REGISTRY_KEY],
        vec![
            "add",
            WINDOWS_REGISTRY_KEY,
            "/ve",
            "/t",
            "REG_SZ",
            "/d",
            "URL:Pixiv Protocol",
            "/f",
        ],
        vec![
            "add",
            WINDOWS_REGISTRY_KEY,
            "/v",
            "URL Protocol",
            "/t",
            "REG_SZ",
            "/d",
            "",
            "/f",
        ],
    ]
    .into_iter()
    .map(|args| args.into_iter().map(OsString::from).collect())
    .collect();
    expected.push(vec![
        "add".into(),
        command_key.into(),
        "/ve".into(),
        "/t".into(),
        "REG_SZ".into(),
        "/d".into(),
        command,
        "/f".into(),
    ]);
    if operation == "install" {
        expected.push(vec![
            "delete".into(),
            WINDOWS_REGISTRY_KEY.into(),
            "/f".into(),
        ]);
    }
    assert_eq!(*process.calls.lock().unwrap(), expected, "{operation}");
}

#[cfg(windows)]
#[test]
fn native_windows_registry_command_preserves_unpaired_surrogate_and_escapes_embedded_quote() {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let executable = OsString::from_wide(&[0x43, 0x3a, 0x5c, 0xd800, 0x22]);
    let mut expected = vec![0x22, 0x43, 0x3a, 0x5c, 0xd800, 0x5c, 0x22];
    expected.extend("\" auth _callback \"%1\"".encode_utf16());
    let command = OsString::from_wide(&expected);
    for operation in ["ensure", "install"] {
        let harness = Harness::new(false);
        *harness.environment.executable.lock().unwrap() = PathBuf::from(executable.clone());
        let process = Arc::new(NativeArgumentProcess::default());
        let handler = WindowsHandler::new(
            harness.environment.clone(),
            process.clone(),
            harness.shell.clone(),
            harness.store.clone(),
            &harness.endpoint,
        );
        match operation {
            "ensure" => {
                handler.ensure_persistent(&Context::new()).unwrap();
                assert_eq!(
                    harness.store.load().unwrap().unwrap().executable_path,
                    "C:\\\u{fffd}\u{fffd}\u{fffd}\""
                );
            }
            "install" => {
                let mut installation = handler
                    .install(&Context::new(), fixture()["endpoint_url"].as_str().unwrap())
                    .unwrap();
                installation.cleanup();
                assert!(!harness.store.path().exists());
                assert!(!harness.endpoint.exists());
                harness.temporary_cleaned();
            }
            _ => unreachable!(),
        }
        assert_native_registry_calls(&process, command.clone(), operation);
        assert_eq!(
            process.calls.lock().unwrap()[3][6]
                .encode_wide()
                .collect::<Vec<_>>(),
            expected,
            "{operation}"
        );
    }
}
