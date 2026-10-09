use crate::config::PrivateWriteOutcome;
pub use crate::config::PrivateWriteOutcome as WriteCommitOutcome;
use std::{
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
pub struct SecretFileWriteError {
    outcome: PrivateWriteOutcome,
    causes: Vec<io::Error>,
}
impl fmt::Debug for SecretFileWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl SecretFileWriteError {
    pub fn commit_outcome(&self) -> PrivateWriteOutcome {
        self.outcome
    }
}
impl fmt::Display for SecretFileWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cause = if self
            .causes
            .iter()
            .any(|e| e.kind() == io::ErrorKind::AlreadyExists)
        {
            "destination already exists"
        } else if self
            .causes
            .iter()
            .any(|e| e.kind() == io::ErrorKind::PermissionDenied)
        {
            "permission denied"
        } else if self
            .causes
            .iter()
            .any(|e| e.kind() == io::ErrorKind::NotFound)
        {
            "destination directory is unavailable"
        } else {
            "filesystem operation failed"
        };
        let outcome = match self.outcome {
            PrivateWriteOutcome::NotCommitted => "not_committed",
            PrivateWriteOutcome::Committed => "committed",
            PrivateWriteOutcome::Unknown => "unknown",
        };
        write!(f, "authentication export write failed: {cause} ({outcome})")
    }
}
impl std::error::Error for SecretFileWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.causes.first().map(|e| e as _)
    }
}
fn directory(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}
pub fn write(path: &Path, body: &[u8], force: bool) -> Result<(), SecretFileWriteError> {
    let mut outcome = PrivateWriteOutcome::NotCommitted;
    let mut causes = Vec::new();
    let result = write_inner(path, body, force, &mut outcome, &mut causes);
    if let Err(e) = result {
        causes.insert(0, e)
    }
    if causes.is_empty() {
        Ok(())
    } else {
        Err(SecretFileWriteError { outcome, causes })
    }
}
fn write_inner(
    path: &Path,
    body: &[u8],
    force: bool,
    outcome: &mut PrivateWriteOutcome,
    causes: &mut Vec<io::Error>,
) -> io::Result<()> {
    let parent = directory(path);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists
            && fs::metadata(parent).is_ok_and(|metadata| !metadata.is_dir())
        {
            io::Error::new(io::ErrorKind::NotADirectory, "not a directory")
        } else {
            error
        }
    })?;
    if !force {
        let file = open_exclusive(path)?;
        if let Err(e) = write_close(file, body, causes) {
            cleanup(path, causes);
            return Err(e);
        }
        *outcome = PrivateWriteOutcome::Committed;
        return sync_parent(parent);
    }
    let (temporary, file) = temporary(parent)?;
    if let Err(e) = write_close(file, body, causes) {
        cleanup(&temporary, causes);
        return Err(e);
    }
    #[cfg(not(windows))]
    let replaced = fs::rename(&temporary, path).map_err(|e| (PrivateWriteOutcome::NotCommitted, e));
    #[cfg(windows)]
    let replaced = crate::config::private_replace_windows::replace(&temporary, path)
        .map_err(|(o, e)| (o, replacement_error(e)));
    match replaced {
        Ok(()) => *outcome = PrivateWriteOutcome::Committed,
        Err((o, e)) => {
            *outcome = o;
            if o == PrivateWriteOutcome::Committed {
                causes.push(e);
            } else {
                if o != PrivateWriteOutcome::Unknown {
                    cleanup(&temporary, causes);
                }
                return Err(e);
            }
        }
    }
    if let Err(e) = protect_path(path) {
        causes.push(e)
    }
    sync_parent(parent)
}
fn cleanup(path: &Path, causes: &mut Vec<io::Error>) {
    if let Err(e) = fs::remove_file(path)
        && e.kind() != io::ErrorKind::NotFound
    {
        causes.push(e)
    }
}
fn temporary(parent: &Path) -> io::Result<(PathBuf, fs::File)> {
    loop {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
        let suffix = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let path = parent.join(format!(".pixiv-secret-{suffix}"));
        match open_exclusive(&path) {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}
fn write_close(mut file: fs::File, body: &[u8], causes: &mut Vec<io::Error>) -> io::Result<()> {
    let result = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let written = file.write(body)?;
        if written != body.len() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "short write"));
        }
        file.sync_all()
    })();
    let closed = crate::config::private_file::close(file);
    if result.is_err() {
        if let Err(e) = closed {
            causes.push(e)
        }
        result
    } else {
        closed
    }
}
#[cfg(not(windows))]
fn open_exclusive(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
#[cfg(not(windows))]
fn protect_path(_: &Path) -> io::Result<()> {
    Ok(())
}
#[cfg(unix)]
fn sync_parent(path: &Path) -> io::Result<()> {
    let file = fs::File::open(path)?;
    let synced = file.sync_all();
    let closed = crate::config::private_file::close(file);
    synced.and(closed)
}
#[cfg(not(unix))]
fn sync_parent(_: &Path) -> io::Result<()> {
    Ok(())
}
#[cfg(windows)]
#[path = "secret_file_windows.rs"]
mod windows;
#[cfg(windows)]
use windows::{open_exclusive, protect_path};

#[cfg(windows)]
fn replacement_error(error: crate::config::ConfigError) -> io::Error {
    fn category(error: &crate::config::ConfigError) -> io::ErrorKind {
        use crate::config::ConfigError;
        match error {
            ConfigError::Io(e) => e.kind(),
            ConfigError::Context(e, _)
            | ConfigError::PrivateWrite(_, e)
            | ConfigError::Operation(_, e) => category(e),
            ConfigError::Joined(errors) => {
                let kinds = errors.iter().map(category).collect::<Vec<_>>();
                [
                    io::ErrorKind::AlreadyExists,
                    io::ErrorKind::PermissionDenied,
                    io::ErrorKind::NotFound,
                ]
                .into_iter()
                .find(|kind| kinds.contains(kind))
                .unwrap_or(io::ErrorKind::Other)
            }
            _ => io::ErrorKind::Other,
        }
    }
    io::Error::new(category(&error), error)
}
