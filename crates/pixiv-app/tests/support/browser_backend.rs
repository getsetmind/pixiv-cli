use pixiv_app::{
    browser_chromium::ChromiumKeySource,
    browser_cookies::{
        BrowserCookieError, BrowserDirEntry, BrowserEnvironment, BrowserFiles, BrowserMetadata,
        SecretBytes, SystemBrowserFiles,
    },
    browser_dpapi::{DpapiBlob, WindowsDpapi, WindowsDpapiApi},
    host_context::ContextHostProcess,
    host_process::{HostPlatform, HostProcess, HostProcessError, ProcessOutput, ProcessStdio},
    lifecycle::{Context, ContextError},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString, c_void},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub fn provider_fixture() -> Value {
    let body = include_bytes!("../../../pixiv-cli/tests/fixtures/browser-chromium-provider.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(body)),
        "4983114facd5cd815e9ec9006ff5d0bc01e86535029a192d0dc80642f9263846"
    );
    let fixture: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["environment"], "linux/amd64");
    assert_eq!(fixture["go_version"], "go1.27.1");
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 147);
    fixture
}

pub fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap()
}

pub fn decode_hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

pub fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut encoded = String::new();
    for byte in bytes {
        write!(encoded, "{byte:02x}").unwrap();
    }
    encoded
}

pub fn context(mode: &str) -> Context {
    match mode {
        "active" => Context::new(),
        "canceled" => {
            let context = Context::new();
            context.cancel();
            context
        }
        "deadline" => Context::with_deadline(Instant::now() - Duration::from_secs(1)),
        other => panic!("unsupported owned context: {other}"),
    }
}

pub fn write(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

pub struct OwnedEnvironment {
    pub home: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

impl BrowserEnvironment for OwnedEnvironment {
    fn user_home(&self) -> io::Result<PathBuf> {
        self.home
            .clone()
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }
    fn config_home(&self) -> Option<PathBuf> {
        self.config.clone()
    }
}

pub struct FixtureKeys {
    keys: Vec<Vec<u8>>,
    cancel: bool,
    calls: AtomicUsize,
}

impl FixtureKeys {
    pub fn from_input(input: &Value) -> Arc<Self> {
        Arc::new(Self {
            keys: input["keys_hex"]
                .as_array()
                .unwrap()
                .iter()
                .map(|key| decode_hex(key.as_str().unwrap()))
                .collect(),
            cancel: input["cancel_inside_key_hook"].as_bool().unwrap(),
            calls: AtomicUsize::new(0),
        })
    }
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ChromiumKeySource for FixtureKeys {
    fn keys(&self, context: &Context) -> Result<Vec<SecretBytes>, BrowserCookieError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.cancel {
            context.cancel();
        }
        Ok(self.keys.iter().cloned().map(SecretBytes::new).collect())
    }
    fn unprotect_legacy(&self, _: &Context, _: &[u8]) -> Result<SecretBytes, BrowserCookieError> {
        panic!("Linux Read must not call DPAPI")
    }
}

pub struct ProcessCall {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub stdio: ProcessStdio,
    pub context: usize,
}

pub struct ReadProcess {
    platform: HostPlatform,
    csv: Vec<u8>,
    secret_mode: String,
    password: Vec<u8>,
    pub calls: Mutex<Vec<ProcessCall>>,
    pub lookups: Mutex<Vec<OsString>>,
}

impl ReadProcess {
    pub fn new(platform: HostPlatform, csv: Vec<u8>) -> Self {
        Self {
            platform,
            csv,
            secret_mode: "unexpected".into(),
            password: Vec::new(),
            calls: Mutex::new(Vec::new()),
            lookups: Mutex::new(Vec::new()),
        }
    }

    pub fn from_input(input: &Value) -> Self {
        let mut process = Self::new(HostPlatform::Linux, decode_hex(text(input, "csv_hex")));
        if text(input, "key_source") == "linux_secret_tool" {
            process.secret_mode = text(input, "secret_mode").into();
            process.password = decode_hex(text(input, "password_hex"));
        }
        process
    }

    pub fn command_fields(&self, database: &Path) -> Value {
        let calls = self.calls.lock().unwrap();
        let sqlite: Vec<_> = calls
            .iter()
            .filter(|call| call.program == "sqlite3")
            .collect();
        assert!(sqlite.len() <= 1);
        match sqlite.first() {
            None => {
                json!({"sqlite_called":false, "sqlite_flags":[], "sqlite_parameters":[], "sqlite_sql":""})
            }
            Some(call) => {
                assert_eq!(call.stdio, ProcessStdio::Capture);
                assert_eq!(call.args.len(), 13);
                assert_eq!(call.args[11], database.as_os_str());
                let mut parameters = Vec::new();
                for pair in call.args[5..11].chunks_exact(2) {
                    assert_eq!(pair[0], "-cmd");
                    parameters.push(pair[1].to_str().unwrap());
                }
                parameters.sort_unstable();
                json!({
                    "sqlite_called":true,
                    "sqlite_flags":call.args[..5].iter().map(|arg| arg.to_str().unwrap()).collect::<Vec<_>>(),
                    "sqlite_parameters":parameters,
                    "sqlite_sql":call.args[12].to_str().unwrap(),
                })
            }
        }
    }

    pub fn secret_args(&self) -> Value {
        let calls = self.calls.lock().unwrap();
        let secret: Vec<_> = calls
            .iter()
            .filter(|call| call.program == "secret-tool")
            .collect();
        assert!(secret.len() <= 1);
        match secret.first() {
            None => json!([]),
            Some(call) => {
                assert_eq!(call.stdio, ProcessStdio::CaptureStdout);
                json!(
                    call.args
                        .iter()
                        .map(|arg| arg.to_str().unwrap())
                        .collect::<Vec<_>>()
                )
            }
        }
    }
}

#[cfg(unix)]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(code << 8)
}

#[cfg(windows)]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(code as u32)
}

