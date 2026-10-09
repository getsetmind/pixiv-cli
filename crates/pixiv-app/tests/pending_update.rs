use pixiv_app::pending_update::{
    PendingUpdateCleanup, PendingUpdateEnvironment, PendingUpdateError,
    SystemPendingUpdateEnvironment,
};
use serde::Deserialize;
use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tempfile::TempDir;

#[derive(Deserialize)]
struct Fixture {
    reference: String,
    source_sha256: String,
    resolver_sha256: String,
    cases: Vec<Case>,
}
#[derive(Clone, Deserialize)]
struct Entry {
    path: String,
    kind: String,
    #[serde(default)]
    target: String,
}
#[derive(Clone, Deserialize)]
struct Case {
    name: String,
    goos: String,
    executable: String,
    entries: Vec<Entry>,
    #[serde(default)]
    executable_error: String,
    #[serde(default)]
    remove_error: String,
    trace: Vec<String>,
    error_stage: String,
    error_prefix: String,
    error_kind: String,
    surviving: Vec<String>,
}

#[derive(Clone)]
struct Environment {
    root: PathBuf,
    case: Case,
    trace: Arc<Mutex<Vec<String>>>,
}
impl Environment {
    fn normalize(&self, value: &str) -> String {
        value.replace(&*self.root.to_string_lossy(), "${ROOT}")
    }
}
impl PendingUpdateEnvironment for Environment {
    fn is_windows(&self) -> bool {
        self.case.goos == "windows"
    }
    fn executable_path(&self) -> io::Result<PathBuf> {
        self.trace.lock().unwrap().push("executable".into());
        if !self.case.executable_error.is_empty() {
            return Err(io::Error::other(self.case.executable_error.clone()));
        }
        let current = std::env::current_dir()?;
        let relative = relative_path(&current, &self.root);
        Ok(PathBuf::from(
            self.case
                .executable
                .replace("${ROOT}", &self.root.to_string_lossy())
                .replace("${RELATIVE_ROOT}", &relative.to_string_lossy()),
        ))
    }
    fn absolute_path(&self, path: &Path) -> io::Result<PathBuf> {
        if self.case.name == "missing_working_directory" {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "working directory no longer exists",
            ));
        }
        SystemPendingUpdateEnvironment.absolute_path(path)
    }
    fn is_symlink(&self, path: &Path) -> io::Result<bool> {
        SystemPendingUpdateEnvironment.is_symlink(path)
    }
    fn evaluate_symlinks(&self, path: &Path) -> io::Result<PathBuf> {
        SystemPendingUpdateEnvironment.evaluate_symlinks(path)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.trace.lock().unwrap().push(format!(
            "remove:{}",
            self.normalize(&path.to_string_lossy())
        ));
        match self.case.remove_error.as_str() {
            "" => SystemPendingUpdateEnvironment.remove(path),
            "not_exist" | "wrapped_not_exist" => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "file does not exist",
            )),
            message => Err(io::Error::other(message.to_owned())),
        }
    }
}
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in &from[common..] {
        result.push("..");
    }
    for component in &to[common..] {
        result.push(component.as_os_str());
    }
    result
}
fn symlink(target: &str, path: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, path).unwrap();
    #[cfg(windows)]
    {
        let _ = (target, path);
        panic!("native Windows symlink creation requires a separately authorized runner");
    }
}
fn error_stage(error: &PendingUpdateError) -> &str {
    match error {
        PendingUpdateError::LocateExecutable { .. } => "executable",
        PendingUpdateError::ResolveExecutable { .. } => "absolute",
        PendingUpdateError::ResolveSymlink { .. } => "symlink",
        PendingUpdateError::ResolveSymlinkTarget { .. } => "symlink_absolute",
        PendingUpdateError::RemoveOldExecutable { .. } => "remove",
    }
}

