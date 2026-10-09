#![cfg(unix)]
use pixiv_app::{
    handler_manifest::{HandlerFileSnapshot, HandlerManifest, HandlerManifestStore},
    host_context::ContextHostProcess,
    host_process::{HostPlatform, HostProcess, HostProcessError, ProcessOutput, ProcessStdio},
    lifecycle::Context,
    linux_handler::{
        LINUX_DESKTOP_FILE, LINUX_PIXIV_SCHEME, LinuxEnvironment, LinuxHandler, LinuxHandlerError,
        desktop_entry,
    },
};
use serde_json::Value;
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs, io,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt, process::ExitStatusExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tempfile::TempDir;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/linux_handler.json")).unwrap()
}
struct Environment {
    home: Option<PathBuf>,
    variables: HashMap<String, OsString>,
    executable: Mutex<Result<PathBuf, &'static str>>,
    events: Arc<Mutex<Vec<String>>>,
}
impl LinuxEnvironment for Environment {
    fn variable(&self, name: &str) -> Option<OsString> {
        self.variables.get(name).cloned()
    }
    fn home_directory(&self) -> io::Result<PathBuf> {
        self.home
            .clone()
            .ok_or_else(|| io::Error::other("$HOME is not defined"))
    }
    fn executable_path(&self) -> io::Result<PathBuf> {
        self.events.lock().unwrap().push("executable".into());
        self.executable
            .lock()
            .unwrap()
            .clone()
            .map_err(io::Error::other)
    }
}
#[derive(Default)]
struct State {
    current: String,
    missing: Option<String>,
    query_fail: bool,
    default_fail: bool,
    launch_fail: bool,
    block_launch: bool,
    block_manifest: Option<PathBuf>,
    calls: Vec<(String, Vec<String>, ProcessStdio)>,
}
struct Process {
    state: Mutex<State>,
    events: Arc<Mutex<Vec<String>>>,
    mime_paths: Vec<PathBuf>,
}
fn failure(program: &OsStr) -> HostProcessError {
    HostProcessError::Exit {
        program: program.to_owned(),
        output: ProcessOutput {
            status: std::process::ExitStatus::from_raw(7 << 8),
            stdout: b"sensitive diagnostic".to_vec(),
            stderr: b"private environment".to_vec(),
        },
    }
}
impl HostProcess for Process {
    fn platform(&self) -> HostPlatform {
        HostPlatform::Linux
    }
    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("find:{}", program.to_string_lossy()));
        if self.state.lock().unwrap().missing.as_deref() == program.to_str() {
            return Err(HostProcessError::Lookup {
                program: program.to_owned(),
                source: io::Error::from(io::ErrorKind::NotFound),
            });
        }
        Ok(Path::new("/synthetic/bin").join(program))
    }
    fn run(
        &self,
        _: &OsStr,
        _: &[OsString],
        _: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        panic!("Linux association commands must use Context")
    }
    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("Linux associations must not use shell/browser opening")
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
        let args: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let mut state = self.state.lock().unwrap();
        state
            .calls
            .push((program.to_string_lossy().into_owned(), args.clone(), stdio));
        self.events.lock().unwrap().push(
            if args[0] == "query" {
                "query"
            } else if program == "gio" {
                "launch"
            } else {
                "default"
            }
            .into(),
        );
        if let Some(error) = context.error() {
            return Err(HostProcessError::Context(error));
        }
        let stdout = if args[0] == "query" {
            assert_eq!(stdio, ProcessStdio::CaptureStdout);
            if state.query_fail {
                return Err(failure(program));
            }
            format!("\u{2003}{}\n", state.current).into_bytes()
        } else if program == "gio" {
            assert_eq!(stdio, ProcessStdio::Discard);
            if state.launch_fail {
                return Err(failure(program));
            }
            if state.block_launch {
                loop {
                    if let Some(error) = context.error() {
                        self.events.lock().unwrap().push("launch-canceled".into());
                        return Err(HostProcessError::Context(error));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            Vec::new()
        } else {
            assert_eq!(stdio, ProcessStdio::Discard);
            state.current = args[1].clone();
            for path in &self.mime_paths {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"association changed").unwrap();
            }
            if let Some(path) = &state.block_manifest {
                fs::create_dir_all(path).unwrap();
            }
            if state.default_fail {
                return Err(failure(program));
            }
            Vec::new()
        };
        Ok(ProcessOutput {
            status: std::process::ExitStatus::from_raw(0),
            stdout,
            stderr: Vec::new(),
        })
    }
}
struct Harness {
    _root: TempDir,
    home: PathBuf,
    environment: Arc<Environment>,
    process: Arc<Process>,
    handler: LinuxHandler,
    store: HandlerManifestStore,
    endpoint: PathBuf,
    desktop: PathBuf,
    mime: Vec<PathBuf>,
    events: Arc<Mutex<Vec<String>>>,
}
impl Harness {
    fn new() -> Self {
        Self::with_paths(false, false)
    }
    fn with_paths(dedup: bool, no_home: bool) -> Self {
        Self::with_leaf(dedup, no_home, None)
    }
    fn with_leaf(dedup: bool, no_home: bool, leaf: Option<&[u8]>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let leaf_root = leaf
            .map(|bytes| home.join(OsString::from_vec(bytes.to_vec())))
            .unwrap_or_else(|| home.clone());
        let data = leaf_root.join("data");
        let config = if dedup {
            data.join("applications/.")
        } else {
            leaf_root.join("config")
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let environment = Arc::new(Environment {
            home: (!no_home).then(|| home.clone()),
            variables: HashMap::from([
                ("XDG_CONFIG_HOME".into(), config.clone().into_os_string()),
                ("XDG_DATA_HOME".into(), data.clone().into_os_string()),
                ("XDG_DATA_DIRS".into(), home.join("system").into_os_string()),
            ]),
            executable: Mutex::new(Ok(PathBuf::from("/synthetic/pixiv"))),
            events: events.clone(),
        });
        let mime = vec![
            config.join("mimeapps.list"),
            data.join("applications/mimeapps.list"),
        ];
        let process = Arc::new(Process {
            state: Mutex::new(State {
                current: "previous.desktop".into(),
                ..State::default()
            }),
            events: events.clone(),
            mime_paths: mime.clone(),
        });
        let store =
            HandlerManifestStore::new(home.join(".pixiv-cli/url-handler/handler-manifest.json"));
        let endpoint = home.join(".pixiv-cli/url-handler-endpoint");
        let desktop = data.join("applications").join(LINUX_DESKTOP_FILE);
        let handler = LinuxHandler::new(
            environment.clone(),
            process.clone(),
            store.clone(),
            endpoint.clone(),
        );
        Self {
            _root: root,
            home,
            environment,
            process,
            handler,
            store,
            endpoint,
            desktop,
            mime,
            events,
        }
    }
    fn put(&self, path: &Path, body: &[u8], mode: u32) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    fn legacy(&self, previous: &str) {
        self.store
            .save(&HandlerManifest {
                version: 1,
                executable_path: "/old/pixiv".into(),
                previous_handler: previous.into(),
                ..HandlerManifest::default()
            })
            .unwrap();
        self.put(&self.desktop, b"old desktop", 0o640);
        self.process.state.lock().unwrap().current = LINUX_DESKTOP_FILE.into();
    }
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
    fn clear(&self) {
        self.events.lock().unwrap().clear();
        self.process.state.lock().unwrap().calls.clear();
    }
}
#[test]
fn desktop_entry_and_id_resolution_follow_shared_go_contract() {
    let f = fixture();
    assert_eq!(
        desktop_entry(f["executable"].as_str().unwrap()),
        f["entry"].as_str().unwrap()
    );
    assert_eq!(LINUX_DESKTOP_FILE, f["desktop_id"]);
    assert_eq!(LINUX_PIXIV_SCHEME, f["scheme"]);
    let h = Harness::new();
    for id in f["invalid_ids"].as_array().unwrap() {
        assert_eq!(
            h.handler
                .desktop_file_path(id.as_str().unwrap())
                .unwrap_err()
                .to_string(),
            "invalid desktop handler ID"
        );
    }
    let system = h.home.join("system/applications/PREVIOUS.DESKTOP");
    h.put(&system, b"system entry", 0o644);
    assert_eq!(
        h.handler.desktop_file_path(" PREVIOUS.DESKTOP ").unwrap(),
        system
    );
    let user = h.desktop.with_file_name("PREVIOUS.DESKTOP");
    fs::create_dir_all(user.parent().unwrap()).unwrap();
    fs::create_dir(&user).unwrap();
    assert_eq!(
        h.handler.desktop_file_path("PREVIOUS.DESKTOP").unwrap(),
        system
    );
    fs::remove_dir(&user).unwrap();
    std::os::unix::fs::symlink(&system, &user).unwrap();
    assert_eq!(
        h.handler.desktop_file_path("PREVIOUS.DESKTOP").unwrap(),
        user
    );
}
#[test]
fn first_repeat_and_disable_preserve_initial_bytes_modes_and_manifest() {
    let h = Harness::new();
    h.put(&h.mime[0], &[b'a', 0, 255], 0o641);
    h.put(&h.desktop, b"prior desktop", 0o644);
    h.handler.ensure_persistent(&Context::new()).unwrap();
    assert_eq!(
        h.events(),
        fixture()["ensure_first_order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
    let first = h.store.load().unwrap().unwrap();
    let snapshots = first.linux_mime_snapshots.as_ref().unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| Path::new(&snapshot.path)
                .strip_prefix(&h.home)
                .unwrap()
                .to_string_lossy()
                .into_owned())
            .collect::<Vec<_>>(),
        fixture()["snapshot_order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        snapshots[0].content.as_deref(),
        Some([b'a', 0, 255].as_slice())
    );
    assert_eq!(snapshots[0].mode, 0o641);
    assert_eq!(
        fs::metadata(&h.desktop).unwrap().permissions().mode() & 0o777,
        0o600
    );
    h.clear();
    *h.environment.executable.lock().unwrap() = Ok("/updated/pixiv".into());
    h.process.state.lock().unwrap().current = "external.desktop".into();
    h.handler.ensure_persistent(&Context::new()).unwrap();
    assert_eq!(
        h.events(),
        fixture()["ensure_repeat_order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
    let repeated = h.store.load().unwrap().unwrap();
    assert_eq!(repeated.executable_path, "/updated/pixiv");
    assert_eq!(repeated.previous_handler, first.previous_handler);
    assert_eq!(repeated.linux_mime_snapshots, first.linux_mime_snapshots);
    h.clear();
    h.handler.disable_persistent(&Context::new()).unwrap();
    assert_eq!(h.events(), ["query"]);
    assert_eq!(fs::read(&h.mime[0]).unwrap(), [b'a', 0, 255]);
    assert_eq!(
        fs::metadata(&h.mime[0]).unwrap().permissions().mode() & 0o777,
        0o641
    );
    assert!(!h.mime[1].exists());
    assert_eq!(fs::read(&h.desktop).unwrap(), b"prior desktop");
    assert!(h.store.load().unwrap().is_none());
}
#[test]
fn external_default_is_untouched_and_legacy_disable_cases_match_fixture() {
    let h = Harness::new();
    h.handler.ensure_persistent(&Context::new()).unwrap();
    h.clear();
    h.process.state.lock().unwrap().current = "external.desktop".into();
    let desktop = fs::read(&h.desktop).unwrap();
    h.handler.disable_persistent(&Context::new()).unwrap();
    assert_eq!(h.events(), ["query"]);
    assert_eq!(fs::read(&h.desktop).unwrap(), desktop);
    assert!(h.store.load().unwrap().is_none());
    for case in fixture()["legacy_disable"].as_array().unwrap() {
        let h = Harness::new();
        h.legacy(case["previous"].as_str().unwrap());
        let result = h.handler.disable_persistent(&Context::new());
        if let Some(error) = case["error"].as_str() {
            assert_eq!(result.unwrap_err().to_string(), error);
        } else {
            result.unwrap();
        }
        assert_eq!(
            !h.store.path().exists(),
            case["manifest_removed"].as_bool().unwrap()
        );
        assert_eq!(
            !h.desktop.exists(),
            case["desktop_removed"].as_bool().unwrap()
        );
        let calls: Vec<_> = h
            .process
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(_, args, _)| args[0] == "default")
            .map(|(_, args, _)| args.clone())
            .collect();
        if case.get("calls").is_some() {
            assert_eq!(serde_json::to_value(calls).unwrap(), case["calls"]);
        }
    }
}
#[test]
fn failures_preserve_original_manifest_and_use_initial_rollback() {
    let h = Harness::new();
    h.put(&h.mime[0], b"original", 0o641);
    h.handler.ensure_persistent(&Context::new()).unwrap();
    let manifest = fs::read(h.store.path()).unwrap();
    h.put(&h.mime[0], b"changed since install", 0o600);
    h.process.state.lock().unwrap().default_fail = true;
    let error = h.handler.ensure_persistent(&Context::new()).unwrap_err();
    assert_eq!(error.to_string(), "run xdg-mime: exit status 7");
    assert!(!error.to_string().contains("sensitive"));
    assert_eq!(fs::read(h.store.path()).unwrap(), manifest);
    assert_eq!(fs::read(&h.mime[0]).unwrap(), b"original");
    assert!(!h.desktop.exists());
    let h = Harness::new();
    h.legacy("previous.desktop");
    let manifest = fs::read(h.store.path()).unwrap();
    h.process.state.lock().unwrap().default_fail = true;
    h.clear();
    assert!(h.handler.ensure_persistent(&Context::new()).is_err());
    assert_eq!(
        h.events(),
        ["find:xdg-mime", "find:gio", "executable", "default"]
    );
    assert!(
        fs::read(&h.desktop)
            .unwrap()
            .starts_with(b"[Desktop Entry]")
    );
    assert_eq!(fs::read(h.store.path()).unwrap(), manifest);
    let h = Harness::new();
    fs::create_dir_all(h.store.path()).unwrap();
    h.put(&h.store.path().join("child"), b"block", 0o600);
    assert!(
        h.handler
            .ensure_persistent(&Context::new())
            .unwrap_err()
            .to_string()
            .contains("directory")
    );
    assert_eq!(h.events(), ["find:xdg-mime", "find:gio", "executable"]);
}
#[test]
fn prerequisite_and_query_error_order_is_fixed() {
    for (command, key, expected) in [
        ("xdg-mime", "xdg", vec!["find:xdg-mime"]),
        ("gio", "gio", vec!["find:xdg-mime", "find:gio"]),
    ] {
        let h = Harness::new();
        h.process.state.lock().unwrap().missing = Some(command.into());
        *h.environment.executable.lock().unwrap() = Err("executable unavailable");
        assert_eq!(
            h.handler
                .ensure_persistent(&Context::new())
                .unwrap_err()
                .to_string(),
            fixture()["errors"][key]
        );
        assert_eq!(h.events(), expected);
    }
    let h = Harness::new();
    h.process.state.lock().unwrap().query_fail = true;
    assert_eq!(
        h.handler
            .ensure_persistent(&Context::new())
            .unwrap_err()
            .to_string(),
        "query xdg-mime: exit status 7"
    );
    assert!(!h.desktop.exists());
    let h = Harness::new();
    h.handler.disable_persistent(&Context::new()).unwrap();
    assert!(h.events().is_empty());
    h.legacy("previous.desktop");
    h.process.state.lock().unwrap().query_fail = true;
    assert!(h.handler.disable_persistent(&Context::new()).is_err());
    assert!(h.desktop.exists());
    assert!(h.store.path().exists());
}
#[test]
fn temporary_install_owned_cleanup_and_failures_restore_state() {
    let h = Harness::new();
    h.put(&h.mime[0], b"original", 0o642);
    let installation = h
        .handler
        .install(&Context::new(), "http://127.0.0.1:41871/callback")
        .unwrap();
    assert_eq!(
        fs::read(&h.endpoint).unwrap(),
        b"http://127.0.0.1:41871/callback\n"
    );
    assert_eq!(h.events(), ["executable", "default"]);
    assert!(!h.store.path().exists());
    drop(installation);
    assert!(!h.endpoint.exists());
    assert!(!h.desktop.exists());
    assert_eq!(fs::read(&h.mime[0]).unwrap(), b"original");
    assert!(!h.mime[1].exists());
    let h = Harness::new();
    h.process.state.lock().unwrap().default_fail = true;
    assert!(
        h.handler
            .install(&Context::new(), "http://127.0.0.1:41871/callback")
            .is_err()
    );
    assert!(!h.endpoint.exists());
    assert!(!h.desktop.exists());
    assert!(h.mime.iter().all(|path| !path.exists()));
    let h = Harness::new();
    *h.environment.executable.lock().unwrap() = Err("executable unavailable");
    assert!(
        h.handler
            .install(&Context::new(), "http://127.0.0.1:41871/callback")
            .is_err()
    );
    assert!(!h.endpoint.exists());
    let h = Harness::new();
    assert!(
        h.handler
            .install(&Context::new(), "https://example.com/callback")
            .is_err()
    );
    assert!(h.events().is_empty());
}
#[test]
fn snapshots_deduplicate_clean_paths_and_home_missing_omits_mime_files() {
    let h = Harness::with_paths(true, false);
    h.put(&h.mime[0], &[b'a', 0, 255], 0o641);
    h.handler.ensure_persistent(&Context::new()).unwrap();
    let manifest = h.store.load().unwrap().unwrap();
    let snapshots = manifest.linux_mime_snapshots.unwrap();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].mode, 0o641);
    assert_eq!(
        snapshots[0].content.as_deref(),
        Some([b'a', 0, 255].as_slice())
    );
    let h = Harness::with_paths(false, true);
    h.handler.ensure_persistent(&Context::new()).unwrap();
    assert_eq!(
        h.store
            .load()
            .unwrap()
            .unwrap()
            .linux_mime_snapshots
            .unwrap()
            .len(),
        1
    );
    let h = Harness::new();
    fs::create_dir_all(&h.mime[0]).unwrap();
    assert_eq!(
        h.handler
            .ensure_persistent(&Context::new())
            .unwrap_err()
            .to_string(),
        fixture()["errors"]["nonregular"]
    );
    assert!(!h.desktop.exists());
}
#[test]
fn restore_attempts_all_snapshots_and_last_duplicate_wins() {
    let h = Harness::new();
    let blocked = h.home.join("blocked");
    h.put(&blocked, b"block", 0o600);
    let nonempty = h.home.join("nonempty");
    h.put(&nonempty.join("child"), b"", 0o600);
    let restored = h.home.join("created/restored");
    let duplicate = h.home.join("duplicate");
    let make = |path: &Path, body: &[u8], mode| HandlerFileSnapshot {
        path: path.to_string_lossy().into_owned(),
        exists: true,
        mode,
        content: Some(body.to_vec()),
    };
    h.store
        .save(&HandlerManifest {
            version: 1,
            executable_path: "/pixiv".into(),
            linux_mime_snapshots: Some(vec![
                make(&blocked.join("child"), b"no", 0o600),
                make(&restored, &[b'a', 0, 255], 0o642),
                HandlerFileSnapshot {
                    path: nonempty.to_string_lossy().into_owned(),
                    ..HandlerFileSnapshot::default()
                },
                make(&duplicate, b"first", 0o644),
                make(&duplicate, b"last", 0o601),
            ]),
            ..HandlerManifest::default()
        })
        .unwrap();
    h.process.state.lock().unwrap().current = LINUX_DESKTOP_FILE.into();
    let error = h.handler.disable_persistent(&Context::new()).unwrap_err();
    let LinuxHandlerError::Joined(errors) = &error else {
        panic!("restore must aggregate errors");
    };
    assert_eq!(errors.len(), 2);
    assert_eq!(error.to_string().lines().count(), 2);
    assert_eq!(fs::read(&restored).unwrap(), [b'a', 0, 255]);
    assert_eq!(
        fs::metadata(&restored).unwrap().permissions().mode() & 0o777,
        0o642
    );
    assert_eq!(fs::read(&duplicate).unwrap(), b"last");
    assert_eq!(
        fs::metadata(&duplicate).unwrap().permissions().mode() & 0o777,
        0o601
    );
    assert!(h.store.path().exists());
}
#[test]
fn delegation_resolves_previous_handler_and_masks_url_diagnostics() {
    let h = Harness::new();
    assert_eq!(
        h.handler
            .delegate_previous(&Context::new(), "pixiv://unrelated/path?code=synthetic")
            .unwrap_err()
            .to_string(),
        fixture()["errors"]["no_previous"]
    );
    h.legacy("previous.desktop");
    let previous = h.desktop.with_file_name("previous.desktop");
    h.put(&previous, b"previous entry", 0o644);
    h.clear();
    let url = "pixiv://unrelated/path?code=synthetic";
    h.handler.delegate_previous(&Context::new(), url).unwrap();
    {
        let state = h.process.state.lock().unwrap();
        assert_eq!(state.calls[0].0, "gio");
        assert_eq!(
            state.calls[0].1,
            ["launch", previous.to_str().unwrap(), url]
        );
    }
    h.process.state.lock().unwrap().launch_fail = true;
    assert_eq!(
        h.handler
            .delegate_previous(&Context::new(), url)
            .unwrap_err()
            .to_string(),
        fixture()["errors"]["delegate_launch"]
    );
    h.process.state.lock().unwrap().missing = Some("gio".into());
    assert_eq!(
        h.handler
            .delegate_previous(&Context::new(), url)
            .unwrap_err()
            .to_string(),
        fixture()["errors"]["delegate_gio"]
    );
    h.process.state.lock().unwrap().missing = None;
    fs::remove_file(previous).unwrap();
    assert_eq!(
        h.handler
            .delegate_previous(&Context::new(), url)
            .unwrap_err()
            .to_string(),
        fixture()["errors"]["delegate_locate"]
    );
}
#[test]
fn native_path_cases_preserve_temporary_bytes_and_go_persistent_json_loss() {
    for case in fixture()["native_path_cases"].as_array().unwrap() {
        let hex = case["leaf_hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect();
        let lossy_leaf = case["lossy_leaf"].as_str().unwrap();
        let h = Harness::with_leaf(false, false, Some(&bytes));
        for path in h.mime.iter().chain([&h.desktop]) {
            h.put(path, b"native original", 0o640);
        }
        let executable = [b"/synthetic/".as_slice(), &bytes].concat();
        *h.environment.executable.lock().unwrap() =
            Ok(PathBuf::from(OsString::from_vec(executable.clone())));
        let installation = h
            .handler
            .install(&Context::new(), "http://127.0.0.1:41871/callback")
            .unwrap();
        assert!(
            fs::read(&h.desktop)
                .unwrap()
                .windows(executable.len())
                .any(|window| window == executable)
        );
        installation.cleanup().unwrap();
        for path in h.mime.iter().chain([&h.desktop]) {
            assert_eq!(fs::read(path).unwrap(), b"native original");
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        assert!(!h.home.join(lossy_leaf).exists());
        let h = Harness::with_leaf(false, false, Some(&bytes));
        h.put(&h.mime[0], b"native original", 0o640);
        *h.environment.executable.lock().unwrap() =
            Ok(PathBuf::from(OsString::from_vec(executable)));
        h.handler.ensure_persistent(&Context::new()).unwrap();
        let manifest = h.store.load().unwrap().unwrap();
        assert_eq!(manifest.executable_path, format!("/synthetic/{lossy_leaf}"));
        assert_eq!(
            manifest.linux_mime_snapshots.unwrap()[0].path,
            h.home
                .join(lossy_leaf)
                .join("config/mimeapps.list")
                .to_str()
                .unwrap()
        );
        h.handler.disable_persistent(&Context::new()).unwrap();
        assert_eq!(fs::read(&h.mime[0]).unwrap(), b"association changed");
        assert_eq!(
            fs::read(h.home.join(lossy_leaf).join("config/mimeapps.list")).unwrap(),
            b"native original"
        );
        assert!(h.desktop.exists());
    }
}
#[test]
fn manifest_save_failure_rolls_back_and_restore_preserves_existing_parent_mode() {
    let h = Harness::new();
    h.put(&h.mime[0], b"original", 0o642);
    h.process.state.lock().unwrap().block_manifest = Some(h.store.path().to_owned());
    assert!(h.handler.ensure_persistent(&Context::new()).is_err());
    assert_eq!(fs::read(&h.mime[0]).unwrap(), b"original");
    assert!(!h.desktop.exists());
    assert!(!h.mime[1].exists());
    let h = Harness::new();
    let restored = h.home.join("existing/restored");
    h.put(&restored, b"changed", 0o600);
    fs::set_permissions(
        restored.parent().unwrap(),
        fs::Permissions::from_mode(0o751),
    )
    .unwrap();
    h.store
        .save(&HandlerManifest {
            version: 1,
            executable_path: "/pixiv".into(),
            linux_mime_snapshots: Some(vec![HandlerFileSnapshot {
                path: restored.to_string_lossy().into_owned(),
                exists: true,
                mode: 0o641,
                content: Some(b"original".to_vec()),
            }]),
            ..HandlerManifest::default()
        })
        .unwrap();
    h.process.state.lock().unwrap().current = LINUX_DESKTOP_FILE.into();
    h.handler.disable_persistent(&Context::new()).unwrap();
    assert_eq!(fs::read(&restored).unwrap(), b"original");
    assert_eq!(
        fs::metadata(restored.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
}
#[tokio::test]
async fn previous_handler_adapter_obeys_cancellation_without_leaking_url() {
    use pixiv_app::callback_handler::PreviousHandler;
    let h = Harness::new();
    h.legacy("previous.desktop");
    h.put(
        &h.desktop.with_file_name("previous.desktop"),
        b"previous entry",
        0o644,
    );
    let cancellation = tokio_util::sync::CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        h.handler
            .delegate("pixiv://synthetic?code=private", &cancellation)
            .await
            .unwrap_err()
            .to_string(),
        "could not open previous Pixiv URL handler"
    );
}
#[tokio::test]
async fn dropping_delegation_cancels_the_owned_running_process() {
    use pixiv_app::callback_handler::PreviousHandler;
    let h = Harness::new();
    h.legacy("previous.desktop");
    h.put(
        &h.desktop.with_file_name("previous.desktop"),
        b"previous entry",
        0o644,
    );
    h.process.state.lock().unwrap().block_launch = true;
    let handler = h.handler.clone();
    let cancellation = tokio_util::sync::CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        handler
            .delegate("pixiv://synthetic", &task_cancellation)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if h.events().iter().any(|event| event == "launch") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    assert!(!cancellation.is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if h.events().iter().any(|event| event == "launch-canceled") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}
#[test]
fn system_invalid_relay_precedes_missing_home_in_isolated_process() {
    let directory = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "system_invalid_relay_child", "--ignored"])
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .env("XDG_CONFIG_HOME", directory.path().join("config"))
        .env("XDG_DATA_HOME", directory.path().join("data"))
        .env("XDG_DATA_DIRS", directory.path().join("system"))
        .env("PATH", directory.path().join("empty-bin"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}
#[test]
#[ignore = "invoked only by the isolated-process system ordering test"]
fn system_invalid_relay_child() {
    assert!(std::env::var_os("HOME").is_none());
    assert!(pixiv_app::callback_handler::callback_endpoint_path().is_err());
    let case = &fixture()["invalid_endpoint_missing_home"];
    let result = LinuxHandler::system().install(&Context::new(), case["relay"].as_str().unwrap());
    let error = match result {
        Ok(_) => panic!("invalid relay must fail before host work"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), case["error"].as_str().unwrap());
}
