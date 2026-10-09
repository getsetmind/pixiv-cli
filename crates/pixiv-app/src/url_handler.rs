use crate::{
    callback_handler::{CallbackResult, PreviousHandler, clean_native_path},
    darwin_handler::{DarwinHandler, DarwinInstallation},
    handler_manifest::HandlerManifestStore,
    handoff_client::HandoffFuture,
    host_process::HostPlatform,
    lifecycle::Context,
    linux_handler::{LinuxHandler, LinuxInstallation},
    windows_handler::{WindowsHandler, WindowsInstallation},
};
use std::{ffi::OsString, io, path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

pub trait UrlHandlerInstallation: Send {
    fn cleanup(&mut self);
}
pub trait PlatformUrlHandler: PreviousHandler {
    fn ensure_persistent(&self, context: &Context) -> CallbackResult<()>;
    fn disable_persistent(&self, context: &Context) -> CallbackResult<()>;
    fn install(
        &self,
        context: &Context,
        relay: &str,
    ) -> CallbackResult<Box<dyn UrlHandlerInstallation>>;
}
pub trait UrlHandler: Send + Sync {
    fn automatic_supported(&self) -> bool;
    fn ensure_if_needed(&self, context: &Context) -> CallbackResult<()>;
    fn ensure_persistent(&self, context: &Context) -> CallbackResult<()>;
    fn disable_persistent(&self, context: &Context) -> CallbackResult<()>;
    fn install(
        &self,
        context: &Context,
        relay: &str,
    ) -> CallbackResult<Box<dyn UrlHandlerInstallation>>;
}
pub trait UrlHandlerEnvironment: Send + Sync {
    fn platform(&self) -> HostPlatform;
    fn program_name(&self) -> OsString;
    fn executable_path(&self) -> io::Result<PathBuf>;
}
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemUrlHandlerEnvironment;
impl UrlHandlerEnvironment for SystemUrlHandlerEnvironment {
    fn platform(&self) -> HostPlatform {
        HostPlatform::current()
    }
    fn program_name(&self) -> OsString {
        std::env::args_os().next().unwrap_or_default()
    }
    fn executable_path(&self) -> io::Result<PathBuf> {
        std::env::current_exe()
    }
}
#[derive(Clone)]
pub struct SystemUrlHandler {
    environment: Arc<dyn UrlHandlerEnvironment>,
    backend: Arc<dyn PlatformUrlHandler>,
    manifest: Option<HandlerManifestStore>,
}
impl SystemUrlHandler {
    pub fn system() -> Self {
        let backend = match HostPlatform::current() {
            HostPlatform::Linux => NativeHandler::Linux(LinuxHandler::system()),
            HostPlatform::Darwin => NativeHandler::Darwin(DarwinHandler::system()),
            HostPlatform::Windows => NativeHandler::Windows(WindowsHandler::system()),
            _ => NativeHandler::Unsupported,
        };
        Self {
            environment: Arc::new(SystemUrlHandlerEnvironment),
            backend: Arc::new(backend),
            manifest: None,
        }
    }
    pub fn with_environment(
        environment: Arc<dyn UrlHandlerEnvironment>,
        backend: Arc<dyn PlatformUrlHandler>,
        manifest: HandlerManifestStore,
    ) -> Self {
        Self {
            environment,
            backend,
            manifest: Some(manifest),
        }
    }
}
impl UrlHandler for SystemUrlHandler {
    fn automatic_supported(&self) -> bool {
        let program = self.environment.program_name();
        if std::path::Path::new(&program)
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().ends_with(b".test"))
        {
            return false;
        }
        matches!(
            self.environment.platform(),
            HostPlatform::Darwin | HostPlatform::Windows
        )
    }
    fn ensure_if_needed(&self, context: &Context) -> CallbackResult<()> {
        if !self.automatic_supported() {
            return Ok(());
        }
        let executable = self.environment.executable_path()?;
        let store = match &self.manifest {
            Some(store) => store.clone(),
            None => HandlerManifestStore::default_path()?,
        };
        if let Some(manifest) = store.load()?
            && clean_native_path(std::path::Path::new(&manifest.executable_path))
                == clean_native_path(&executable)
        {
            return Ok(());
        }
        self.backend.ensure_persistent(context)
    }
    fn ensure_persistent(&self, context: &Context) -> CallbackResult<()> {
        self.backend.ensure_persistent(context)
    }
    fn disable_persistent(&self, context: &Context) -> CallbackResult<()> {
        self.backend.disable_persistent(context)
    }
    fn install(
        &self,
        context: &Context,
        relay: &str,
    ) -> CallbackResult<Box<dyn UrlHandlerInstallation>> {
        self.backend.install(context, relay)
    }
}
impl PreviousHandler for SystemUrlHandler {
    fn delegate<'a>(
        &'a self,
        raw_url: &'a str,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<()>> {
        self.backend.delegate(raw_url, cancellation)
    }
}
enum NativeHandler {
    Linux(LinuxHandler),
    Darwin(DarwinHandler),
    Windows(WindowsHandler),
    Unsupported,
}
impl PlatformUrlHandler for NativeHandler {
    fn ensure_persistent(&self, context: &Context) -> CallbackResult<()> {
        match self {
            Self::Linux(handler) => handler
                .ensure_persistent(context)
                .map_err(|error| Box::new(error) as _),
            Self::Darwin(handler) => handler
                .ensure_persistent(context)
                .map_err(|error| Box::new(error) as _),
            Self::Windows(handler) => handler
                .ensure_persistent(context)
                .map_err(|error| Box::new(error) as _),
            Self::Unsupported => Err(io::Error::other(
                "persistent pixiv:// callback handler is not supported on this platform",
            )
            .into()),
        }
    }
    fn disable_persistent(&self, context: &Context) -> CallbackResult<()> {
        match self {
            Self::Linux(handler) => handler
                .disable_persistent(context)
                .map_err(|error| Box::new(error) as _),
            Self::Darwin(handler) => handler
                .disable_persistent(context)
                .map_err(|error| Box::new(error) as _),
            Self::Windows(handler) => handler
                .disable_persistent(context)
                .map_err(|error| Box::new(error) as _),
            Self::Unsupported => Err(io::Error::other(
                "persistent pixiv:// callback handler is not supported on this platform",
            )
            .into()),
        }
    }
    fn install(
        &self,
        context: &Context,
        relay: &str,
    ) -> CallbackResult<Box<dyn UrlHandlerInstallation>> {
        let installed = match self {
            Self::Linux(handler) => {
                NativeInstallation::Linux(Some(handler.install(context, relay)?))
            }
            Self::Darwin(handler) => {
                NativeInstallation::Darwin(Some(handler.install(context, relay)?))
            }
            Self::Windows(handler) => {
                NativeInstallation::Windows(Some(handler.install(context, relay)?))
            }
            Self::Unsupported => {
                return Err(io::Error::other(
                    "pixiv:// callback handler is only supported on macOS",
                )
                .into());
            }
        };
        Ok(Box::new(installed))
    }
}
impl PreviousHandler for NativeHandler {
    fn delegate<'a>(
        &'a self,
        raw_url: &'a str,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<()>> {
        match self {
            Self::Linux(handler) => handler.delegate(raw_url, cancellation),
            Self::Darwin(handler) => handler.delegate(raw_url, cancellation),
            Self::Windows(handler) => handler.delegate(raw_url, cancellation),
            Self::Unsupported => Box::pin(async {
                Err(io::Error::other(
                    "previous Pixiv URL handler cannot be opened on this platform",
                )
                .into())
            }),
        }
    }
}
enum NativeInstallation {
    Linux(Option<LinuxInstallation>),
    Darwin(Option<DarwinInstallation>),
    Windows(Option<WindowsInstallation>),
}
impl UrlHandlerInstallation for NativeInstallation {
    fn cleanup(&mut self) {
        match self {
            Self::Linux(installed) => {
                if let Some(installed) = installed.take() {
                    let _ = installed.cleanup();
                }
            }
            Self::Darwin(installed) => {
                if let Some(mut installed) = installed.take() {
                    installed.cleanup();
                }
            }
            Self::Windows(installed) => {
                if let Some(mut installed) = installed.take() {
                    installed.cleanup();
                }
            }
        }
    }
}
