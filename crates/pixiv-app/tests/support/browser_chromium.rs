use pixiv_app::{
    browser_chromium::ChromiumKeySource,
    browser_cookies::{
        BrowserCookieError, BrowserDirEntry, BrowserFiles, BrowserMetadata, SecretBytes,
    },
    browser_dpapi::{DpapiBlob, WindowsDpapi, WindowsDpapiApi},
    host_context::ContextHostProcess,
    host_process::{HostPlatform, HostProcess, HostProcessError, ProcessOutput, ProcessStdio},
    lifecycle::{Context, ContextError},
};
use serde_json::Value;
use std::{
    ffi::{OsStr, OsString, c_void},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

pub fn crypto_fixture() -> Vec<Value> {
    let document: Value = serde_json::from_str(include_str!(
        "../../../pixiv-cli/tests/fixtures/browser-chromium-crypto.json"
    ))
    .unwrap();
    assert_eq!(
        document["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    let cases = document["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 159);
    cases
}
pub fn provider_fixture() -> Vec<Value> {
    let document: Value = serde_json::from_str(include_str!(
        "../../../pixiv-cli/tests/fixtures/browser-chromium-provider.json"
    ))
    .unwrap();
    assert_eq!(
        document["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(document["environment"], "linux/amd64");
    assert_eq!(document["go_version"], "go1.27.1");
    let cases = document["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 147);
    cases
}
pub fn string<'a>(input: &'a Value, name: &str) -> &'a str {
    input[name].as_str().unwrap_or("")
}
pub fn hex(input: &str) -> Vec<u8> {
    assert_eq!(input.len() % 2, 0);
    input
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            fn digit(byte: u8) -> u8 {
                match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    _ => panic!("fixture hex"),
                }
            }
            (digit(pair[0]) << 4) | digit(pair[1])
        })
        .collect()
}
pub fn context(mode: &str) -> Context {
    if mode == "deadline" {
        return Context::with_deadline(Instant::now());
    }
    let context = Context::new();
    if mode == "canceled" {
        context.cancel();
    }
    context
}
pub fn class(error: &BrowserCookieError) -> &'static str {
    match error {
        BrowserCookieError::EncryptedMalformed => "encrypted_malformed",
        BrowserCookieError::EncryptedFormatUnknown => "encrypted_format_unknown",
        BrowserCookieError::PermissionDenied => "permission_denied",
        BrowserCookieError::SecretServiceUnavailable => "secret_service_unavailable",
        BrowserCookieError::SecretServiceAccess => "secret_service_access",
        BrowserCookieError::QueryFailed => "query_failed",
        BrowserCookieError::Context(ContextError::Canceled) => "context_canceled",
        BrowserCookieError::Context(ContextError::DeadlineExceeded) => "context_deadline_exceeded",
        _ => "other",
    }
}
pub fn assert_failure(error: Option<&BrowserCookieError>, expected: &Value, id: &str) {
    assert_eq!(
        error.map(ToString::to_string).unwrap_or_default(),
        string(expected, "error"),
        "{id} error"
    );
    assert_eq!(
        error.map(class).unwrap_or(""),
        string(expected, "class"),
        "{id} class"
    );
}
pub fn assert_error<T>(
    result: Result<T, BrowserCookieError>,
    expected: &Value,
    id: &str,
) -> Option<T> {
    assert_failure(result.as_ref().err(), expected, id);
    result.ok()
}
pub fn assert_value(result: Result<SecretBytes, BrowserCookieError>, expected: &Value, id: &str) {
    assert_failure(result.as_ref().err(), expected, id);
    match result {
        Ok(value) => {
            assert_eq!(
                value.as_bytes(),
                hex(string(expected, "value_hex")),
                "{id} value"
            );
            assert_eq!(format!("{value:?}"), "<redacted>", "{id} debug");
            assert_eq!(value.to_string(), "<redacted>", "{id} display");
            assert_eq!(
                serde_json::to_value(&value).unwrap(),
                Value::String("<redacted>".into()),
                "{id} JSON"
            );
            if id == "cipher-gcm-empty-plaintext" {
                // Go's successful nil GCM slice maps to Rust's successful empty SecretBytes.
                assert!(value.as_bytes().is_empty());
                assert_eq!(expected["value_nil"], true);
            } else {
                assert_eq!(expected["value_nil"], false, "{id} success presence");
            }
        }
        Err(_) => {
            assert_eq!(expected["value_nil"], true, "{id} error presence");
            assert_eq!(expected["value_hex"], "", "{id} error bytes");
        }
    }
}

