use super::schema::Case;
use pixiv_app::{
    browser_cookies::{BrowserDirEntry, BrowserEnvironment, BrowserFiles, BrowserMetadata},
    host_context::ContextHostProcess,
    host_process::{HostPlatform, HostProcess, HostProcessError, ProcessOutput, ProcessStdio},
    lifecycle::Context,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

pub fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn write(path: &Path, bytes: &[u8], executable: bool) {
    private_dir(path.parent().unwrap());
    fs::write(path, bytes).unwrap();
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if executable { 0o700 } else { 0o600 }),
    )
    .unwrap();
}
fn assert_owned(home: &Path, path: &Path) {
    assert!(
        path.starts_with(home),
        "browser boundary escaped owned home"
    );
    assert!(
        !path
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    );
}

pub struct Environment {
    pub home: PathBuf,
    pub xdg_empty: bool,
}
impl BrowserEnvironment for Environment {
    fn user_home(&self) -> io::Result<PathBuf> {
        Ok(self.home.clone())
    }
    fn config_home(&self) -> Option<PathBuf> {
        (!self.xdg_empty).then(|| self.home.join("xdg-config"))
    }
}

pub struct Files {
    pub home: PathBuf,
}
impl BrowserFiles for Files {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        assert_owned(&self.home, path);
        fs::read(path)
    }
    fn open_file(&self, path: &Path) -> io::Result<Box<dyn io::Read + Send>> {
        assert_owned(&self.home, path);
        Ok(Box::new(fs::File::open(path)?))
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<BrowserDirEntry>> {
        assert_owned(&self.home, path);
        let mut entries = fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                Ok(BrowserDirEntry {
                    name: entry.file_name(),
                    is_dir: entry.file_type()?.is_dir(),
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort_by(|left, right| {
            left.name
                .as_encoded_bytes()
                .cmp(right.name.as_encoded_bytes())
        });
        Ok(entries)
    }
    fn metadata(&self, path: &Path) -> io::Result<BrowserMetadata> {
        assert_owned(&self.home, path);
        Ok(BrowserMetadata {
            is_dir: fs::metadata(path)?.is_dir(),
        })
    }
}

pub fn command_observation(home: &Path, program: &OsStr, args: &[OsString]) -> Value {
    let program = program.to_str().unwrap();
    assert!(matches!(program, "sqlite3" | "secret-tool"));
    let mut stable_args = Vec::new();
    let mut parameters = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        if program == "sqlite3" && args[index] == "-cmd" {
            index += 1;
            let parameter = args[index]
                .to_str()
                .unwrap()
                .strip_prefix(".parameter set ")
                .unwrap();
            let (name, value) = parameter.split_once(' ').unwrap();
            assert!(matches!(name, "@h1" | "@h2" | "@n"));
            assert!(
                parameters
                    .insert(name.to_owned(), value.to_owned())
                    .is_none()
            );
        } else {
            stable_args.push(
                args[index]
                    .to_str()
                    .unwrap()
                    .replace(home.to_str().unwrap(), "$HOME"),
            );
        }
        index += 1;
    }
    let mut command = json!({"program":program, "args":stable_args});
    if program == "sqlite3" {
        assert_eq!(parameters.len(), 3);
        command["parameters"] = json!(parameters);
    }
    command
}

pub struct Process {
    pub home: PathBuf,
    pub commands: Mutex<Vec<Value>>,
}
impl Process {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.into(),
            commands: Mutex::new(Vec::new()),
        }
    }
    fn capture(&self, program: &OsStr, args: &[OsString]) {
        self.commands
            .lock()
            .unwrap()
            .push(command_observation(&self.home, program, args));
    }
    fn command(&self, executable: &Path, args: &[OsString]) -> Command {
        assert_owned(&self.home, executable);
        let mut command = Command::new(executable);
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("xdg-config"))
            .env("XDG_DATA_HOME", self.home.join("xdg-data"))
            .env("XDG_CACHE_HOME", self.home.join("xdg-cache"))
            .env("XDG_STATE_HOME", self.home.join("xdg-state"))
            .env("XDG_RUNTIME_DIR", self.home.join("xdg-runtime"))
            .env("APPDATA", self.home.join("appdata"))
            .env("LOCALAPPDATA", self.home.join("local-appdata"))
            .env("TMPDIR", self.home.join("temp"))
            .env("TMP", self.home.join("temp"))
            .env("TEMP", self.home.join("temp"))
            .env("PATH", self.home.join("bin"))
            .env("TZ", "UTC")
            .current_dir(&self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    pub fn verify_sqlite(&self) -> ProcessOutput {
        let output = self
            .command(&self.home.join("bin/sqlite3"), &["-version".into()])
            .output()
            .unwrap();
        assert!(output.status.success());
        ProcessOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        }
    }
}
impl HostProcess for Process {
    fn platform(&self) -> HostPlatform {
        HostPlatform::Linux
    }
    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        assert!(matches!(program.to_str(), Some("sqlite3" | "secret-tool")));
        let path = self.home.join("bin").join(program);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 => {
                Ok(path)
            }
            _ => Err(HostProcessError::Lookup {
                program: program.into(),
                source: io::Error::new(
                    io::ErrorKind::NotFound,
                    "executable file not found in $PATH",
                ),
            }),
        }
    }
    fn run(
        &self,
        _: &OsStr,
        _: &[OsString],
        _: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        panic!("browser commands must forward the ordinary command Context")
    }
    fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
        panic!("browser import must not open URLs")
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
        let executable = self.look_path(program)?;
        if let Some(error) = context.error() {
            return Err(HostProcessError::Context(error));
        }
        assert_eq!(
            stdio,
            if program == "sqlite3" {
                ProcessStdio::Capture
            } else {
                ProcessStdio::CaptureStdout
            }
        );
        assert_eq!(
            crate::fanbox_auth_support::context_marker(context),
            "command"
        );
        self.capture(program, args);
        let output = self
            .command(&executable, args)
            .arg0(program)
            .output()
            .map_err(|source| HostProcessError::Spawn {
                program: executable.into_os_string(),
                source,
            })?;
        let output = ProcessOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
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

