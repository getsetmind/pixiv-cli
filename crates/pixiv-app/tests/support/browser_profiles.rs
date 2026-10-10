use pixiv_app::{
    browser_cookies::{
        BrowserDirEntry, BrowserFiles, BrowserMetadata, BrowserProfile, SecretBytes,
    },
    host_context::ContextHostProcess,
    host_process::{HostPlatform, HostProcess, HostProcessError, ProcessOutput, ProcessStdio},
    lifecycle::Context,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::Mutex,
};

#[derive(Default)]
pub struct OwnedFiles {
    pub errors: Mutex<BTreeMap<PathBuf, io::ErrorKind>>,
    pub trace: Mutex<Vec<(String, PathBuf)>>,
    pub cancel_on_read: Mutex<Option<Context>>,
}
impl OwnedFiles {
    fn check(&self, operation: &str, path: &Path) -> io::Result<()> {
        self.trace
            .lock()
            .unwrap()
            .push((operation.into(), path.into()));
        if let Some(kind) = self.errors.lock().unwrap().get(path).copied() {
            return Err(io::Error::new(
                kind,
                "synthetic private filesystem diagnostic",
            ));
        }
        Ok(())
    }
}
impl BrowserFiles for OwnedFiles {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.check("read", path)?;
        let result = fs::read(path);
        if let Some(context) = self.cancel_on_read.lock().unwrap().as_ref() {
            context.cancel();
        }
        result
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<BrowserDirEntry>> {
        self.check("read_dir", path)?;
        fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                Ok(BrowserDirEntry {
                    name: entry.file_name(),
                    is_dir: entry.file_type()?.is_dir(),
                })
            })
            .collect()
    }
    fn metadata(&self, path: &Path) -> io::Result<BrowserMetadata> {
        self.check("metadata", path)?;
        Ok(BrowserMetadata {
            is_dir: fs::metadata(path)?.is_dir(),
        })
    }
}

pub fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(text, 16).unwrap()
        })
        .collect()
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
#[cfg(unix)]
pub fn os(bytes: Vec<u8>) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes)
}
#[cfg(not(unix))]
pub fn os(bytes: Vec<u8>) -> OsString {
    OsString::from(String::from_utf8(bytes).unwrap())
}
pub fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
pub fn replace(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    assert!(!from.is_empty());
    let mut result = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset..].starts_with(from) {
            result.extend_from_slice(to);
            offset += from.len();
        } else {
            result.push(bytes[offset]);
            offset += 1;
        }
    }
    result
}
pub fn firefox_profiles(profiles: &[BrowserProfile], root: &Path) -> Value {
    Value::Array(profiles.iter().map(|profile| json!({
        "id_hex": hex(&profile.id),
        "name_hex": hex(&profile.name),
        "path_hex": hex(&replace(profile.path.as_os_str().as_encoded_bytes(), root.as_os_str().as_encoded_bytes(), b"$root")),
    })).collect())
}
pub fn safari_profiles(profiles: &[BrowserProfile], root: &Path) -> Value {
    Value::Array(profiles.iter().map(|profile| json!({
        "id": std::str::from_utf8(&profile.id).unwrap(),
        "name": std::str::from_utf8(&profile.name).unwrap(),
        "path": std::str::from_utf8(&replace(profile.path.as_os_str().as_encoded_bytes(), root.as_os_str().as_encoded_bytes(), b"$home")).unwrap(),
    })).collect())
}
pub fn values(values: Vec<SecretBytes>) -> Value {
    Value::Array(
        values
            .into_iter()
            .map(|value| Value::String(hex(value.as_bytes())))
            .collect(),
    )
}
pub fn context(input: &Value) -> Context {
    let context = Context::new();
    if input["pre_cancel"].as_bool() == Some(true) {
        context.cancel();
    }
    context
}
pub fn nodes(root: &Path, input: &Value, encoded_paths: bool) {
    let Some(nodes) = input["nodes"].as_array() else {
        return;
    };
    for node in nodes {
        let path = root.join(if encoded_paths {
            os(unhex(node["path_hex"].as_str().unwrap()))
        } else {
            OsString::from(node["path"].as_str().unwrap())
        });
        match node["kind"].as_str().unwrap() {
            "directory" => fs::create_dir_all(path).unwrap(),
            "symlink" => {
                let target = root.join(if encoded_paths {
                    os(unhex(node["target_hex"].as_str().unwrap()))
                } else {
                    OsString::from(node["target"].as_str().unwrap())
                });
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, path).unwrap();
                #[cfg(not(unix))]
                panic!("this sealed path/symlink corpus is Linux-only: {target:?} {path:?}");
            }
            "file" => write(
                &path,
                &node["data_hex"].as_str().map_or_else(Vec::new, unhex),
            ),
            other => panic!("unsupported frozen node kind: {other}"),
        }
    }
}
#[cfg(unix)]
fn status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(code << 8)
}
#[cfg(windows)]
fn status(code: i32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(code as u32)
}

