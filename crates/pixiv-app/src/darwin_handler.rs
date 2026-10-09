use crate::{
    auth_bundle::{go_quote, go_utf8},
    callback_handler::{
        CallbackResult, FileCallbackEndpointStore, PreviousHandler, callback_endpoint_path,
        clean_native_path, user_home_directory,
    },
    config::{ConfigError, private_file},
    handler_manifest::{HandlerManifest, HandlerManifestError, HandlerManifestStore},
    handoff_client::{HandoffFuture, go_trim},
    host_context::ContextHostProcess,
    host_process::{HostProcessError, ProcessStdio, SystemHostProcess},
    lifecycle::Context,
};
use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub const DARWIN_BUNDLE_ID: &str = "com.flanchan.pixiv-cli.url-handler";
pub const DARWIN_SOURCE_VERSION: &str = "6";
pub const DARWIN_SWIFT_SOURCE: &str = include_str!("darwin_handler/url-handler.swift");
pub const DARWIN_INFO_PLIST: &str = include_str!("darwin_handler/Info.plist");
pub const DARWIN_LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

pub trait DarwinEnvironment: Send + Sync {
    fn home_directory(&self) -> io::Result<PathBuf>;
    fn executable_path(&self) -> io::Result<PathBuf>;
    fn temporary_directory(&self) -> PathBuf;
}
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemDarwinEnvironment;
impl DarwinEnvironment for SystemDarwinEnvironment {
    fn home_directory(&self) -> io::Result<PathBuf> {
        user_home_directory()
    }
    fn executable_path(&self) -> io::Result<PathBuf> {
        std::env::current_exe()
    }
    fn temporary_directory(&self) -> PathBuf {
        std::env::temp_dir()
    }
}
#[derive(Debug)]
pub enum DarwinHandlerError {
    Message(&'static str),
    Io(io::Error),
    Manifest(HandlerManifestError),
    Storage(ConfigError),
    Endpoint(crate::callback_handler::CallbackEndpointError),
    Process(HostProcessError),
    Command(&'static str, HostProcessError, String),
}
impl fmt::Display for DarwinHandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(value) => f.write_str(value),
            Self::Io(value) => value.fmt(f),
            Self::Manifest(value) => value.fmt(f),
            Self::Storage(value) => value.fmt(f),
            Self::Endpoint(value) => value.fmt(f),
            Self::Process(value) => value.fmt(f),
            Self::Command(prefix, value, output) => write!(f, "{prefix}: {value}: {output}"),
        }
    }
}
impl Error for DarwinHandlerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::Io(value) => Some(value),
            Self::Manifest(value) => Some(value),
            Self::Storage(value) => Some(value),
            Self::Endpoint(value) => Some(value),
            Self::Process(value) | Self::Command(_, value, _) => Some(value),
        }
    }
}
impl From<io::Error> for DarwinHandlerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<HandlerManifestError> for DarwinHandlerError {
    fn from(value: HandlerManifestError) -> Self {
        Self::Manifest(value)
    }
}
impl From<ConfigError> for DarwinHandlerError {
    fn from(value: ConfigError) -> Self {
        Self::Storage(value)
    }
}
impl From<HostProcessError> for DarwinHandlerError {
    fn from(value: HostProcessError) -> Self {
        Self::Process(value)
    }
}

