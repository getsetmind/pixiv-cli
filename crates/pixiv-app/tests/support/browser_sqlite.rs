use pixiv_app::{
    host_context::ContextHostProcess,
    host_process::{
        HostPlatform, HostProcess, HostProcessError, ProcessOutput, ProcessStdio, SystemHostProcess,
    },
    lifecycle::Context,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    path::PathBuf,
    process::ExitStatus,
    sync::Mutex,
};

#[derive(Deserialize)]
pub struct Fixture {
    pub reference: String,
    pub sources: BTreeMap<String, String>,
    pub sqlite_shell: BTreeMap<String, String>,
    pub cases: Vec<Case>,
}

#[derive(Deserialize)]
pub struct Case {
    pub name: String,
    pub operation: String,
    pub input: Input,
    pub output: Expected,
}

#[derive(Clone, Default, Deserialize)]
pub struct Input {
    pub db_path: String,
    pub sql: String,
    pub params: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub stdout_hex: String,
    #[serde(default)]
    pub stderr_hex: String,
    #[serde(default)]
    pub exit: i32,
    #[serde(default)]
    pub missing_command: bool,
    #[serde(default)]
    pub pre_cancel: bool,
    #[serde(default)]
    pub active_cancel: bool,
    #[serde(default)]
    pub schema: String,
    pub statements: Option<Vec<String>>,
    #[serde(default)]
    pub database_state: String,
    #[serde(default)]
    pub exclusive_lock: bool,
    #[serde(default)]
    pub boundary: String,
}

#[derive(Deserialize)]
pub struct Expected {
    pub command: Option<Value>,
    pub error: String,
    pub rows_hex: Option<Vec<Vec<String>>>,
}

pub fn fixture() -> Fixture {
    serde_json::from_str(include_str!(
        "../../../pixiv-cli/tests/fixtures/browser-sqliteio.json"
    ))
    .unwrap()
}

pub fn decode_hex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
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

pub fn rows_hex(rows: &[Vec<Vec<u8>>]) -> Option<Vec<Vec<String>>> {
    (!rows.is_empty()).then(|| {
        rows.iter()
            .map(|row| row.iter().map(|field| encode_hex(field)).collect())
            .collect()
    })
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

pub fn normalized_command(args: &[OsString], root: &str) -> Value {
    assert!(args.len() >= 7);
    let args: Vec<_> = args.iter().map(|arg| arg.to_str().unwrap()).collect();
    assert_eq!((args.len() - 7) % 2, 0);
    let mut params = BTreeMap::new();
    for pair in args[5..args.len() - 2].chunks_exact(2) {
        assert_eq!(pair[0], "-cmd");
        let parts: Vec<_> = pair[1].splitn(4, ' ').collect();
        assert_eq!(&parts[..2], &[".parameter", "set"]);
        assert_eq!(parts.len(), 4);
        assert!(params.insert(parts[2], parts[3]).is_none());
    }
    json!({
        "prefix": &args[..5],
        "params": params,
        "db_path": args[args.len() - 2].replace(root, "$root"),
        "sql": args[args.len() - 1],
    })
}

pub struct RecordedHost {
    pub input: Input,
    pub calls: Mutex<Vec<Vec<OsString>>>,
    pub cancel_success: bool,
    pub spawn_failure: bool,
}

impl RecordedHost {
    pub fn new(input: Input) -> Self {
        Self {
            input,
            calls: Mutex::new(Vec::new()),
            cancel_success: false,
            spawn_failure: false,
        }
    }
}

impl HostProcess for RecordedHost {
    fn platform(&self) -> HostPlatform {
        HostPlatform::Windows
    }

    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        assert_eq!(program, "sqlite3");
        if self.input.missing_command {
            Err(HostProcessError::Lookup {
                program: program.into(),
                source: io::Error::new(io::ErrorKind::NotFound, "synthetic-sensitive-value"),
            })
        } else {
            Ok(PathBuf::from("/owned-synthetic-bin/sqlite3"))
        }
    }

    fn run(
        &self,
        _: &OsStr,
        _: &[OsString],
        _: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        panic!("SQLite must preserve the caller context")
    }

    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("SQLite must not open browsers")
    }
}

impl ContextHostProcess for RecordedHost {
    fn run_context(
        &self,
        context: &Context,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        assert_eq!(stdio, ProcessStdio::Capture);
        self.look_path(program)?;
        if let Some(reason) = context.error() {
            return Err(HostProcessError::Context(reason));
        }
        self.calls.lock().unwrap().push(args.to_vec());
        if self.spawn_failure {
            return Err(HostProcessError::Spawn {
                program: program.into(),
                source: io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "synthetic-sensitive-value",
                ),
            });
        }
        if self.input.active_cancel || self.cancel_success {
            context.cancel();
        }
        let output = ProcessOutput {
            status: exit_status(if self.input.active_cancel {
                1
            } else {
                self.input.exit
            }),
            stdout: decode_hex(&self.input.stdout_hex),
            stderr: decode_hex(&self.input.stderr_hex),
        };
        if output.status.success() {
            Ok(output)
        } else {
            Err(HostProcessError::Exit {
                program: program.into(),
                output,
            })
        }
    }
}

pub struct PinnedHost(pub PathBuf);

impl HostProcess for PinnedHost {
    fn platform(&self) -> HostPlatform {
        HostPlatform::current()
    }

    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        assert_eq!(program, "sqlite3");
        SystemHostProcess.look_path(self.0.as_os_str())
    }

    fn run(
        &self,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        SystemHostProcess.run(self.look_path(program)?.as_os_str(), args, stdio)
    }

    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("SQLite must not open browsers")
    }
}

impl ContextHostProcess for PinnedHost {
    fn run_context(
        &self,
        context: &Context,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        assert_eq!(stdio, ProcessStdio::Capture);
        SystemHostProcess.run_context(context, self.look_path(program)?.as_os_str(), args, stdio)
    }
}