type ProcessObservation = (OsString, Vec<OsString>, ProcessStdio, usize);

pub struct CapturedProcess {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: i32,
    pub missing: bool,
    pub observed: Mutex<Vec<ProcessObservation>>,
}
impl CapturedProcess {
    pub fn from_input(input: &Value) -> Self {
        Self {
            stdout: unhex(input["stdout_hex"].as_str().unwrap()),
            stderr: unhex(input["stderr_hex"].as_str().unwrap()),
            exit: input["exit"].as_i64().unwrap() as i32,
            missing: input["missing_command"].as_bool().unwrap(),
            observed: Mutex::new(Vec::new()),
        }
    }
    pub fn command(&self, root: &Path) -> Value {
        let observed = self.observed.lock().unwrap();
        let Some((program, args, stdio, _)) = observed.first() else {
            return Value::Null;
        };
        assert_eq!(observed.len(), 1);
        assert_eq!(program, "sqlite3");
        assert_eq!(*stdio, ProcessStdio::Capture);
        assert!(args.len() >= 7);
        let mut params = BTreeMap::new();
        for pair in args[5..args.len() - 2].chunks_exact(2) {
            assert_eq!(pair[0], "-cmd");
            let command = pair[1].to_str().unwrap();
            let pieces = command.splitn(4, ' ').collect::<Vec<_>>();
            assert_eq!(&pieces[..2], &[".parameter", "set"]);
            params.insert(pieces[2], pieces[3]);
        }
        json!({
            "prefix": args[..5].iter().map(|arg| arg.to_str().unwrap()).collect::<Vec<_>>(),
            "params": params,
            "db_path": std::str::from_utf8(&replace(args[args.len()-2].as_encoded_bytes(), root.as_os_str().as_encoded_bytes(), b"$root")).unwrap(),
            "sql": args.last().unwrap().to_str().unwrap(),
        })
    }
}
impl HostProcess for CapturedProcess {
    fn platform(&self) -> HostPlatform {
        HostPlatform::Linux
    }
    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        if self.missing {
            Err(HostProcessError::Lookup {
                program: program.into(),
                source: io::Error::new(
                    io::ErrorKind::NotFound,
                    "synthetic private executable diagnostic",
                ),
            })
        } else {
            Ok(PathBuf::from("/owned/sqlite3"))
        }
    }
    fn run(
        &self,
        _: &OsStr,
        _: &[OsString],
        _: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        panic!("provider must forward its Context through run_context")
    }
    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("cookie providers must not open URLs")
    }
}
impl ContextHostProcess for CapturedProcess {
    fn run_context(
        &self,
        context: &Context,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        if self.missing {
            return Err(HostProcessError::Lookup {
                program: program.into(),
                source: io::Error::new(
                    io::ErrorKind::NotFound,
                    "synthetic private executable diagnostic",
                ),
            });
        }
        self.observed.lock().unwrap().push((
            program.into(),
            args.into(),
            stdio,
            context as *const Context as usize,
        ));
        let output = ProcessOutput {
            status: status(self.exit),
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
        };
        if self.exit == 0 {
            Ok(output)
        } else {
            Err(HostProcessError::Exit {
                program: program.into(),
                output,
            })
        }
    }
}
