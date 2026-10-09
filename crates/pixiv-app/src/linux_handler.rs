use crate::{
    callback_handler::{
        CallbackResult, FileCallbackEndpointStore, PreviousHandler, callback_endpoint_path,
        clean_native_path, user_home_directory,
    },
    config::{ConfigError, private_file},
    handler_manifest::{
        HandlerFileSnapshot, HandlerManifest, HandlerManifestError, HandlerManifestStore,
    },
    handoff_client::{HandoffFuture, go_trim},
    host_context::ContextHostProcess,
    host_process::{HostProcessError, ProcessStdio, SystemHostProcess},
    lifecycle::Context,
};
use std::{
    collections::HashSet,
    error::Error,
    ffi::{OsStr, OsString},
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub const LINUX_DESKTOP_FILE: &str = "pixiv-cli-url-handler.desktop";
pub const LINUX_PIXIV_SCHEME: &str = "x-scheme-handler/pixiv";

pub trait LinuxEnvironment: Send + Sync {
    fn variable(&self, name: &str) -> Option<OsString>;
    fn home_directory(&self) -> io::Result<PathBuf>;
    fn executable_path(&self) -> io::Result<PathBuf>;
}
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLinuxEnvironment;
impl LinuxEnvironment for SystemLinuxEnvironment {
    fn variable(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }
    fn home_directory(&self) -> io::Result<PathBuf> {
        user_home_directory()
    }
    fn executable_path(&self) -> io::Result<PathBuf> {
        std::env::current_exe()
    }
}

#[derive(Debug)]
pub enum LinuxHandlerError {
    Message(&'static str),
    Io(io::Error),
    Manifest(HandlerManifestError),
    Storage(ConfigError),
    Endpoint(crate::callback_handler::CallbackEndpointError),
    Process(&'static str, HostProcessError),
    Joined(Vec<LinuxHandlerError>),
}
impl fmt::Display for LinuxHandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Io(error) => error.fmt(f),
            Self::Manifest(error) => error.fmt(f),
            Self::Storage(error) => error.fmt(f),
            Self::Endpoint(error) => error.fmt(f),
            Self::Process(prefix, error) => write!(f, "{prefix}: {error}"),
            Self::Joined(errors) => {
                for (index, error) in errors.iter().enumerate() {
                    if index != 0 {
                        f.write_str("\n")?;
                    }
                    error.fmt(f)?;
                }
                Ok(())
            }
        }
    }
}
impl Error for LinuxHandlerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Manifest(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Endpoint(error) => Some(error),
            Self::Process(_, error) => Some(error),
            Self::Message(_) | Self::Joined(_) => None,
        }
    }
}
impl From<io::Error> for LinuxHandlerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<HandlerManifestError> for LinuxHandlerError {
    fn from(error: HandlerManifestError) -> Self {
        Self::Manifest(error)
    }
}
impl From<ConfigError> for LinuxHandlerError {
    fn from(error: ConfigError) -> Self {
        Self::Storage(error)
    }
}

