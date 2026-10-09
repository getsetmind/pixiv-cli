use crate::{
    auth_bundle::go_utf8,
    callback_handler::{
        CallbackResult, FileCallbackEndpointStore, PreviousHandler, ValidatedCallbackEndpoint,
        callback_endpoint_path,
    },
    config::private_file,
    handler_manifest::{HandlerManifest, HandlerManifestError, HandlerManifestStore},
    handoff_client::HandoffFuture,
    host_context::ContextHostProcess,
    host_process::{HostProcessError, ProcessStdio, SystemHostProcess},
    lifecycle::Context,
    windows_shell::{SystemWindowsShell, WindowsShell},
};
use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub const WINDOWS_REGISTRY_KEY: &str = r"HKCU\Software\Classes\pixiv";
pub const PREVIOUS_WINDOWS_PROG_ID: &str = "pixiv-cli.previous-pixiv";
pub const PREVIOUS_WINDOWS_REGISTRY_KEY: &str = r"HKCU\Software\Classes\pixiv-cli.previous-pixiv";

pub trait WindowsEnvironment: Send + Sync {
    fn executable_path(&self) -> io::Result<PathBuf>;
    fn temporary_directory(&self) -> PathBuf;
}
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemWindowsEnvironment;
impl WindowsEnvironment for SystemWindowsEnvironment {
    fn executable_path(&self) -> io::Result<PathBuf> {
        std::env::current_exe()
    }
    fn temporary_directory(&self) -> PathBuf {
        std::env::temp_dir()
    }
}

#[derive(Debug)]
pub enum WindowsHandlerError {
    Message(&'static str),
    Io(io::Error),
    Manifest(HandlerManifestError),
    Endpoint(crate::callback_handler::CallbackEndpointError),
    Process(HostProcessError),
    Registry(HostProcessError),
}
impl fmt::Display for WindowsHandlerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(value) => formatter.write_str(value),
            Self::Io(value) => value.fmt(formatter),
            Self::Manifest(value) => value.fmt(formatter),
            Self::Endpoint(value) => value.fmt(formatter),
            Self::Process(value) => value.fmt(formatter),
            Self::Registry(value) => write!(formatter, "run Windows registry command: {value}"),
        }
    }
}
impl Error for WindowsHandlerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::Io(value) => Some(value),
            Self::Manifest(value) => Some(value),
            Self::Endpoint(value) => Some(value),
            Self::Process(value) | Self::Registry(value) => Some(value),
        }
    }
}
impl From<io::Error> for WindowsHandlerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<HandlerManifestError> for WindowsHandlerError {
    fn from(value: HandlerManifestError) -> Self {
        Self::Manifest(value)
    }
}

