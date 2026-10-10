use crate::{
    browser_cookies::SecretBytes,
    host_context::ContextHostProcess,
    host_process::{HostProcessError, ProcessStdio},
    lifecycle::{Context, ContextError},
};
use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt,
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserSecretError {
    NotAvailableOnBuild,
    ItemNotFound,
    KeychainCommand,
    EmptyPassword,
    InvalidItem,
    InvalidBlob,
    Dpapi,
    SecretService,
    Context(ContextError),
}

impl fmt::Display for BrowserSecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::NotAvailableOnBuild => "browsercookies/secret: not available on this build",
            Self::ItemNotFound => "browsercookies/secret: keychain item not found",
            Self::KeychainCommand => "browsercookies/secret: keychain command failed",
            Self::EmptyPassword => "browsercookies/secret: keychain item has an empty password",
            Self::InvalidItem => "browsercookies/secret: invalid keychain item name",
            Self::InvalidBlob => "browsercookies/secret: invalid encrypted blob",
            Self::Dpapi => "browsercookies/secret: DPAPI unprotect failed",
            Self::SecretService => "browsercookies/secret: Secret Service lookup failed",
            Self::Context(error) => return error.fmt(formatter),
        };
        formatter.write_str(message)
    }
}

impl Error for BrowserSecretError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Context(error) => Some(error),
            _ => None,
        }
    }
}

pub struct SecretService {
    process: Arc<dyn ContextHostProcess>,
}

impl SecretService {
    pub fn new(process: Arc<dyn ContextHostProcess>) -> Self {
        Self { process }
    }

    pub fn get_password(
        &self,
        context: &Context,
        application: &str,
    ) -> Result<SecretBytes, BrowserSecretError> {
        if let Some(error) = context.error() {
            return Err(BrowserSecretError::Context(error));
        }
        if !matches!(application.trim(), "chrome" | "microsoft-edge") {
            return Err(BrowserSecretError::InvalidItem);
        }
        let program = OsStr::new("secret-tool");
        if self.process.look_path(program).is_err() {
            return Err(BrowserSecretError::NotAvailableOnBuild);
        }
        let args: Vec<OsString> = [
            "lookup",
            "xdg:schema",
            "chrome_libsecret",
            "application",
            application,
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let output = self
            .process
            .run_context(context, program, &args, ProcessStdio::CaptureStdout)
            .map_err(|_| {
                context
                    .error()
                    .map(BrowserSecretError::Context)
                    .unwrap_or(BrowserSecretError::SecretService)
            })?;
        password(output.stdout)
    }
}

pub struct Keychain {
    process: Arc<dyn ContextHostProcess>,
}

impl Keychain {
    pub fn new(process: Arc<dyn ContextHostProcess>) -> Self {
        Self { process }
    }

    pub fn get_password(
        &self,
        context: &Context,
        service: &str,
        account: &str,
    ) -> Result<SecretBytes, BrowserSecretError> {
        if service.trim().is_empty() || account.trim().is_empty() {
            return Err(BrowserSecretError::InvalidItem);
        }
        let args: Vec<OsString> = ["find-generic-password", "-w", "-s", service, "-a", account]
            .into_iter()
            .map(OsString::from)
            .collect();
        let output = self
            .process
            .run_context(
                context,
                OsStr::new("security"),
                &args,
                ProcessStdio::CaptureStdout,
            )
            .map_err(|error| {
                if let Some(reason) = context.error() {
                    return BrowserSecretError::Context(reason);
                }
                if exit_stderr(&error).is_some_and(|stderr| {
                    stderr
                        .windows(b"could not be found".len())
                        .any(|window| window == b"could not be found")
                }) {
                    BrowserSecretError::ItemNotFound
                } else {
                    BrowserSecretError::KeychainCommand
                }
            })?;
        password(output.stdout)
    }
}

fn exit_stderr(error: &HostProcessError) -> Option<&[u8]> {
    match error {
        HostProcessError::Exit { output, .. } => Some(&output.stderr),
        HostProcessError::Captured { source, .. } => exit_stderr(source),
        _ => None,
    }
}

fn password(mut bytes: Vec<u8>) -> Result<SecretBytes, BrowserSecretError> {
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
    {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(BrowserSecretError::EmptyPassword);
    }
    Ok(SecretBytes::new(bytes))
}
