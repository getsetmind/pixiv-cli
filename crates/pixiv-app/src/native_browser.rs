use crate::{
    callback_handler::{BrowserOpener, CallbackResult},
    host_process::{HostPlatform, HostProcess, HostProcessError, ProcessStdio, SystemHostProcess},
};
use std::{
    ffi::{OsStr, OsString},
    io,
};

#[derive(Clone, Debug)]
pub struct NativeBrowserOpener<H = SystemHostProcess> {
    host: H,
}

impl Default for NativeBrowserOpener<SystemHostProcess> {
    fn default() -> Self {
        Self::new(SystemHostProcess)
    }
}

impl<H: HostProcess> NativeBrowserOpener<H> {
    pub fn new(host: H) -> Self {
        Self { host }
    }

    fn run(&self, provider: &str, url: &str) -> CallbackResult<()> {
        self.host.run(
            OsStr::new(provider),
            &[OsString::from(url)],
            ProcessStdio::Inherit,
        )?;
        Ok(())
    }
}

impl<H: HostProcess> BrowserOpener for NativeBrowserOpener<H> {
    fn open(&self, url: &str) -> CallbackResult<()> {
        match self.host.platform() {
            HostPlatform::Linux => {
                let providers = ["xdg-open", "x-www-browser", "www-browser"];
                for provider in providers {
                    if self.host.look_path(OsStr::new(provider)).is_ok() {
                        return self.run(provider, url);
                    }
                }
                Err(Box::new(HostProcessError::Lookup {
                    program: OsString::from(providers.join(",")),
                    source: io::Error::new(
                        io::ErrorKind::NotFound,
                        "executable file not found in $PATH",
                    ),
                }))
            }
            HostPlatform::Darwin => self.run("open", url),
            HostPlatform::Windows => {
                self.host.shell_open_url(url)?;
                Ok(())
            }
            HostPlatform::FreeBsd | HostPlatform::NetBsd | HostPlatform::OpenBsd => {
                let error = match self.run("xdg-open", url) {
                    Ok(()) => return Ok(()),
                    Err(error) => error,
                };
                if error
                    .downcast_ref::<HostProcessError>()
                    .is_some_and(HostProcessError::is_not_found)
                {
                    let source = if self.host.platform() == HostPlatform::NetBsd {
                        "pkgsrc(7)"
                    } else {
                        "ports(8)"
                    };
                    return Err(io::Error::other(format!(
                        "xdg-open: command not found - install xdg-utils from {source}"
                    ))
                    .into());
                }
                Err(error)
            }
            HostPlatform::Unsupported(platform) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("openBrowser: unsupported operating system: {platform}"),
            )
            .into()),
        }
    }
}