#[derive(Clone)]
pub struct DarwinHandler {
    environment: Arc<dyn DarwinEnvironment>,
    process: Arc<dyn ContextHostProcess>,
    manifest: Option<HandlerManifestStore>,
    endpoint: Option<PathBuf>,
}
impl DarwinHandler {
    pub fn new(
        environment: Arc<dyn DarwinEnvironment>,
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
            environment: Arc::new(SystemDarwinEnvironment),
            process: Arc::new(SystemHostProcess),
            manifest: None,
            endpoint: None,
        }
    }
    fn manifest_store(&self) -> Result<HandlerManifestStore, DarwinHandlerError> {
        self.manifest
            .clone()
            .map(Ok)
            .unwrap_or_else(|| HandlerManifestStore::default_path().map_err(Into::into))
    }
    fn endpoint_path(&self) -> Result<PathBuf, DarwinHandlerError> {
        self.endpoint
            .clone()
            .map(Ok)
            .unwrap_or_else(|| callback_endpoint_path().map_err(Into::into))
    }
    pub fn app_path(&self) -> Result<PathBuf, DarwinHandlerError> {
        Ok(clean_native_path(
            &self
                .environment
                .home_directory()?
                .join(".pixiv-cli/url-handler/PixivCLIURLHandler.app"),
        ))
    }
    fn combined(
        &self,
        context: &Context,
        program: &str,
        args: &[OsString],
        prefix: &'static str,
    ) -> Result<(), DarwinHandlerError> {
        self.process
            .run_context(context, OsStr::new(program), args, ProcessStdio::Combined)
            .map(|_| ())
            .map_err(|error| {
                let output = go_trim(&go_utf8(error.captured_stdout())).to_owned();
                DarwinHandlerError::Command(prefix, error, output)
            })
    }
    fn query_default(&self, context: &Context) -> Result<String, DarwinHandlerError> {
        let script = format!(
            "import Foundation; import CoreServices; let scheme={} as NSString; if let h = LSCopyDefaultHandlerForURLScheme(scheme)?.takeRetainedValue() {{ print(h as String) }}",
            go_quote("pixiv")
        );
        let output = self.process.run_context(
            context,
            OsStr::new("swift"),
            &["-e".into(), script.into()],
            ProcessStdio::CaptureStdout,
        )?;
        Ok(go_trim(&go_utf8(&output.stdout)).to_owned())
    }
    fn set_default(&self, context: &Context, bundle: &str) -> Result<(), DarwinHandlerError> {
        let script = format!(
            "import Foundation; import CoreServices; let scheme={} as NSString; let handler={} as NSString; let status = LSSetDefaultHandlerForURLScheme(scheme, handler); if status != 0 {{ exit(1) }}",
            go_quote("pixiv"),
            go_quote(bundle)
        );
        self.combined(
            context,
            "swift",
            &["-e".into(), script.into()],
            "set pixiv:// callback handler",
        )
    }
    fn register(&self, context: &Context, app: &Path) -> Result<(), DarwinHandlerError> {
        self.combined(
            context,
            DARWIN_LSREGISTER,
            &["-f".into(), app.as_os_str().to_owned()],
            "register pixiv:// callback helper",
        )
    }
    pub fn ensure_app(&self, context: &Context, app: &Path) -> Result<(), DarwinHandlerError> {
        let executable = app.join("Contents/MacOS/PixivCLIURLHandler");
        let info = app.join("Contents/Info.plist");
        let version = app.join("Contents/Resources/source-version");
        if file_exists(&executable)
            && file_exists(&info)
            && fs::read(&version)
                .is_ok_and(|bytes| go_trim(&go_utf8(&bytes)) == DARWIN_SOURCE_VERSION)
        {
            return Ok(());
        }
        create_directory(private_file::directory(&executable))?;
        let source = PrivateSource::new(&self.environment.temporary_directory())?;
        self.combined(
            context,
            "swiftc",
            &[
                source.path.as_os_str().to_owned(),
                "-o".into(),
                executable.into_os_string(),
            ],
            "compile pixiv:// callback helper",
        )?;
        write_info(&info, DARWIN_INFO_PLIST.as_bytes())?;
        private_file::write(&version, format!("{DARWIN_SOURCE_VERSION}\n").as_bytes())?;
        Ok(())
    }
    pub fn ensure_persistent(&self, context: &Context) -> Result<(), DarwinHandlerError> {
        let executable = self.environment.executable_path()?;
        let home = self.environment.home_directory()?;
        let app = self.app_path()?;
        self.ensure_app(context, &app)?;
        let store = self.manifest_store()?;
        let mut manifest = match store.load()? {
            Some(manifest) => manifest,
            None => HandlerManifest {
                version: 1,
                previous_handler: self.query_default(context)?,
                ..HandlerManifest::default()
            },
        };
        manifest.executable_path = go_utf8(executable.as_os_str().as_encoded_bytes());
        manifest.home_directory = go_utf8(home.as_os_str().as_encoded_bytes());
        save_bundle_manifest(&app, &manifest)?;
        self.register(context, &app)?;
        self.set_default(context, DARWIN_BUNDLE_ID)?;
        if let Err(error) = store.save(&manifest) {
            self.restore_previous(&manifest.previous_handler);
            return Err(error.into());
        }
        Ok(())
    }
    pub fn disable_persistent(&self, context: &Context) -> Result<(), DarwinHandlerError> {
        let store = self.manifest_store()?;
        let Some(manifest) = store.load()? else {
            return Ok(());
        };
        if self.query_default(context)? == DARWIN_BUNDLE_ID {
            if !previous_available(&manifest.previous_handler) {
                return Err(DarwinHandlerError::Message(
                    "cannot safely restore a previous macOS Pixiv URL handler",
                ));
            }
            self.set_default(context, &manifest.previous_handler)?;
        }
        store.remove()?;
        Ok(())
    }
    fn restore_previous(&self, previous: &str) {
        if previous_available(previous) {
            let _ = self.set_default(&Context::new(), previous);
        }
    }
    pub fn install(
        &self,
        context: &Context,
        relay: &str,
    ) -> Result<DarwinInstallation, DarwinHandlerError> {
        self.process.look_path(OsStr::new("swiftc"))?;
        self.process.look_path(OsStr::new("swift"))?;
        let app = self.app_path()?;
        self.ensure_app(context, &app)?;
        let endpoint = self.endpoint_path()?;
        FileCallbackEndpointStore::new(&endpoint)
            .write(relay)
            .map_err(DarwinHandlerError::Endpoint)?;
        let previous = self.query_default(context).unwrap_or_default();
        let result = (|| {
            self.register(context, &app)?;
            if let Err(error) = self.set_default(context, DARWIN_BUNDLE_ID) {
                self.restore_previous(&previous);
                return Err(error);
            }
            Ok(DarwinInstallation {
                handler: self.clone(),
                endpoint: endpoint.clone(),
                previous,
                cleanup_on_drop: true,
            })
        })();
        if result.is_err() {
            let _ = private_file::remove_if_exists(&endpoint);
        }
        result
    }
    pub fn delegate_previous(
        &self,
        context: &Context,
        url: &str,
    ) -> Result<(), DarwinHandlerError> {
        let manifest = self
            .manifest_store()?
            .load()?
            .filter(|manifest| previous_available(&manifest.previous_handler))
            .ok_or(DarwinHandlerError::Message(
                "no previous Pixiv URL handler is available",
            ))?;
        self.process
            .run_context(
                context,
                OsStr::new("open"),
                &["-b".into(), manifest.previous_handler.into(), url.into()],
                ProcessStdio::Discard,
            )
            .map_err(|_| {
                DarwinHandlerError::Message("could not open previous Pixiv URL handler")
            })?;
        Ok(())
    }
}
impl PreviousHandler for DarwinHandler {
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
            let result = tokio::select! { result = &mut task => result, _ = cancellation.cancelled() => { context.cancel(); task.await }, };
            result
                .map_err(|_| io::Error::other("could not open previous Pixiv URL handler"))?
                .map_err(|error| Box::new(error) as crate::callback_handler::CallbackError)
        })
    }
}
pub struct DarwinInstallation {
    handler: DarwinHandler,
    endpoint: PathBuf,
    previous: String,
    cleanup_on_drop: bool,
}
impl DarwinInstallation {
    pub fn cleanup(&mut self) {
        self.cleanup_on_drop = false;
        let _ = private_file::remove_if_exists(&self.endpoint);
        self.handler.restore_previous(&self.previous);
    }
}
impl Drop for DarwinInstallation {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            self.cleanup();
        }
    }
}
struct CancelOnDrop(Context);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
fn previous_available(previous: &str) -> bool {
    !previous.is_empty() && previous != DARWIN_BUNDLE_ID
}
fn file_exists(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| !metadata.is_dir())
}
pub fn save_bundle_manifest(
    app: &Path,
    manifest: &HandlerManifest,
) -> Result<(), DarwinHandlerError> {
    let body = crate::handler_manifest::encode_unpromoted(manifest)?;
    private_file::write(&app.join("Contents/Resources/handler-manifest.json"), &body)?;
    Ok(())
}
fn create_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}
fn write_info(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    private_file::close(file)
}
struct PrivateSource {
    directory: PathBuf,
    path: PathBuf,
}
impl PrivateSource {
    fn new(temporary: &Path) -> io::Result<Self> {
        for _ in 0..128 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
            let suffix = random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let directory = temporary.join(format!("pixiv-cli-url-handler-{suffix}"));
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
            let source = Self {
                path: directory.join(format!("url-handler-{suffix}.swift")),
                directory,
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&source.directory, fs::Permissions::from_mode(0o700))?;
            }
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&source.path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(DARWIN_SWIFT_SOURCE.as_bytes())?;
            private_file::close(file)?;
            return Ok(source);
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create private compiler source",
        ))
    }
}
impl Drop for PrivateSource {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