pub fn prepare(home: &Path, case: &Case, sqlite_bytes: &[u8]) {
    private_dir(home);
    private_dir(&home.join("bin"));
    private_dir(&home.join("temp"));
    if !case.sqlite_unavailable {
        write(&home.join("bin/sqlite3"), sqlite_bytes, true);
    }
    if case.secret_mode != "missing" {
        let result = match case.secret_mode.as_str() {
            "error" => "printf '%s' 'owned-private-secret-canary permission denied' >&2\nexit 7\n"
                .to_owned(),
            "empty" => "exit 0\n".to_owned(),
            "value" => {
                let octal = unhex(&case.secret_hex)
                    .iter()
                    .map(|byte| format!("\\{byte:03o}"))
                    .collect::<String>();
                format!("printf '%b' '{octal}'\n")
            }
            other => panic!("unknown frozen secret mode {other}"),
        };
        let script = format!(
            "#!/bin/sh\n[ \"$#\" -eq 5 ] || exit 92\n[ \"$1\" = lookup ] && [ \"$2\" = xdg:schema ] && [ \"$3\" = chrome_libsecret ] && [ \"$4\" = application ] || exit 93\ncase \"$5\" in chrome|microsoft-edge) ;; *) exit 94 ;; esac\n{result}"
        );
        write(&home.join("bin/secret-tool"), script.as_bytes(), true);
    }
    let base = home.join(if case.xdg_empty {
        ".config"
    } else {
        "xdg-config"
    });
    let browser = case.browser.trim().to_ascii_lowercase();
    let root = base.join(match browser.as_str() {
        "chrome" => "google-chrome",
        "edge" => "microsoft-edge",
        "firefox" => "mozilla/firefox",
        "safari" => "google-chrome",
        other => panic!("unclassified frozen browser {other}"),
    });
    for profile in &case.profiles {
        assert_eq!(
            Path::new(&profile.path)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap(),
            profile.id
        );
        let directory = if let Some(path) = profile.path.strip_prefix("$HOME/") {
            home.join(path)
        } else {
            root.join(&profile.path)
        };
        assert_owned(home, &directory);
        private_dir(&directory);
        let path = directory.join(if browser == "firefox" {
            "cookies.sqlite"
        } else {
            "Cookies"
        });
        match profile.mode.as_str() {
            "missing" => continue,
            "directory" => {
                private_dir(&path);
                continue;
            }
            "invalid" => {
                write(&path, b"owned-invalid-database", false);
                continue;
            }
            "" | "missing-table" => {}
            other => panic!("unknown frozen profile mode {other}"),
        }
        let database = Connection::open(&path).unwrap();
        if profile.mode == "missing-table" {
            database
                .execute_batch("CREATE TABLE owned_unrelated(value TEXT)")
                .unwrap();
        } else {
            database
                .execute_batch(if browser == "firefox" {
                    "CREATE TABLE moz_cookies(host TEXT,name TEXT,value TEXT)"
                } else {
                    "CREATE TABLE cookies(host_key TEXT,name TEXT,value TEXT,encrypted_value BLOB)"
                })
                .unwrap();
            for cookie in profile.cookies.as_deref().unwrap_or_default() {
                let value = unhex(&cookie.value_hex);
                let encrypted = unhex(&cookie.encrypted_hex);
                if browser == "firefox" {
                    database
                        .execute(
                            "INSERT INTO moz_cookies VALUES(?,?,CAST(? AS TEXT))",
                            rusqlite::params![cookie.host, cookie.name, value],
                        )
                        .unwrap();
                } else {
                    database
                        .execute(
                            "INSERT INTO cookies VALUES(?,?,CAST(? AS TEXT),?)",
                            rusqlite::params![cookie.host, cookie.name, value, encrypted],
                        )
                        .unwrap();
                }
            }
        }
        database.close().unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    if let Some(ini) = &case.firefox_ini {
        write(
            &root.join("profiles.ini"),
            ini.replace("$HOME", home.to_str().unwrap()).as_bytes(),
            false,
        );
    }
    for file in &case.files {
        let path = if let Some(path) = file.path.strip_prefix("$ROOT/") {
            root.join(path)
        } else {
            home.join(&file.path)
        };
        assert_owned(home, &path);
        if file.mode == "directory" {
            private_dir(&path);
        } else {
            assert!(file.mode.is_empty());
            write(&path, &unhex(&file.bytes_hex), false);
        }
    }
}
