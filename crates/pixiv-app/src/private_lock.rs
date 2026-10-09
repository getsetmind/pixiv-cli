use crate::config::{ConfigError, private_file};
use crate::handoff_state::HandoffStateError;
use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};
use tokio_util::sync::CancellationToken;

pub fn with_private_lock<T>(
    path: &Path,
    cancellation: Option<&CancellationToken>,
    action: impl FnOnce() -> Result<T, HandoffStateError>,
) -> Result<T, HandoffStateError> {
    check_cancelled(cancellation)?;
    private_file::ensure_directory(private_file::directory(path))?;
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(".lock");
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(Path::new(&sidecar)).map_err(ConfigError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(ConfigError::Io)?;
    }
    let guard = LockGuard::acquire(file).map_err(ConfigError::Io)?;
    check_cancelled(cancellation)?;
    let result = action();
    drop(guard);
    result
}

fn check_cancelled(cancellation: Option<&CancellationToken>) -> Result<(), HandoffStateError> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        Err(HandoffStateError::Cancelled)
    } else {
        Ok(())
    }
}

struct LockGuard {
    file: File,
}
impl LockGuard {
    fn acquire(file: File) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::{
                Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx},
                System::IO::OVERLAPPED,
            };
            let mut overlap: OVERLAPPED = unsafe { std::mem::zeroed() };
            if unsafe {
                LockFileEx(
                    file.as_raw_handle(),
                    LOCKFILE_EXCLUSIVE_LOCK,
                    0,
                    1,
                    0,
                    &mut overlap,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(not(any(unix, windows)))]
        return Err(io::Error::other(
            "local state locking is unsupported on this platform",
        ));
        #[cfg(any(unix, windows))]
        Ok(Self { file })
    }
}
impl Drop for LockGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::{Storage::FileSystem::UnlockFileEx, System::IO::OVERLAPPED};
            let mut overlap: OVERLAPPED = unsafe { std::mem::zeroed() };
            unsafe {
                UnlockFileEx(self.file.as_raw_handle(), 0, 1, 0, &mut overlap);
            }
        }
    }
}