#[test]
#[cfg(unix)]
fn pending_windows_cleanup_matches_frozen_go_filesystem_contracts() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/pending_update_cleanup.json")).unwrap();
    assert_eq!(
        fixture.reference,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(
        fixture.source_sha256,
        "bfc9fd4ef560745c0cdbe5219320bcf76f82bd2f79f4adee6cb94923763c1d45"
    );
    assert_eq!(
        fixture.resolver_sha256,
        "c299a189ccd1dc7de2bc820b49566a5be244dfa6bc08add4fea0e5af07eb4d2c"
    );
    for case in fixture.cases {
        let temp = TempDir::new().unwrap();
        for entry in &case.entries {
            let path = temp.path().join(&entry.path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            match entry.kind.as_str() {
                "file" => fs::write(&path, b"keep").unwrap(),
                "directory" => fs::create_dir_all(&path).unwrap(),
                "symlink" => symlink(&entry.target, &path),
                _ => panic!("unknown entry kind"),
            }
        }
        let env = Environment {
            root: temp.path().to_owned(),
            case: case.clone(),
            trace: Arc::default(),
        };
        let result = PendingUpdateCleanup::new(env.clone()).cleanup();
        assert_eq!(*env.trace.lock().unwrap(), case.trace, "{}", case.name);
        match result {
            Ok(()) => assert_eq!(case.error_stage, "", "{}", case.name),
            Err(error) => {
                assert_eq!(
                    error_stage(&error),
                    case.error_stage,
                    "{}: {error}",
                    case.name
                );
                let display = env.normalize(&error.to_string());
                if case.executable_error.is_empty() && case.remove_error.is_empty() {
                    assert!(
                        display.starts_with(&case.error_prefix),
                        "{}: {display}",
                        case.name
                    );
                } else {
                    assert_eq!(display, case.error_prefix, "{}", case.name);
                }
                let source = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
                match case.error_kind.as_str() {
                    "not_exist" => assert_eq!(source.kind(), io::ErrorKind::NotFound),
                    "directory_not_empty" => {
                        assert_eq!(source.kind(), io::ErrorKind::DirectoryNotEmpty)
                    }
                    "not_directory" => assert_eq!(source.kind(), io::ErrorKind::NotADirectory),
                    "too_many_links" => {
                        assert_eq!(source.to_string(), "EvalSymlinks: too many links")
                    }
                    "other" => assert_eq!(source.kind(), io::ErrorKind::Other),
                    kind => panic!("unknown error kind {kind}"),
                }
            }
        }
        let surviving: Vec<_> = case
            .entries
            .iter()
            .filter(|entry| fs::symlink_metadata(temp.path().join(&entry.path)).is_ok())
            .map(|entry| entry.path.clone())
            .collect();
        assert_eq!(surviving, case.surviving, "{}", case.name);
    }
}

#[derive(Clone)]
struct FaultEnvironment {
    fail: &'static str,
    trace: Arc<Mutex<Vec<String>>>,
}
impl FaultEnvironment {
    fn step(&self, name: &str) -> io::Result<()> {
        self.trace.lock().unwrap().push(name.to_owned());
        if name == self.fail {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "synthetic denied",
            ))
        } else {
            Ok(())
        }
    }
}
impl PendingUpdateEnvironment for FaultEnvironment {
    fn is_windows(&self) -> bool {
        self.fail != "nonwindows"
    }
    fn executable_path(&self) -> io::Result<PathBuf> {
        self.step("executable")?;
        Ok("initial.exe".into())
    }
    fn absolute_path(&self, path: &Path) -> io::Result<PathBuf> {
        if path == Path::new("initial.exe") {
            self.step("absolute")?;
            Ok("/absolute.exe".into())
        } else {
            self.step("symlink_absolute")?;
            Ok("/resolved.exe".into())
        }
    }
    fn is_symlink(&self, _path: &Path) -> io::Result<bool> {
        self.step("lstat")?;
        Ok(true)
    }
    fn evaluate_symlinks(&self, _path: &Path) -> io::Result<PathBuf> {
        self.step("symlink")?;
        Ok("target.exe".into())
    }
    fn remove(&self, _path: &Path) -> io::Result<()> {
        self.step("remove")
    }
}

#[test]
fn pending_cleanup_dependency_errors_keep_order_context_and_io_source() {
    for (failure, expected, stage, display) in [
        ("nonwindows", vec![], "", ""),
        (
            "executable",
            vec!["executable"],
            "executable",
            "locate executable for pending update cleanup: synthetic denied",
        ),
        (
            "absolute",
            vec!["executable", "absolute"],
            "absolute",
            "resolve current executable \"initial.exe\": synthetic denied",
        ),
        (
            "lstat",
            vec!["executable", "absolute", "lstat", "remove"],
            "",
            "",
        ),
        (
            "symlink",
            vec!["executable", "absolute", "lstat", "symlink"],
            "symlink",
            "resolve executable symlink \"/absolute.exe\": synthetic denied",
        ),
        (
            "symlink_absolute",
            vec![
                "executable",
                "absolute",
                "lstat",
                "symlink",
                "symlink_absolute",
            ],
            "symlink_absolute",
            "resolve executable symlink target \"target.exe\": synthetic denied",
        ),
        (
            "remove",
            vec![
                "executable",
                "absolute",
                "lstat",
                "symlink",
                "symlink_absolute",
                "remove",
            ],
            "remove",
            "remove pending old executable \"/resolved.exe.old\": synthetic denied",
        ),
        (
            "",
            vec![
                "executable",
                "absolute",
                "lstat",
                "symlink",
                "symlink_absolute",
                "remove",
            ],
            "",
            "",
        ),
    ] {
        let env = FaultEnvironment {
            fail: failure,
            trace: Arc::default(),
        };
        let result = PendingUpdateCleanup::new(env.clone()).cleanup();
        assert_eq!(*env.trace.lock().unwrap(), expected, "{failure}");
        if stage.is_empty() {
            assert!(result.is_ok(), "{failure}: {result:?}");
        } else {
            let error = result.unwrap_err();
            assert_eq!(error_stage(&error), stage, "{failure}");
            assert_eq!(error.to_string(), display, "{failure}");
            assert_eq!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<io::Error>()
                    .unwrap()
                    .kind(),
                io::ErrorKind::PermissionDenied,
                "{failure}"
            );
        }
    }
}

#[test]
fn system_absolute_paths_lexically_clean_without_requiring_existing_targets() {
    let env = SystemPendingUpdateEnvironment;
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(env.absolute_path(Path::new("")).unwrap(), cwd);
    assert_eq!(
        env.absolute_path(Path::new("absent/.././pixiv.exe"))
            .unwrap(),
        cwd.join("pixiv.exe")
    );
    #[cfg(unix)]
    assert_eq!(
        env.absolute_path(Path::new("/../../tmp//absent/../pixiv.exe"))
            .unwrap(),
        PathBuf::from("/tmp/pixiv.exe")
    );
}

#[test]
#[cfg(not(windows))]
fn system_startup_cleanup_is_a_true_non_windows_noop() {
    pixiv_app::pending_update::cleanup_pending_windows_update().unwrap();
    assert!(!SystemPendingUpdateEnvironment.is_windows());
}
