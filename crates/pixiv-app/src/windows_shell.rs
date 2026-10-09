use crate::lifecycle::{Context, ContextError};
use std::{error::Error, fmt, io};

pub trait WindowsShell: Send + Sync {
    fn open_class(
        &self,
        context: &Context,
        class_name: &str,
        raw_url: &str,
    ) -> Result<(), WindowsShellError>;
}

#[derive(Debug)]
pub enum WindowsShellError {
    Context(ContextError),
    Native(io::Error),
    Message(&'static str),
}

impl fmt::Display for WindowsShellError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(error) => error.fmt(formatter),
            Self::Native(error) => error.fmt(formatter),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl Error for WindowsShellError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Context(error) => Some(error),
            Self::Native(error) => Some(error),
            Self::Message(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemWindowsShell;

impl WindowsShell for SystemWindowsShell {
    fn open_class(
        &self,
        context: &Context,
        class_name: &str,
        raw_url: &str,
    ) -> Result<(), WindowsShellError> {
        if let Some(reason) = context.error() {
            return Err(WindowsShellError::Context(reason));
        }
        let class = utf16(class_name)?;
        let file = utf16(raw_url)?;
        #[cfg(windows)]
        {
            use windows_sys::Win32::{
                Foundation::GetLastError,
                UI::{
                    Shell::{SEE_MASK_CLASSNAME, SHELLEXECUTEINFOW, ShellExecuteExW},
                    WindowsAndMessaging::SW_SHOWNORMAL,
                },
            };
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_CLASSNAME,
                lpFile: file.as_ptr(),
                lpClass: class.as_ptr(),
                nShow: SW_SHOWNORMAL,
                ..SHELLEXECUTEINFOW::default()
            };
            let result = unsafe { ShellExecuteExW(&mut info) };
            if result == 0 {
                let code = unsafe { GetLastError() };
                return Err(if code == 0 {
                    WindowsShellError::Message("ShellExecuteExW failed")
                } else {
                    WindowsShellError::Native(io::Error::from_raw_os_error(code as i32))
                });
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = (class, file);
            Err(WindowsShellError::Native(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows ShellExecuteExW is unavailable on this host",
            )))
        }
    }
}

fn utf16(value: &str) -> Result<Vec<u16>, WindowsShellError> {
    if value.contains('\0') {
        return Err(WindowsShellError::Native(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid argument",
        )));
    }
    Ok(value.encode_utf16().chain(Some(0)).collect())
}