impl HostProcess for ReadProcess {
    fn platform(&self) -> HostPlatform {
        self.platform
    }
    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        assert!(program == "sqlite3" || program == "secret-tool");
        self.lookups.lock().unwrap().push(program.into());
        Ok(PathBuf::from("/owned-fixture-bin").join(program))
    }
    fn run(
        &self,
        _: &OsStr,
        _: &[OsString],
        _: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        panic!("Read must keep the caller context")
    }
    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("cookie Read must not launch a browser")
    }
}

impl ContextHostProcess for ReadProcess {
    fn run_context(
        &self,
        context: &Context,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        self.look_path(program)?;
        if let Some(error) = context.error() {
            return Err(HostProcessError::Context(error));
        }
        self.calls.lock().unwrap().push(ProcessCall {
            program: program.into(),
            args: args.into(),
            stdio,
            context: context as *const Context as usize,
        });
        if program == "sqlite3" {
            assert_eq!(stdio, ProcessStdio::Capture);
            return Ok(ProcessOutput {
                status: exit_status(0),
                stdout: self.csv.clone(),
                stderr: Vec::new(),
            });
        }
        assert_eq!(stdio, ProcessStdio::CaptureStdout);
        match self.secret_mode.as_str() {
            "success" => Ok(ProcessOutput {
                status: exit_status(0),
                stdout: self.password.clone(),
                stderr: Vec::new(),
            }),
            "fail" => Err(HostProcessError::Exit {
                program: program.into(),
                output: ProcessOutput {
                    status: exit_status(17),
                    stdout: Vec::new(),
                    stderr: b"owned synthetic private diagnostic".to_vec(),
                },
            }),
            other => panic!("unexpected owned secret-tool invocation: {other}"),
        }
    }
}