pub struct Keys {
    pub keys: Vec<Vec<u8>>,
    pub error: bool,
    pub cancel: bool,
    pub calls: AtomicUsize,
}
impl Keys {
    pub fn from_input(input: &Value) -> Arc<Self> {
        let keys = input["keys_hex"]
            .as_array()
            .map(|keys| keys.iter().map(|v| hex(v.as_str().unwrap())).collect())
            .unwrap_or_else(|| vec![hex(string(input, "key_hex"))]);
        Arc::new(Self {
            keys,
            error: string(input, "key_error") == "access",
            cancel: input["cancel_inside_key_hook"].as_bool().unwrap_or(false),
            calls: AtomicUsize::new(0),
        })
    }
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
impl ChromiumKeySource for Keys {
    fn keys(&self, context: &Context) -> Result<Vec<SecretBytes>, BrowserCookieError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.cancel {
            context.cancel();
        }
        if self.error {
            return Err(BrowserCookieError::SecretServiceAccess);
        }
        Ok(self.keys.iter().cloned().map(SecretBytes::new).collect())
    }
    fn unprotect_legacy(&self, _: &Context, _: &[u8]) -> Result<SecretBytes, BrowserCookieError> {
        panic!("Linux decoder must not invoke Windows DPAPI")
    }
}

pub struct StateFile {
    pub mode: String,
    pub body: Vec<u8>,
    pub events: Arc<Mutex<Vec<&'static str>>>,
}
impl BrowserFiles for StateFile {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        assert_eq!(path, Path::new("/synthetic/browser/Local State"));
        self.events.lock().unwrap().push("state");
        match self.mode.as_str() {
            "missing" => Err(io::ErrorKind::NotFound.into()),
            "permission_denied" => Err(io::ErrorKind::PermissionDenied.into()),
            "directory" => Err(io::Error::other("synthetic is-directory")),
            _ => Ok(self.body.clone()),
        }
    }
    fn read_dir(&self, _: &Path) -> io::Result<Vec<BrowserDirEntry>> {
        panic!("key source does not list profiles")
    }
    fn metadata(&self, _: &Path) -> io::Result<BrowserMetadata> {
        panic!("key source does not stat cookies")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SecretCall {
    Lookup(OsString),
    Run(OsString, Vec<OsString>, ProcessStdio),
}
pub struct SecretHost {
    pub mode: String,
    pub password: Vec<u8>,
    pub calls: Arc<Mutex<Vec<SecretCall>>>,
    pub events: Arc<Mutex<Vec<&'static str>>>,
}
impl HostProcess for SecretHost {
    fn platform(&self) -> HostPlatform {
        HostPlatform::Linux
    }
    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        self.calls
            .lock()
            .unwrap()
            .push(SecretCall::Lookup(program.to_owned()));
        if self.mode == "missing" {
            Err(HostProcessError::Lookup {
                program: program.to_owned(),
                source: io::ErrorKind::NotFound.into(),
            })
        } else {
            Ok(PathBuf::from("/synthetic/bin/secret-tool"))
        }
    }
    fn run(
        &self,
        _: &OsStr,
        _: &[OsString],
        _: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        panic!("caller context is required")
    }
    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("no browser launch")
    }
}
fn status(code: i32) -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code << 8)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code as u32)
    }
}
impl ContextHostProcess for SecretHost {
    fn run_context(
        &self,
        context: &Context,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        self.calls
            .lock()
            .unwrap()
            .push(SecretCall::Run(program.to_owned(), args.to_vec(), stdio));
        self.events.lock().unwrap().push("secret");
        if self.mode == "wait" {
            context.cancel();
            return Err(HostProcessError::Context(ContextError::Canceled));
        }
        if self.mode == "fail" {
            return Err(HostProcessError::Exit {
                program: program.to_owned(),
                output: ProcessOutput {
                    status: status(17),
                    stdout: vec![],
                    stderr: b"synthetic-private-stderr\xff".to_vec(),
                },
            });
        }
        Ok(ProcessOutput {
            status: status(0),
            stdout: self.password.clone(),
            stderr: vec![],
        })
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
        panic!("Linux key source must not invoke DPAPI")
    }
    unsafe fn local_free(&self, _: *mut c_void) -> *mut c_void {
        panic!("no allocation")
    }
}
pub fn unused_dpapi() -> Arc<WindowsDpapi> {
    Arc::new(WindowsDpapi::new(Arc::new(UnusedDpapi)))
}