#[derive(Clone)]
pub struct LinuxHandler {
    environment: Arc<dyn LinuxEnvironment>,
    process: Arc<dyn ContextHostProcess>,
    manifest: Option<HandlerManifestStore>,
    endpoint: Option<PathBuf>,
}
impl LinuxHandler {
    pub fn new(
        environment: Arc<dyn LinuxEnvironment>,
        process: Arc<dyn ContextHostProcess>,
        manifest: HandlerManifestStore,
        endpoint: impl Into<PathBuf>,
    ) -> Self {
        Self {
            environment,
            process,
            manifest: Some(manifest),
            endpoint: Some(endpoint.into()),
        }
    }
    pub fn system() -> Self {
        Self {
            environment: Arc::new(SystemLinuxEnvironment),
            process: Arc::new(SystemHostProcess),
            manifest: None,
            endpoint: None,
        }
    }
    fn manifest_store(&self) -> Result<HandlerManifestStore, LinuxHandlerError> {
        self.manifest.clone().map(Ok).unwrap_or_else(|| {
            HandlerManifestStore::default_path().map_err(LinuxHandlerError::Manifest)
        })
    }
    fn endpoint_path(&self) -> Result<PathBuf, LinuxHandlerError> {
        self.endpoint
            .clone()
            .map(Ok)
            .unwrap_or_else(|| callback_endpoint_path().map_err(LinuxHandlerError::Io))
    }
    fn variable(&self, name: &str) -> OsString {
        self.environment
            .variable(name)
            .map(|value| trim_native(&value))
            .unwrap_or_default()
    }
    pub fn applications_directory(&self) -> Result<PathBuf, LinuxHandlerError> {
        let data = self.variable("XDG_DATA_HOME");
        let path = if data.is_empty() {
            self.environment.home_directory()?.join(".local/share")
        } else {
            PathBuf::from(data)
        };
        Ok(clean_native_path(&path.join("applications")))
    }
    fn mime_paths(&self) -> Vec<PathBuf> {
        let Ok(home) = self.environment.home_directory() else {
            return Vec::new();
        };
        let config = self.variable("XDG_CONFIG_HOME");
        let data = self.variable("XDG_DATA_HOME");
        let config = if config.is_empty() {
            home.join(".config")
        } else {
            PathBuf::from(config)
        };
        let data = if data.is_empty() {
            home.join(".local/share")
        } else {
            PathBuf::from(data)
        };
        vec![
            config.join("mimeapps.list"),
            data.join("applications/mimeapps.list"),
        ]
    }
    pub fn desktop_file_path(&self, id: &str) -> Result<PathBuf, LinuxHandlerError> {
        let id = go_trim(id);
        if id.is_empty() || id.contains('/') || !id.to_ascii_lowercase().ends_with(".desktop") {
            return Err(LinuxHandlerError::Message("invalid desktop handler ID"));
        }
        let mut directories = vec![self.applications_directory()?];
        let data = self.variable("XDG_DATA_DIRS");
        let data = if data.is_empty() {
            OsString::from("/usr/local/share:/usr/share")
        } else {
            data
        };
        directories.extend(
            split_native_paths(&data)
                .into_iter()
                .filter(|value| !value.is_empty())
                .map(|value| PathBuf::from(value).join("applications")),
        );
        let mut seen = HashSet::new();
        for directory in directories {
            let directory = clean_native_path(&directory);
            if !seen.insert(directory.clone()) {
                continue;
            }
            let candidate = directory.join(id);
            match fs::metadata(&candidate) {
                Ok(metadata) if metadata.is_file() => return Ok(candidate),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(io::Error::from(io::ErrorKind::NotFound).into())
    }
    fn query_default(&self, context: &Context) -> Result<String, LinuxHandlerError> {
        let output = self
            .process
            .run_context(
                context,
                OsStr::new("xdg-mime"),
                &["query".into(), "default".into(), LINUX_PIXIV_SCHEME.into()],
                ProcessStdio::CaptureStdout,
            )
            .map_err(|error| LinuxHandlerError::Process("query xdg-mime", error))?;
        Ok(go_trim(&crate::auth_bundle::go_utf8(&output.stdout)).to_owned())
    }
    fn set_default(&self, context: &Context, id: &str) -> Result<(), LinuxHandlerError> {
        self.process
            .run_context(
                context,
                OsStr::new("xdg-mime"),
                &["default".into(), id.into(), LINUX_PIXIV_SCHEME.into()],
                ProcessStdio::Discard,
            )
            .map_err(|error| LinuxHandlerError::Process("run xdg-mime", error))?;
        Ok(())
    }
    fn snapshot(&self, desktop: &Path) -> Result<Vec<NativeSnapshot>, LinuxHandlerError> {
        let mut paths = self.mime_paths();
        paths.push(desktop.to_owned());
        let mut seen = HashSet::new();
        paths
            .into_iter()
            .map(|path| clean_native_path(&path))
            .filter(|path| seen.insert(path.clone()))
            .map(|path| snapshot_file(&path))
            .collect()
    }
    pub fn ensure_persistent(&self, context: &Context) -> Result<(), LinuxHandlerError> {
        for (command, message) in [
            (
                "xdg-mime",
                "xdg-mime is required for the desktop Pixiv callback handler",
            ),
            (
                "gio",
                "gio is required for the desktop Pixiv callback handler",
            ),
        ] {
            self.process
                .look_path(OsStr::new(command))
                .map_err(|_| LinuxHandlerError::Message(message))?;
        }
        let executable = self.environment.executable_path()?;
        let desktop = self.applications_directory()?.join(LINUX_DESKTOP_FILE);
        let store = self.manifest_store()?;
        let existing = store.load()?;
        let first = existing.is_none();
        let snapshots = match &existing {
            Some(manifest) => manifest
                .linux_mime_snapshots
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(NativeSnapshot::from_saved)
                .collect(),
            None => Vec::new(),
        };
        let mut manifest = match existing {
            Some(mut manifest) => {
                manifest.executable_path = go_native_string(executable.as_os_str());
                manifest
            }
            None => HandlerManifest {
                version: 1,
                executable_path: go_native_string(executable.as_os_str()),
                previous_handler: self.query_default(context)?,
                linux_mime_snapshots: None,
                ..HandlerManifest::default()
            },
        };
        let snapshots = if first {
            self.snapshot(&desktop)?
        } else {
            snapshots
        };
        private_file::write(&desktop, &desktop_entry_native(executable.as_os_str()))?;
        if let Err(error) = self.set_default(context, LINUX_DESKTOP_FILE) {
            let _ = restore(&snapshots);
            return Err(error);
        }
        if first {
            manifest.linux_mime_snapshots =
                Some(snapshots.iter().map(NativeSnapshot::saved).collect());
        }
        if let Err(error) = store.save(&manifest) {
            let _ = restore(&snapshots);
            return Err(error.into());
        }
        Ok(())
    }
    pub fn disable_persistent(&self, context: &Context) -> Result<(), LinuxHandlerError> {
        let store = self.manifest_store()?;
        let Some(manifest) = store.load()? else {
            return Ok(());
        };
        if self.query_default(context)? == LINUX_DESKTOP_FILE {
            let snapshots = manifest.linux_mime_snapshots.as_deref().unwrap_or_default();
            if !snapshots.is_empty() {
                restore(
                    &snapshots
                        .iter()
                        .map(NativeSnapshot::from_saved)
                        .collect::<Vec<_>>(),
                )?;
            } else if !manifest.previous_handler.is_empty()
                && manifest.previous_handler != LINUX_DESKTOP_FILE
            {
                self.set_default(context, &manifest.previous_handler)?;
                remove(&self.applications_directory()?.join(LINUX_DESKTOP_FILE))?;
            } else if manifest.previous_handler.is_empty() {
                return Err(LinuxHandlerError::Message(
                    "cannot safely restore a previous Linux Pixiv URL handler",
                ));
            }
        }
        store.remove()?;
        Ok(())
    }
    pub fn install(
        &self,
        context: &Context,
        relay: &str,
    ) -> Result<LinuxInstallation, LinuxHandlerError> {
        let relay = crate::callback_handler::ValidatedCallbackEndpoint::parse(relay)
            .map_err(LinuxHandlerError::Endpoint)?;
        let endpoint = self.endpoint_path()?;
        FileCallbackEndpointStore::new(&endpoint)
            .write_validated(&relay)
            .map_err(LinuxHandlerError::Endpoint)?;
        let result = (|| {
            let executable = self.environment.executable_path()?;
            let desktop = self.applications_directory()?.join(LINUX_DESKTOP_FILE);
            let snapshots = self.snapshot(&desktop)?;
            private_file::write(&desktop, &desktop_entry_native(executable.as_os_str()))?;
            if let Err(error) = self.set_default(context, LINUX_DESKTOP_FILE) {
                let _ = restore(&snapshots);
                return Err(error);
            }
            Ok(LinuxInstallation {
                endpoint: endpoint.clone(),
                snapshots: Some(snapshots),
            })
        })();
        if result.is_err() {
            let _ = remove(&endpoint);
        }
        result
    }
    pub fn delegate_previous(&self, context: &Context, url: &str) -> Result<(), LinuxHandlerError> {
        let manifest = self
            .manifest_store()?
            .load()?
            .filter(|manifest| {
                !manifest.previous_handler.is_empty()
                    && manifest.previous_handler != LINUX_DESKTOP_FILE
            })
            .ok_or(LinuxHandlerError::Message(
                "no previous Pixiv URL handler is available",
            ))?;
        self.process.look_path(OsStr::new("gio")).map_err(|_| {
            LinuxHandlerError::Message("gio is required to open the previous Pixiv URL handler")
        })?;
        let desktop = self
            .desktop_file_path(&manifest.previous_handler)
            .map_err(|_| {
                LinuxHandlerError::Message("could not locate previous Pixiv URL handler")
            })?;
        self.process
            .run_context(
                context,
                OsStr::new("gio"),
                &["launch".into(), desktop.into_os_string(), url.into()],
                ProcessStdio::Discard,
            )
            .map_err(|_| LinuxHandlerError::Message("could not open previous Pixiv URL handler"))?;
        Ok(())
    }
}
impl PreviousHandler for LinuxHandler {
    fn delegate<'a>(
        &'a self,
        raw_url: &'a str,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<()>> {
        Box::pin(async move {
            let handler = self.clone();
            let url = raw_url.to_owned();
            let context = Context::new();
            if cancellation.is_cancelled() {
                context.cancel();
            }
            let _cancellation = CancelOnDrop(context.clone());
            let task_context = context.clone();
            let mut task =
                tokio::task::spawn_blocking(move || handler.delegate_previous(&task_context, &url));
            let result = tokio::select! {
                result = &mut task => result,
                _ = cancellation.cancelled() => { context.cancel(); task.await },
            };
            result
                .map_err(|_| io::Error::other("could not open previous Pixiv URL handler"))?
                .map_err(|error| Box::new(error) as crate::callback_handler::CallbackError)
        })
    }
}

pub struct LinuxInstallation {
    endpoint: PathBuf,
    snapshots: Option<Vec<NativeSnapshot>>,
}
impl LinuxInstallation {
    pub fn cleanup(mut self) -> Result<(), LinuxHandlerError> {
        self.restore()
    }
    fn restore(&mut self) -> Result<(), LinuxHandlerError> {
        let Some(snapshots) = self.snapshots.take() else {
            return Ok(());
        };
        let _ = remove(&self.endpoint);
        restore(&snapshots)
    }
}
impl Drop for LinuxInstallation {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub fn desktop_entry(executable: &str) -> String {
    String::from_utf8(desktop_entry_native(OsStr::new(executable)))
        .expect("UTF-8 executable produces UTF-8 desktop entry")
}

fn snapshot_file(path: &Path) -> Result<NativeSnapshot, LinuxHandlerError> {
    let mut snapshot = NativeSnapshot {
        path: path.to_owned(),
        exists: false,
        mode: 0,
        content: None,
    };
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(snapshot),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        return Err(LinuxHandlerError::Message(
            "desktop integration state is not a regular file",
        ));
    }
    snapshot.exists = true;
    snapshot.mode = permission_mode(&metadata)?;
    snapshot.content = Some(fs::read(path)?);
    Ok(snapshot)
}
fn remove(path: &Path) -> io::Result<()> {
    private_file::remove_if_exists(path)
}

fn restore(snapshots: &[NativeSnapshot]) -> Result<(), LinuxHandlerError> {
    let mut errors = Vec::new();
    for snapshot in snapshots {
        let path = &snapshot.path;
        let result = if !snapshot.exists {
            remove(path).map_err(LinuxHandlerError::Io)
        } else {
            restore_file(path, snapshot)
        };
        if let Err(error) = result {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(LinuxHandlerError::Joined(errors))
    }
}
#[cfg(unix)]
fn restore_file(path: &Path, snapshot: &NativeSnapshot) -> Result<(), LinuxHandlerError> {
    use std::{
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    };
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(private_file::directory(path))?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(native_permission_bits(snapshot.mode))
        .open(path)?;
    file.write_all(snapshot.content.as_deref().unwrap_or_default())?;
    private_file::close(file)?;
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(native_permission_bits(snapshot.mode)),
    )?;
    Ok(())
}
#[cfg(not(unix))]
fn restore_file(_: &Path, _: &NativeSnapshot) -> Result<(), LinuxHandlerError> {
    Err(LinuxHandlerError::Message(
        "Linux file permissions are unavailable on this platform",
    ))
}

#[cfg(unix)]
fn permission_mode(metadata: &fs::Metadata) -> Result<u32, LinuxHandlerError> {
    use std::os::unix::fs::PermissionsExt;
    Ok(metadata.permissions().mode() & 0o777)
}
#[cfg(not(unix))]
fn permission_mode(_: &fs::Metadata) -> Result<u32, LinuxHandlerError> {
    Err(LinuxHandlerError::Message(
        "Linux file permissions are unavailable on this platform",
    ))
}

struct CancelOnDrop(Context);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct NativeSnapshot {
    path: PathBuf,
    exists: bool,
    mode: u32,
    content: Option<Vec<u8>>,
}
impl NativeSnapshot {
    fn from_saved(value: &HandlerFileSnapshot) -> Self {
        Self {
            path: PathBuf::from(&value.path),
            exists: value.exists,
            mode: value.mode,
            content: value.content.clone(),
        }
    }
    fn saved(&self) -> HandlerFileSnapshot {
        HandlerFileSnapshot {
            path: go_native_string(self.path.as_os_str()),
            exists: self.exists,
            mode: self.mode,
            content: self.content.clone(),
        }
    }
}
#[cfg(unix)]
fn trim_native(value: &OsStr) -> OsString {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let mut bytes = value.as_bytes();
    loop {
        let whitespace = (1..=bytes.len().min(4)).find(|&length| {
            std::str::from_utf8(&bytes[..length])
                .is_ok_and(|value| value.chars().count() == 1 && go_trim(value).is_empty())
        });
        match whitespace {
            Some(length) => bytes = &bytes[length..],
            None => break,
        }
    }
    loop {
        let whitespace = (1..=bytes.len().min(4)).find(|&length| {
            std::str::from_utf8(&bytes[bytes.len() - length..])
                .is_ok_and(|value| value.chars().count() == 1 && go_trim(value).is_empty())
        });
        match whitespace {
            Some(length) => bytes = &bytes[..bytes.len() - length],
            None => break,
        }
    }
    OsString::from_vec(bytes.to_vec())
}
#[cfg(not(unix))]
fn trim_native(value: &OsStr) -> OsString {
    go_trim(&value.to_string_lossy()).into()
}
#[cfg(unix)]
fn split_native_paths(value: &OsStr) -> Vec<OsString> {
    use std::os::unix::ffi::OsStrExt;
    value
        .as_bytes()
        .split(|byte| *byte == b':')
        .map(|bytes| trim_native(OsStr::from_bytes(bytes)))
        .collect()
}
#[cfg(not(unix))]
fn split_native_paths(value: &OsStr) -> Vec<OsString> {
    value
        .to_string_lossy()
        .split(':')
        .map(|value| go_trim(value).into())
        .collect()
}
pub fn desktop_entry_native(executable: &OsStr) -> Vec<u8> {
    let mut entry = b"[Desktop Entry]\nType=Application\nName=Pixiv CLI OAuth Callback\nNoDisplay=true\nExec=\"".to_vec();
    for byte in executable.as_encoded_bytes() {
        if matches!(*byte, b'\\' | b'"' | b'$' | b'`') {
            entry.push(b'\\');
        }
        entry.push(*byte);
    }
    entry.extend_from_slice(b"\" auth _callback %u\nMimeType=x-scheme-handler/pixiv;\n");
    entry
}

fn go_native_string(value: &OsStr) -> String {
    crate::auth_bundle::go_utf8(value.as_encoded_bytes())
}

#[cfg(unix)]
fn native_permission_bits(mode: u32) -> u32 {
    (mode & 0o777)
        | if mode & (1 << 23) != 0 { 0o4000 } else { 0 }
        | if mode & (1 << 22) != 0 { 0o2000 } else { 0 }
        | if mode & (1 << 20) != 0 { 0o1000 } else { 0 }
}