#[derive(Clone)]
pub struct WindowsHandler {
    environment: Arc<dyn WindowsEnvironment>,
    process: Arc<dyn ContextHostProcess>,
    shell: Arc<dyn WindowsShell>,
    manifest: Option<HandlerManifestStore>,
    endpoint: Option<PathBuf>,
}
impl WindowsHandler {
    pub fn new(
        environment: Arc<dyn WindowsEnvironment>,
        process: Arc<dyn ContextHostProcess>,
        shell: Arc<dyn WindowsShell>,
        manifest: HandlerManifestStore,
        endpoint: impl Into<PathBuf>,
    ) -> Self {
        Self {
            environment,
            process,
            shell,
            manifest: Some(manifest),
            endpoint: Some(endpoint.into()),
        }
    }
    pub fn system() -> Self {
        Self {
            environment: Arc::new(SystemWindowsEnvironment),
            process: Arc::new(SystemHostProcess),
            shell: Arc::new(SystemWindowsShell),
            manifest: None,
            endpoint: None,
        }
    }
    fn manifest_store(&self) -> Result<HandlerManifestStore, WindowsHandlerError> {
        self.manifest
            .clone()
            .map(Ok)
            .unwrap_or_else(|| HandlerManifestStore::default_path().map_err(Into::into))
    }
    fn endpoint_path(&self) -> Result<PathBuf, WindowsHandlerError> {
        self.endpoint
            .clone()
            .map(Ok)
            .unwrap_or_else(|| callback_endpoint_path().map_err(Into::into))
    }
    fn registry(
        &self,
        context: &Context,
        arguments: &[OsString],
    ) -> Result<(), WindowsHandlerError> {
        self.process
            .run_context(
                context,
                OsStr::new("reg.exe"),
                arguments,
                ProcessStdio::Discard,
            )
            .map(|_| ())
            .map_err(WindowsHandlerError::Registry)
    }
    fn key_exists(&self, context: &Context) -> Result<bool, WindowsHandlerError> {
        match self.registry(context, &["query".into(), WINDOWS_REGISTRY_KEY.into()]) {
            Ok(()) => Ok(true),
            Err(WindowsHandlerError::Registry(error)) if is_exit_error(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }
    fn install_handler(
        &self,
        context: &Context,
        executable: &OsStr,
    ) -> Result<(), WindowsHandlerError> {
        self.registry(
            context,
            &[
                "add".into(),
                WINDOWS_REGISTRY_KEY.into(),
                "/ve".into(),
                "/t".into(),
                "REG_SZ".into(),
                "/d".into(),
                "URL:Pixiv Protocol".into(),
                "/f".into(),
            ],
        )?;
        self.registry(
            context,
            &[
                "add".into(),
                WINDOWS_REGISTRY_KEY.into(),
                "/v".into(),
                "URL Protocol".into(),
                "/t".into(),
                "REG_SZ".into(),
                "/d".into(),
                "".into(),
                "/f".into(),
            ],
        )?;
        self.registry(
            context,
            &[
                "add".into(),
                format!(r"{WINDOWS_REGISTRY_KEY}\shell\open\command").into(),
                "/ve".into(),
                "/t".into(),
                "REG_SZ".into(),
                "/d".into(),
                windows_url_handler_command_native(executable),
                "/f".into(),
            ],
        )
    }
    fn default_handler_is_ours(&self, context: &Context) -> Result<bool, WindowsHandlerError> {
        match self.process.run_context(
            context,
            OsStr::new("reg.exe"),
            &[
                "query".into(),
                format!(r"{WINDOWS_REGISTRY_KEY}\shell\open\command").into(),
                "/ve".into(),
            ],
            ProcessStdio::CaptureStdout,
        ) {
            Ok(output) => Ok(go_utf8(&output.stdout)
                .to_lowercase()
                .contains(" auth _callback ")),
            Err(error) if is_exit_error(&error) => Ok(false),
            Err(error) => Err(WindowsHandlerError::Process(error)),
        }
    }
    pub fn ensure_persistent(&self, context: &Context) -> Result<(), WindowsHandlerError> {
        let executable = self.environment.executable_path()?;
        let executable_text = go_utf8(executable.as_os_str().as_encoded_bytes());
        let store = self.manifest_store()?;
        let mut manifest = match store.load()? {
            Some(manifest) => manifest,
            None => {
                let previous = self.key_exists(context)?;
                let mut manifest = HandlerManifest {
                    version: 1,
                    executable_path: executable_text.clone(),
                    ..HandlerManifest::default()
                };
                if previous {
                    self.registry(
                        context,
                        &[
                            "copy".into(),
                            WINDOWS_REGISTRY_KEY.into(),
                            PREVIOUS_WINDOWS_REGISTRY_KEY.into(),
                            "/s".into(),
                            "/f".into(),
                        ],
                    )?;
                    manifest.previous_handler = PREVIOUS_WINDOWS_PROG_ID.to_owned();
                }
                manifest
            }
        };
        manifest.executable_path = executable_text;
        self.install_handler(context, executable.as_os_str())?;
        store.save(&manifest)?;
        Ok(())
    }
    pub fn disable_persistent(&self, context: &Context) -> Result<(), WindowsHandlerError> {
        let store = self.manifest_store()?;
        let Some(manifest) = store.load()? else {
            return Ok(());
        };
        if self.default_handler_is_ours(context)? {
            self.registry(
                context,
                &["delete".into(), WINDOWS_REGISTRY_KEY.into(), "/f".into()],
            )?;
            if !manifest.previous_handler.is_empty() {
                self.registry(
                    context,
                    &[
                        "copy".into(),
                        PREVIOUS_WINDOWS_REGISTRY_KEY.into(),
                        WINDOWS_REGISTRY_KEY.into(),
                        "/s".into(),
                        "/f".into(),
                    ],
                )?;
            }
        }
        if !manifest.previous_handler.is_empty() {
            self.registry(
                context,
                &[
                    "delete".into(),
                    PREVIOUS_WINDOWS_REGISTRY_KEY.into(),
                    "/f".into(),
                ],
            )?;
        }
        store.remove()?;
        Ok(())
    }
    pub fn delegate_previous(
        &self,
        context: &Context,
        raw_url: &str,
    ) -> Result<(), WindowsHandlerError> {
        let manifest = self
            .manifest_store()?
            .load()?
            .filter(|manifest| !manifest.previous_handler.is_empty())
            .ok_or(WindowsHandlerError::Message(
                "no previous Pixiv URL handler is available",
            ))?;
        self.shell
            .open_class(context, &manifest.previous_handler, raw_url)
            .map_err(|_| WindowsHandlerError::Message("could not open previous Pixiv URL handler"))
    }
    pub fn install(
        &self,
        context: &Context,
        relay: &str,
    ) -> Result<WindowsInstallation, WindowsHandlerError> {
        let relay =
            ValidatedCallbackEndpoint::parse(relay).map_err(WindowsHandlerError::Endpoint)?;
        let endpoint = self.endpoint_path()?;
        FileCallbackEndpointStore::new(&endpoint)
            .write_validated(&relay)
            .map_err(WindowsHandlerError::Endpoint)?;
        let result = (|| {
            let executable = self.environment.executable_path()?;
            let backup = RegistryBackup::new(&self.environment.temporary_directory())?;
            let previous = self.key_exists(context)?;
            if previous {
                self.registry(
                    context,
                    &[
                        "export".into(),
                        WINDOWS_REGISTRY_KEY.into(),
                        backup.path.as_os_str().to_owned(),
                        "/y".into(),
                    ],
                )?;
                set_mode(&backup.path, 0o600)?;
            }
            if let Err(error) = self.install_handler(context, executable.as_os_str()) {
                self.restore_registry(previous, &backup.path);
                return Err(error);
            }
            Ok(WindowsInstallation {
                handler: self.clone(),
                endpoint: endpoint.clone(),
                backup,
                previous,
                cleanup_on_drop: true,
            })
        })();
        if result.is_err() {
            let _ = private_file::remove_if_exists(&endpoint);
        }
        result
    }
    fn restore_registry(&self, previous: bool, backup: &Path) {
        let context = Context::new();
        let _ = self.registry(
            &context,
            &["delete".into(), WINDOWS_REGISTRY_KEY.into(), "/f".into()],
        );
        if previous {
            let _ = self.registry(&context, &["import".into(), backup.as_os_str().to_owned()]);
        }
    }
}
impl PreviousHandler for WindowsHandler {
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

pub struct WindowsInstallation {
    handler: WindowsHandler,
    endpoint: PathBuf,
    backup: RegistryBackup,
    previous: bool,
    cleanup_on_drop: bool,
}
impl WindowsInstallation {
    pub fn cleanup(&mut self) {
        self.cleanup_on_drop = false;
        let _ = private_file::remove_if_exists(&self.endpoint);
        self.handler
            .restore_registry(self.previous, &self.backup.path);
        self.backup.remove();
    }
}
impl Drop for WindowsInstallation {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            self.cleanup();
        }
    }
}
struct RegistryBackup {
    directory: PathBuf,
    path: PathBuf,
}
impl RegistryBackup {
    fn new(temporary: &Path) -> io::Result<Self> {
        for _ in 0..10000 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
            let suffix = random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let directory = temporary.join(format!("pixiv-cli-url-handler-registry-{suffix}"));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
            let backup = Self {
                path: directory.join("pixiv-url-handler.reg"),
                directory,
            };
            set_mode(&backup.directory, 0o700)?;
            return Ok(backup);
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create private registry backup",
        ))
    }
    fn remove(&self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
impl Drop for RegistryBackup {
    fn drop(&mut self) {
        self.remove();
    }
}
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)
    }
}
fn is_exit_error(error: &HostProcessError) -> bool {
    match error {
        HostProcessError::Exit { .. } => true,
        HostProcessError::Captured { source, .. } => is_exit_error(source),
        _ => false,
    }
}
fn windows_url_handler_command_native(executable: &OsStr) -> OsString {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let mut command = vec![u16::from(b'"')];
        for unit in executable.encode_wide() {
            if unit == u16::from(b'"') {
                command.push(u16::from(b'\\'));
            }
            command.push(unit);
        }
        command.extend("\" auth _callback \"%1\"".encode_utf16());
        OsString::from_wide(&command)
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let mut command = vec![b'"'];
        for byte in executable.as_bytes() {
            if *byte == b'"' {
                command.push(b'\\');
            }
            command.push(*byte);
        }
        command.extend_from_slice(b"\" auth _callback \"%1\"");
        OsString::from_vec(command)
    }
    #[cfg(not(any(unix, windows)))]
    {
        windows_url_handler_command(&executable.to_string_lossy()).into()
    }
}
pub fn windows_url_handler_command(executable: &str) -> String {
    format!(
        "\"{}\" auth _callback \"%1\"",
        executable.replace('"', "\\\"")
    )
}
struct CancelOnDrop(Context);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