pub fn read_fields(result: Result<Vec<SecretBytes>, BrowserCookieError>) -> Value {
    match result {
        Ok(values) => json!({
            "class":"", "error":"", "values_nil":false,
            "values_hex":values.iter().map(|value| encode_hex(value.as_bytes())).collect::<Vec<_>>(),
            "secret_strings":values.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }),
        Err(error) => {
            let class = match &error {
                BrowserCookieError::QueryInvalid => "query_invalid",
                BrowserCookieError::InvalidProfileId => "invalid_profile_id",
                BrowserCookieError::DatabaseNotFound => "database_not_found",
                BrowserCookieError::EncryptedFormatUnknown => "encrypted_format_unknown",
                BrowserCookieError::QueryFailed => "query_failed",
                BrowserCookieError::SecretServiceAccess => "secret_service_access",
                BrowserCookieError::Context(ContextError::Canceled) => "context_canceled",
                other => panic!("unexpected actual Read error: {other}"),
            };
            json!({"class":class, "error":error.to_string(), "values_nil":true, "values_hex":[], "secret_strings":[]})
        }
    }
}

#[derive(Default)]
pub struct StageFiles {
    pub errors: Mutex<BTreeMap<(String, PathBuf), io::ErrorKind>>,
    pub trace: Mutex<Vec<(String, PathBuf)>>,
}

impl StageFiles {
    fn check(&self, stage: &str, path: &Path) -> io::Result<()> {
        self.trace.lock().unwrap().push((stage.into(), path.into()));
        match self
            .errors
            .lock()
            .unwrap()
            .get(&(stage.into(), path.into()))
            .copied()
        {
            Some(kind) => Err(io::Error::new(
                kind,
                "owned synthetic private filesystem diagnostic",
            )),
            None => Ok(()),
        }
    }
    pub fn error(&self, stage: &str, path: &Path, kind: io::ErrorKind) {
        self.errors
            .lock()
            .unwrap()
            .insert((stage.into(), path.into()), kind);
    }
}

struct LateReadError {
    file: fs::File,
    kind: io::ErrorKind,
    delivered: bool,
}

impl Read for LateReadError {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.delivered {
            return Err(io::Error::new(
                self.kind,
                "owned synthetic private late read diagnostic",
            ));
        }
        self.delivered = true;
        self.file.read(bytes)
    }
}

impl BrowserFiles for StageFiles {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.check("read_file", path)?;
        SystemBrowserFiles.read_file(path)
    }
    fn open_file(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
        self.check("open_file", path)?;
        let file = fs::File::open(path)?;
        let kind = self
            .errors
            .lock()
            .unwrap()
            .get(&("late_read".into(), path.into()))
            .copied();
        match kind {
            Some(kind) => Ok(Box::new(LateReadError {
                file,
                kind,
                delivered: false,
            })),
            None => Ok(Box::new(file)),
        }
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<BrowserDirEntry>> {
        self.check("read_dir", path)?;
        SystemBrowserFiles.read_dir(path)
    }
    fn metadata(&self, path: &Path) -> io::Result<BrowserMetadata> {
        self.check("metadata", path)?;
        SystemBrowserFiles.metadata(path)
    }
}

struct UnusedDpapi;

unsafe impl WindowsDpapiApi for UnusedDpapi {
    unsafe fn crypt_unprotect_data(
        &self,
        _: *const DpapiBlob,
        _: *mut *mut u16,
        _: *const DpapiBlob,
        _: *mut c_void,
        _: *const c_void,
        _: u32,
        _: *mut DpapiBlob,
    ) -> bool {
        panic!("owned plaintext and Linux-key tests must not invoke DPAPI")
    }
    unsafe fn local_free(&self, _: *mut c_void) -> *mut c_void {
        panic!("no native allocation")
    }
}

pub fn unused_dpapi() -> Arc<WindowsDpapi> {
    Arc::new(WindowsDpapi::new(Arc::new(UnusedDpapi)))
}
