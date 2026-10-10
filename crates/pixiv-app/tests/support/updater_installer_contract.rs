#![allow(dead_code)]
use base64::{Engine, engine::general_purpose::STANDARD};
use pixiv_app::{
    reverse_search::{
        ReverseFuture,
        http::{HttpRequest, HttpTransport},
    },
    update::{
        BuildInfo, CallerContext, ExternalError, Release, ReleaseAsset, ReleaseCache,
        ReleaseInstaller, SourceDetector, UpdateFuture,
        installer::{
            BinaryChecker, ExecutableLocator, FileReleaseCache, FileReplacer, InstallFile,
            InstallFileSystem, NativeInstallFileSystem, NativeSourceDetector,
            ReleaseInstallerOptions, ReplacementSourcePreservationError, SignedReleaseInstaller,
            SourceDetectionEnvironment,
        },
        source::{ReleaseSourceSelector, parse_release_sources},
    },
};
use pixiv_sdk::{
    context::Context,
    fanbox::transport::{BodyFuture, Headers, RawBody, RawRead, RawResponse},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../pixiv-cli/tests/fixtures/updater-signed-install.json"
    ))
    .unwrap()
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn decode(value: &Value) -> Vec<u8> {
    STANDARD.decode(value.as_str().unwrap_or("")).unwrap()
}
fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("").into()
}
#[derive(Debug)]
struct Failure(String);
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for Failure {}
fn failure(value: &str) -> ExternalError {
    Box::new(Failure(value.into()))
}
fn norm(value: impl AsRef<str>, root: &Path) -> String {
    let mut value = value.as_ref().replace(root.to_str().unwrap(), "$ROOT");
    for prefix in [
        ".pixiv-update-stage-",
        ".pixiv-update-",
        ".github-releases-",
    ] {
        let mut offset = 0;
        while let Some(at) = value[offset..].find(prefix) {
            let start = offset + at + prefix.len();
            let end = start
                + value[start..]
                    .bytes()
                    .take_while(u8::is_ascii_digit)
                    .count();
            if end > start {
                value.replace_range(start..end, "TEMP");
                offset = start + 4;
            } else {
                offset = start;
            }
        }
    }
    value
}
fn material(root: &Path) -> Vec<String> {
    fn walk(root: &Path, path: &Path, out: &mut Vec<String>) {
        let mut entries = fs::read_dir(path)
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let p = entry.path();
            let metadata = fs::symlink_metadata(&p).unwrap();
            let (kind, bytes) = if metadata.file_type().is_symlink() {
                (
                    format!("symlink:{}", fs::read_link(&p).unwrap().display()),
                    String::new(),
                )
            } else if metadata.is_dir() {
                ("dir".into(), String::new())
            } else {
                (
                    "file".into(),
                    String::from_utf8_lossy(&fs::read(&p).unwrap()).into_owned(),
                )
            };
            out.push(norm(
                format!("{}|{kind}|{bytes}", p.strip_prefix(root).unwrap().display()),
                root,
            ));
            if metadata.is_dir() {
                walk(root, &p, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}
#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0)
}
#[cfg(not(unix))]
fn mode(_path: &Path) -> u32 {
    0
}
#[cfg(unix)]
fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
#[cfg(not(unix))]
fn write_mode(path: &Path, bytes: &[u8], _mode: u32) {
    fs::write(path, bytes).unwrap();
}
#[derive(Clone)]
struct Trace {
    root: PathBuf,
    values: Arc<Mutex<Vec<String>>>,
}
impl Trace {
    fn record(&self, value: impl AsRef<str>) {
        self.values.lock().unwrap().push(norm(value, &self.root));
    }
}
struct Body {
    bytes: Vec<u8>,
    done: bool,
    fail: bool,
    cancel: Option<Context>,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            if self.done {
                if let Some(context) = self.cancel.take() {
                    context.cancel();
                }
                return RawRead {
                    count: 0,
                    error: self.fail.then(|| failure("synthetic body read failure")),
                    eof: !self.fail,
                };
            }
            let count = output.len().min(self.bytes.len());
            output[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes.drain(..count);
            self.done = self.bytes.is_empty();
            RawRead {
                count,
                error: None,
                eof: false,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async { Err(failure("synthetic ignored body close failure")) })
    }
}

struct Transport {
    input: Value,
    trace: Trace,
    context: Context,
}
impl HttpTransport for Transport {
    fn send(
        &self,
        r: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let name = r.url.rsplit('/').next().unwrap();
            let side = match name {
                "checksums.txt" => "checksums",
                "checksums.json" => "manifest",
                _ => "archive",
            };
            let setting = text(&self.input, "transport");
            let preferred = r.url.starts_with("https://preferred.invalid/");
            let sources = self.input["sources"].as_bool().unwrap();
            let user_agent = r
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("User-Agent"))
                .and_then(|(_, v)| v.first())
                .cloned()
                .unwrap_or_default();
            self.trace.record(format!(
                "http {} {} user-agent={user_agent}",
                r.method, r.url
            ));
            if setting == format!("{side}_request")
                || (sources
                    && (setting == "all_failure" || (preferred && setting == "fallback_request")))
            {
                return Err(failure("synthetic request failure"));
            }
            let status = if setting == format!("{side}_status")
                || (preferred && setting == "fallback_status")
            {
                503
            } else {
                200
            };
            let bytes = if sources && setting == "verification_no_fallback" && side == "manifest" {
                b"{}".to_vec()
            } else {
                decode(&self.input[side])
            };
            Ok(Some(RawResponse {
                status,
                headers: Headers::new(),
                content_length: bytes.len() as i64,
                body: Some(Box::new(Body {
                    bytes,
                    done: false,
                    fail: setting == format!("{side}_read")
                        || (preferred && setting == "fallback_read"),
                    cancel: (text(&self.input, "cancel") == "after_archive" && side == "archive")
                        .then(|| self.context.clone()),
                })),
            }))
        })
    }
}
struct Probe;
impl HttpTransport for Probe {
    fn send(
        &self,
        r: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            if !r.url.starts_with("https://preferred.invalid/") {
                return Err(Box::new(r.context.cancelled().await) as ExternalError);
            }
            Ok(Some(RawResponse {
                status: 200,
                headers: Headers::new(),
                content_length: -1,
                body: Some(Box::new(Body {
                    bytes: b"probe body does not verify checksums".to_vec(),
                    done: false,
                    fail: false,
                    cancel: None,
                })),
            }))
        })
    }
}
struct Locator {
    target: PathBuf,
    trace: Trace,
    fail: bool,
}
impl ExecutableLocator for Locator {
    fn executable(&self) -> Result<PathBuf, ExternalError> {
        self.trace.record("executable");
        if self.fail {
            Err(failure("synthetic executable lookup failed"))
        } else {
            Ok(self.target.clone())
        }
    }
}
struct Checker {
    input: Value,
    trace: Trace,
    context: Context,
}
impl BinaryChecker for Checker {
    fn check(
        &self,
        _context: CallerContext,
        path: PathBuf,
        tag: String,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            self.trace
                .record(format!("checker path={} tag={tag}", path.display()));
            let bytes = fs::read(&path).unwrap();
            self.trace.record(format!(
                "checker bytes={} mode={:04o}",
                quote(&bytes),
                mode(&path)
            ));
            assert_eq!(
                bytes,
                b"owned synthetic candidate bytes; never executable\n"
            );
            assert_eq!(mode(&path), 0o755);
            let setting = text(&self.input, "checker");
            if setting == "parent_swap_failure" {
                let parent = path.parent().unwrap().parent().unwrap();
                fs::rename(parent, self.trace.root.join("saved-bin")).unwrap();
                write_mode(parent, b"owned parent replacement", 0o600);
                return Err(failure("synthetic checker rejected version"));
            }
            if setting == "failure" {
                return Err(failure("synthetic checker rejected version"));
            }
            if setting == "extra_workdir_file" {
                write_mode(&path.parent().unwrap().join("keep"), b"owned extra", 0o600);
            }
            if setting == "cancel" || text(&self.input, "cancel") == "after_checker" {
                self.context.cancel();
            }
            Ok(())
        })
    }
}
fn quote(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for ch in String::from_utf8_lossy(bytes).chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}
#[derive(Debug)]
struct Wrapped {
    label: String,
    cause: ExternalError,
}
impl fmt::Display for Wrapped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.label, self.cause)
    }
}
impl Error for Wrapped {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}
fn has_cause(error: &(dyn Error + 'static), expected: &str) -> bool {
    if error
        .downcast_ref::<Failure>()
        .is_some_and(|e| e.0 == expected)
    {
        return true;
    }
    if let Some(joined) = error.downcast_ref::<pixiv_app::update::JoinedError>()
        && joined.causes().any(|cause| has_cause(cause, expected))
    {
        return true;
    }
    error
        .source()
        .is_some_and(|cause| has_cause(cause, expected))
}
fn canceled(error: &(dyn Error + 'static)) -> bool {
    if error.downcast_ref::<pixiv_sdk::context::ContextError>()
        == Some(&pixiv_sdk::context::ContextError::Canceled)
    {
        return true;
    }
    if let Some(joined) = error.downcast_ref::<pixiv_app::update::JoinedError>()
        && joined.causes().any(|cause| canceled(cause))
    {
        return true;
    }
    error.source().is_some_and(canceled)
}
struct Replacer {
    setting: String,
    trace: Trace,
    cache: bool,
}
impl FileReplacer for Replacer {
    fn replace(&self, source: &Path, target: &Path) -> Result<(), ExternalError> {
        self.trace.record(format!(
            "replace source={} target={}",
            source.display(),
            target.display()
        ));
        let bytes = fs::read(source).unwrap();
        if self.cache {
            self.trace.record(format!(
                "source mode={:04o} bytes={} old={}",
                mode(source),
                quote(&bytes),
                quote(&fs::read(target).unwrap_or_default())
            ));
        } else {
            self.trace
                .record(format!("replace bytes={}", quote(&bytes)));
        }
        let original = if self.cache {
            match self.setting.as_str() {
                "preserve_source" => "synthetic unresolved cache recovery",
                "committed_error" => "synthetic committed cache failure",
                "cleanup_ignored_after_failure" => "synthetic original cache failure",
                _ => "synthetic cache replacement failed",
            }
        } else {
            "synthetic replacement failed"
        };
        match self.setting.as_str() {
            "failure" | "replace_failure" => Err(failure(original)),
            "preserve" => Err(Box::new(ReplacementSourcePreservationError::new(failure(
                original,
            )))),
            "wrapped_preserve" | "preserve_source" => Err(Box::new(Wrapped {
                label: if self.cache {
                    "wrapped".into()
                } else {
                    "wrapped recovery".into()
                },
                cause: Box::new(ReplacementSourcePreservationError::new(failure(original))),
            })),
            "joined_preserve" => Err(Box::new(pixiv_app::update::JoinedError(vec![
                failure("second recovery cause"),
                Box::new(ReplacementSourcePreservationError::new(failure(original))),
            ]))),
            "cleanup_failure" | "cleanup_error_after_success" | "cleanup_ignored_after_failure" => {
                fs::remove_file(source).unwrap();
                fs::create_dir(source).unwrap();
                write_mode(
                    &source.join("keep"),
                    if self.cache {
                        b"owned cache recovery"
                    } else {
                        b"owned recovery material"
                    },
                    0o600,
                );
                if self.setting == "cleanup_error_after_success" {
                    Ok(())
                } else {
                    Err(failure(original))
                }
            }
            "committed_error" => {
                NativeInstallFileSystem.rename(source, target)?;
                Err(failure(original))
            }
            _ => NativeInstallFileSystem.rename(source, target),
        }
    }
}
type CandidateSnapshots = Arc<Mutex<Vec<Vec<u8>>>>;
type CandidateSnapshot = (PathBuf, CandidateSnapshots);

struct StageFile {
    inner: Box<dyn InstallFile>,
    context: Context,
    trace: Trace,
    cancel_on_close: bool,
    candidate_snapshot: Option<CandidateSnapshot>,
}
impl InstallFile for StageFile {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), ExternalError> {
        self.inner.write_all(bytes)
    }
    fn sync(&mut self) -> Result<(), ExternalError> {
        self.inner.sync()
    }
    fn set_permissions(&mut self, mode: u32) -> Result<(), ExternalError> {
        self.inner.set_permissions(mode)
    }
    fn close(&mut self) -> Result<(), ExternalError> {
        let r = self.inner.close();
        if let Some((path, snapshots)) = &self.candidate_snapshot {
            snapshots.lock().unwrap().push(fs::read(path).unwrap());
        }
        if self.cancel_on_close {
            self.trace.record("after_stage_close");
            self.context.cancel();
        }
        r
    }
}
struct FileSystem {
    context: Context,
    trace: Trace,
    cancel: String,
    created_files: Arc<Mutex<Vec<PathBuf>>>,
    candidate_bytes: CandidateSnapshots,
}
impl InstallFileSystem for FileSystem {
    fn read(&self, p: &Path) -> Result<Vec<u8>, ExternalError> {
        NativeInstallFileSystem.read(p)
    }
    fn resolve_executable(&self, p: &Path) -> Result<PathBuf, ExternalError> {
        NativeInstallFileSystem.resolve_executable(p)
    }
    fn create_dir_all(&self, p: &Path, mode: u32) -> Result<(), ExternalError> {
        NativeInstallFileSystem.create_dir_all(p, mode)
    }
    fn set_permissions(&self, p: &Path, mode: u32) -> Result<(), ExternalError> {
        NativeInstallFileSystem.set_permissions(p, mode)
    }
    fn create_temp_dir(&self, p: &Path, prefix: &str) -> Result<PathBuf, ExternalError> {
        NativeInstallFileSystem.create_temp_dir(p, prefix)
    }
    fn create_file(&self, p: &Path, mode: u32) -> Result<Box<dyn InstallFile>, ExternalError> {
        let file = NativeInstallFileSystem.create_file(p, mode)?;
        assert!(
            p.exists(),
            "candidate creation must be physically observable"
        );
        self.created_files.lock().unwrap().push(p.into());
        Ok(Box::new(StageFile {
            inner: file,
            context: self.context.clone(),
            trace: self.trace.clone(),
            cancel_on_close: false,
            candidate_snapshot: Some((p.into(), self.candidate_bytes.clone())),
        }))
    }
    fn create_temp_file(
        &self,
        p: &Path,
        prefix: &str,
    ) -> Result<(PathBuf, Box<dyn InstallFile>), ExternalError> {
        let (path, file) = NativeInstallFileSystem.create_temp_file(p, prefix)?;
        if prefix == ".pixiv-update-stage-" && self.cancel == "after_stage_create" {
            self.trace.record("after_stage_create");
            self.context.cancel();
        }
        Ok((
            path,
            Box::new(StageFile {
                inner: file,
                context: self.context.clone(),
                trace: self.trace.clone(),
                cancel_on_close: prefix == ".pixiv-update-stage-"
                    && self.cancel == "after_stage_close",
                candidate_snapshot: None,
            }),
        ))
    }
    fn rename(&self, s: &Path, t: &Path) -> Result<(), ExternalError> {
        NativeInstallFileSystem.rename(s, t)?;
        if self.cancel == "after_staging" {
            self.trace.record("after_staging");
            self.context.cancel();
        }
        Ok(())
    }
    fn remove(&self, p: &Path) -> Result<(), ExternalError> {
        NativeInstallFileSystem.remove(p)
    }
    fn remove_all(&self, p: &Path) -> Result<(), ExternalError> {
        NativeInstallFileSystem.remove_all(p)
    }
}
pub async fn replay_signed_install() {
    let fixture = fixture();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 199);
    for row in fixture["cases"].as_array().unwrap() {
        let input = &row["input"];
        let name = text(input, "name");
        let root = tempfile::tempdir().unwrap();
        let mut target = root
            .path()
            .join("bin")
            .join(if text(input, "goos") == "windows" {
                "pixiv.exe"
            } else {
                "pixiv"
            });
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let target_type = text(input, "target");
        match target_type.as_str() {
            "missing" => {}
            "parent_missing" => target = root.path().join("absent/pixiv"),
            "directory" => fs::create_dir(&target).unwrap(),
            "symlink" => {
                let real = root.path().join("real").join(target.file_name().unwrap());
                fs::create_dir_all(real.parent().unwrap()).unwrap();
                write_mode(&real, b"owned old target bytes\n", 0o700);
                #[cfg(unix)]
                std::os::unix::fs::symlink(
                    format!("../real/{}", target.file_name().unwrap().to_str().unwrap()),
                    &target,
                )
                .unwrap();
            }
            "dangling_symlink" => {
                #[cfg(unix)]
                std::os::unix::fs::symlink("absent", &target).unwrap();
            }
            _ => write_mode(&target, b"owned old target bytes\n", 0o700),
        }
        let context = Context::new();
        let trace = Trace {
            root: root.path().into(),
            values: Arc::default(),
        };
        let mut keys = BTreeMap::from([(
            "synthetic-installer-fixture-only".into(),
            (0..fixture["synthetic_public_key_hex"].as_str().unwrap().len())
                .step_by(2)
                .map(|i| {
                    u8::from_str_radix(
                        &fixture["synthetic_public_key_hex"].as_str().unwrap()[i..i + 2],
                        16,
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>(),
        )]);
        match text(input, "trust").as_str() {
            "empty" => keys.clear(),
            "unknown_id" => {
                let value = keys.remove("synthetic-installer-fixture-only").unwrap();
                keys.insert("another".into(), value);
            }
            "wrong_length" => {
                keys.get_mut("synthetic-installer-fixture-only")
                    .unwrap()
                    .pop();
            }
            "wrong_public_key" => keys.get_mut("synthetic-installer-fixture-only").unwrap()[0] ^= 1,
            _ => {}
        }
        let created_files = Arc::new(Mutex::new(Vec::new()));
        let mut options = ReleaseInstallerOptions {
            http_transport: Some(Arc::new(pixiv_app::update::http::ClientTransport::new(
                Arc::new(Transport {
                    input: input.clone(),
                    trace: trace.clone(),
                    context: context.clone(),
                }),
            ))),
            trusted_keys: keys,
            executable_path: Some(Arc::new(Locator {
                target: target.clone(),
                trace: trace.clone(),
                fail: target_type == "executable_error",
            })),
            goos: text(input, "goos"),
            goarch: text(input, "goarch"),
            binary_checker: Some(Arc::new(Checker {
                input: input.clone(),
                trace: trace.clone(),
                context: context.clone(),
            })),
            replacer: Some(Arc::new(Replacer {
                setting: text(input, "replacer"),
                trace: trace.clone(),
                cache: false,
            })),
            file_system: Some(Arc::new(FileSystem {
                context: context.clone(),
                trace: trace.clone(),
                cancel: text(input, "cancel"),
                created_files: created_files.clone(),
                candidate_bytes: Arc::default(),
            })),
            ..Default::default()
        };
        if input["sources"].as_bool().unwrap() {
            options.source_selector=Some(Arc::new(ReleaseSourceSelector::new(parse_release_sources(b"preferred|-|https://preferred.invalid/{url}\nfallback|-|https://fallback.invalid/{url}\n").unwrap(),Arc::new(Probe))));
        }
        let installer = SignedReleaseInstaller::new(options);
        if text(input, "cancel") == "before" {
            context.cancel();
        }
        let release = &input["release"];
        let rel = Release {
            tag_name: text(release, "TagName"),
            version: text(release, "Version"),
            prerelease: release["Prerelease"].as_bool().unwrap_or(false),
            assets: serde_json::from_value::<Vec<ReleaseAsset>>(release["Assets"].clone()).unwrap(),
        };
        let result = installer.install(Arc::new(context), rel).await;
        let error = result
            .as_ref()
            .err()
            .map(|e| norm(e.to_string(), root.path()))
            .unwrap_or_default();
        assert_eq!(
            result.as_ref().err().is_some_and(|e| canceled(e.as_ref())),
            row["canceled"].as_bool().unwrap(),
            "{name}: canceled"
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| has_cause(e.as_ref(), "synthetic checker rejected version")),
            row["checker_cause"].as_bool().unwrap(),
            "{name}: checker cause"
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| has_cause(e.as_ref(), "synthetic replacement failed")),
            row["replacement_cause"].as_bool().unwrap(),
            "{name}: replacement cause"
        );
        assert_eq!(
            result.as_ref().err().is_some_and(|e| {
                pixiv_app::update::installer::must_preserve_replacement_source(e.as_ref())
            }),
            row["preserve_source"].as_bool().unwrap(),
            "{name}: preservation"
        );
        if name.starts_with("archive_") {
            assert_eq!(
                !created_files.lock().unwrap().is_empty(),
                row["direct_candidate_exists"].as_bool().unwrap(),
                "{name}: public candidate creation witness against direct Go reference"
            );
            let extracted_error = created_files
                .lock()
                .unwrap()
                .first()
                .map(|path| norm(path.to_string_lossy(), root.path()));
            let public_extraction_error = if let Some(path) = extracted_error {
                error.replace(
                    &path,
                    &format!("$ROOT/{}", path.rsplit('/').next().unwrap()),
                )
            } else {
                error.clone()
            };
            assert_eq!(
                public_extraction_error,
                row["extract_error"].as_str().unwrap(),
                "{name}: public extraction failure against direct Go reference"
            );
        }
        assert_eq!(error, row["error"].as_str().unwrap(), "{name}: error");
        assert_eq!(
            *trace.values.lock().unwrap(),
            row["trace"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect::<Vec<_>>(),
            "{name}: trace"
        );
        assert_eq!(
            String::from_utf8_lossy(&fs::read(&target).unwrap_or_default()),
            row["target_bytes"].as_str().unwrap(),
            "{name}: bytes"
        );
        assert_eq!(
            mode(&target),
            row["target_mode"].as_u64().unwrap() as u32,
            "{name}: mode"
        );
        assert_eq!(
            material(root.path()),
            row["material"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect::<Vec<_>>(),
            "{name}: material"
        );
    }
}

struct DetectorEnvironment {
    row: Value,
    root: PathBuf,
    trace: Trace,
    resolved: AtomicBool,
}
impl DetectorEnvironment {
    fn expand(&self, value: &str) -> PathBuf {
        value.replace("$ROOT", self.root.to_str().unwrap()).into()
    }
}
impl SourceDetectionEnvironment for DetectorEnvironment {
    fn executable(&self) -> Result<PathBuf, ExternalError> {
        self.trace.record("executable");
        if matches!(text(&self.row, "failure").as_str(), "executable" | "all") {
            return Err(failure("synthetic executable failure"));
        }
        Ok(self.expand("$ROOT/raw/pixiv"))
    }
    fn eval_symlinks(&self, path: &Path) -> Result<PathBuf, ExternalError> {
        self.trace.record(format!("resolve {}", path.display()));
        if !self.resolved.swap(true, Ordering::SeqCst) {
            if text(&self.row, "failure") == "resolve_actual" {
                return Err(failure("synthetic actual resolver failure"));
            }
            return Ok(self.expand(&text(&self.row, "actual")));
        }
        match text(&self.row, "failure").as_str() {
            "expected_missing" => Err(Box::new(io::Error::from(io::ErrorKind::NotFound))),
            "expected_error" => Err(failure("synthetic expected resolver failure")),
            _ => Ok(self.expand(&text(&self.row, "expected"))),
        }
    }
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ExternalError> {
        self.trace.record(format!("read {}", path.display()));
        if text(&self.row, "failure") == "receipt" {
            return Err(failure("file does not exist"));
        }
        Ok(text(&self.row, "receipt").into_bytes())
    }
    fn build_main(&self) -> Option<String> {
        self.trace.record("buildinfo");
        if matches!(
            text(&self.row, "failure").as_str(),
            "build_absent" | "build_nil"
        ) {
            None
        } else {
            Some(text(&self.row, "build_main"))
        }
    }
    fn getenv(&self, key: &str) -> String {
        self.trace.record(format!("getenv {key}"));
        text(&self.row, if key == "GOBIN" { "gobin" } else { "gopath" })
            .replace("$ROOT", self.root.to_str().unwrap())
    }
    fn default_gopath(&self) -> String {
        "$DEFAULT_GOPATH".into()
    }
}
pub fn replay_detectors() {
    let fixture = fixture();
    assert_eq!(fixture["detectors"].as_array().unwrap().len(), 34);
    for row in fixture["detectors"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let trace = Trace {
            root: root.path().into(),
            values: Arc::default(),
        };
        let detector = NativeSourceDetector::new(
            text(row, "goos"),
            Arc::new(DetectorEnvironment {
                row: row.clone(),
                root: root.path().into(),
                trace: trace.clone(),
                resolved: AtomicBool::new(false),
            }),
        );
        let result = detector.detect(&BuildInfo {
            version: text(row, "version"),
        });
        assert_eq!(
            result.as_ref().map(|source| source.as_str()).unwrap_or(""),
            row["source"].as_str().unwrap(),
            "{} source",
            row["name"]
        );
        assert_eq!(
            result
                .err()
                .map(|e| norm(e.to_string(), root.path()))
                .unwrap_or_default(),
            row["error"].as_str().unwrap(),
            "{} error",
            row["name"]
        );
        assert_eq!(
            *trace.values.lock().unwrap(),
            row["trace"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect::<Vec<_>>(),
            "{} trace",
            row["name"]
        );
    }
}
pub async fn replay_caches() {
    let fixture = fixture();
    assert_eq!(fixture["caches"].as_array().unwrap().len(), 13);
    for row in fixture["caches"].as_array().unwrap() {
        let name = text(row, "name");
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("cache");
        let target = dir.join("github-releases.json");
        let old = b"{\"schema\":2,\"checked_at\":\"2026-10-10T00:00:00Z\",\"releases\":[]}\n";
        let body=b"{\"schema\":2,\"checked_at\":\"2026-10-10T12:00:00Z\",\"releases\":[],\"pages\":[]}\n";
        match name.as_str() {
            "read_missing" | "first_write" => {}
            "read_directory" => fs::create_dir_all(&target).unwrap(),
            "read_bytes" => {
                fs::create_dir_all(&dir).unwrap();
                write_mode(&target, b"not decoded by physical cache\xff", 0o644);
            }
            "directory_is_file" => write_mode(&dir, b"owned blocking file", 0o600),
            _ => {
                fs::create_dir_all(&dir).unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
                }
                write_mode(&target, old, 0o644);
            }
        }
        let trace = Trace {
            root: root.path().into(),
            values: Arc::default(),
        };
        let cache = FileReleaseCache::with_ports(
            &dir,
            &target,
            Arc::new(NativeInstallFileSystem),
            Arc::new(Replacer {
                setting: name.clone(),
                trace: trace.clone(),
                cache: true,
            }),
        );
        let context = Context::new();
        if name.contains("canceled") {
            context.cancel();
        }
        let (bytes, result) = if name.starts_with("read_") {
            match cache.read(Arc::new(context)).await {
                Ok(bytes) => (bytes, Ok(())),
                Err(error) => (None, Err(error)),
            }
        } else {
            let result = cache.write(Arc::new(context), body.to_vec()).await;
            let bytes = cache
                .read(Arc::new(Context::background()))
                .await
                .unwrap_or(None);
            (bytes, result)
        };
        let error = result
            .as_ref()
            .err()
            .map(|e| norm(e.to_string(), root.path()))
            .unwrap_or_default();
        assert_eq!(error, row["error"].as_str().unwrap(), "{name} error");
        assert_eq!(
            bytes.is_some(),
            row["exists"].as_bool().unwrap(),
            "{name} exists"
        );
        assert_eq!(
            String::from_utf8_lossy(&bytes.unwrap_or_default()),
            row["bytes"].as_str().unwrap(),
            "{name} bytes"
        );
        assert_eq!(
            mode(&dir),
            row["directory_mode"].as_u64().unwrap() as u32,
            "{name} directory mode"
        );
        assert_eq!(
            mode(&target),
            row["file_mode"].as_u64().unwrap() as u32,
            "{name} file mode"
        );
        assert_eq!(
            material(root.path()),
            row["material"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect::<Vec<_>>(),
            "{name} material"
        );
        assert_eq!(
            *trace.values.lock().unwrap(),
            row["trace"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect::<Vec<_>>(),
            "{name} trace"
        );
        assert_eq!(
            result.as_ref().err().is_some_and(|error| {
                pixiv_app::update::installer::must_preserve_replacement_source(error.as_ref())
            }),
            row["preserve_source"].as_bool().unwrap(),
            "{name} preservation"
        );
    }
}

struct ArchiveChecker {
    payload: Vec<u8>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
impl BinaryChecker for ArchiveChecker {
    fn check(
        &self,
        _context: CallerContext,
        path: PathBuf,
        _tag: String,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            assert_eq!(fs::read(&path).unwrap(), self.payload);
            assert_eq!(mode(&path), 0o755);
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
pub async fn replay_archive_edges() {
    replay_extra_archives(include_str!("updater_archive_edges_go_reference.json"), 13).await;
}
pub async fn replay_sparse_archives() {
    replay_extra_archives(include_str!("updater_sparse_go_reference.json"), 7).await;
}
pub async fn replay_old_gnu_archives() {
    replay_extra_archives(include_str!("updater_old_gnu_go_reference.json"), 2).await;
}
pub async fn replay_header_archives() {
    replay_extra_archives(include_str!("updater_header_go_reference.json"), 4).await;
}
pub async fn replay_zip_creator_archives() {
    replay_extra_archives(include_str!("updater_zip_creator_go_reference.json"), 3).await;
}
pub async fn replay_zip_count_archives() {
    replay_extra_archives(include_str!("updater_zip_count_go_reference.json"), 1).await;
}
async fn replay_extra_archives(json: &str, count: usize) {
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use sha2::{Digest, Sha256};
    let reference: Value = serde_json::from_str(json).unwrap();
    assert_eq!(reference["exit_code"], 0);
    assert_eq!(reference["rows"].as_array().unwrap().len(), count);
    let seed = Sha256::digest(b"pixiv updater synthetic fixture key; no production trust");
    let key = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
    for row in reference["rows"].as_array().unwrap() {
        let name = text(row, "name");
        let payload = if row["accepted_payload_base64"].is_string() {
            decode(&row["accepted_payload_base64"])
        } else {
            b"owned metadata candidate; never executable\n".to_vec()
        };
        let goos = text(row, "goos");
        let binary = if goos == "windows" {
            "pixiv.exe"
        } else {
            "pixiv"
        };
        let archive = decode(&row["archive_base64"]);
        assert_eq!(hex(&Sha256::digest(&archive)), row["archive_sha256"]);
        let archive_name = format!(
            "pixiv-cli_1.2.3_{goos}_amd64{}",
            if goos == "windows" { ".zip" } else { ".tar.gz" }
        );
        let checksums =
            format!("{}  {archive_name}\n", hex(&Sha256::digest(&archive))).into_bytes();
        let manifest=serde_json::to_vec(&serde_json::json!({"key_id":"synthetic-installer-fixture-only","checksums_sha256":hex(&Sha256::digest(&checksums)),"signature":STANDARD.encode(key.sign(&checksums).as_ref())})).unwrap();
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join(binary);
        write_mode(&target, b"owned old target bytes\n", 0o700);
        let context = Context::new();
        let trace = Trace {
            root: root.path().into(),
            values: Arc::default(),
        };
        let created_files = Arc::new(Mutex::new(Vec::new()));
        let candidate_bytes = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let input = serde_json::json!({"archive":STANDARD.encode(archive),"checksums":STANDARD.encode(checksums),"manifest":STANDARD.encode(manifest),"transport":"","cancel":"","sources":false});
        let installer = SignedReleaseInstaller::new(ReleaseInstallerOptions {
            http_transport: Some(Arc::new(pixiv_app::update::http::ClientTransport::new(
                Arc::new(Transport {
                    input,
                    trace: trace.clone(),
                    context: context.clone(),
                }),
            ))),
            trusted_keys: BTreeMap::from([(
                "synthetic-installer-fixture-only".into(),
                key.public_key().as_ref().to_vec(),
            )]),
            executable_path: Some(Arc::new(Locator {
                target: target.clone(),
                trace: trace.clone(),
                fail: false,
            })),
            goos: goos.clone(),
            goarch: "amd64".into(),
            binary_checker: Some(Arc::new(ArchiveChecker {
                payload: payload.clone(),
                calls: calls.clone(),
            })),
            file_system: Some(Arc::new(FileSystem {
                context: context.clone(),
                trace,
                cancel: String::new(),
                created_files: created_files.clone(),
                candidate_bytes: candidate_bytes.clone(),
            })),
            ..Default::default()
        });
        let names = [archive_name.as_str(), "checksums.txt", "checksums.json"];
        let release = Release {
            tag_name: "v1.2.3".into(),
            version: "1.2.3".into(),
            prerelease: false,
            assets: names
                .map(|name| ReleaseAsset {
                    name: name.into(),
                    download_url: format!(
                        "https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.3/{name}"
                    ),
                })
                .to_vec(),
        };
        let result = installer.install(Arc::new(context), release).await;
        let mut error = result
            .err()
            .map(|e| norm(e.to_string(), root.path()))
            .unwrap_or_default();
        if let Some(path) = created_files.lock().unwrap().first() {
            error = error.replace(
                &norm(path.display().to_string(), root.path()),
                &format!("$ROOT/{binary}"),
            );
        }
        if row["candidate_base64"].is_string() {
            let snapshots = candidate_bytes.lock().unwrap();
            assert_eq!(
                snapshots.first().cloned().unwrap_or_default(),
                decode(&row["candidate_base64"]),
                "{name}: physical candidate bytes at close"
            );
        }
        assert_eq!(
            error,
            row["error"].as_str().unwrap(),
            "{name}: Go-first error"
        );
        assert_eq!(
            !created_files.lock().unwrap().is_empty(),
            row["candidate_exists"].as_bool().unwrap(),
            "{name}: public physical candidate creation"
        );
        let accepted = row["error"].as_str().unwrap().is_empty();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            usize::from(accepted),
            "{name}: verified checker boundary"
        );
        assert_eq!(
            fs::read(&target).unwrap(),
            if accepted {
                payload.as_slice()
            } else {
                b"owned old target bytes\n".as_slice()
            },
            "{name}: target"
        );
    }
}
