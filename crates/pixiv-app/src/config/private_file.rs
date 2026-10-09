use super::{ConfigError, PrivateWriteOutcome};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(super) fn directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

pub(super) fn ensure_directory(directory: &Path) -> Result<(), ConfigError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        builder.mode(0o700);
        builder.create(directory).map_err(ConfigError::Io)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(ConfigError::Io)?;
    }
    #[cfg(not(unix))]
    builder.create(directory).map_err(ConfigError::Io)?;
    Ok(())
}

fn joined(errors: Vec<ConfigError>) -> Result<(), ConfigError> {
    let mut errors = errors;
    match errors.len() {
        0 => Ok(()),
        1 => Err(errors.remove(0)),
        _ => Err(ConfigError::Joined(errors)),
    }
}

pub(super) fn write_body(file: &mut fs::File, body: &[u8]) -> io::Result<()> {
    let written = file.write(body)?;
    if written != body.len() {
        Err(io::Error::new(io::ErrorKind::WriteZero, "short write"))
    } else {
        Ok(())
    }
}

pub(super) fn write(path: &Path, body: &[u8]) -> Result<(), ConfigError> {
    let mut outcome = PrivateWriteOutcome::NotCommitted;
    write_inner(path, body, &mut outcome)
        .map_err(|error| ConfigError::PrivateWrite(outcome, Box::new(error)))
}

fn write_inner(
    path: &Path,
    body: &[u8],
    outcome: &mut PrivateWriteOutcome,
) -> Result<(), ConfigError> {
    let directory = directory(path);
    let mut missing = Vec::<PathBuf>::new();
    let mut current = directory;
    loop {
        match fs::metadata(current) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => missing.push(current.into()),
            Err(error) => return Err(ConfigError::Io(error)),
        }
        let Some(parent) = current
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            break;
        };
        current = parent;
    }
    ensure_directory(directory)?;
    let (temporary, mut file) = temporary_file(directory)?;
    let mut errors = Vec::new();
    let written = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        write_body(&mut file, body)?;
        file.sync_all()
    })();
    if let Err(error) = written {
        errors.push(ConfigError::Io(error));
    }
    if let Err(error) = close(file) {
        errors.push(ConfigError::Io(error));
    }
    if errors.is_empty() {
        match replace(&temporary, path) {
            Ok(()) => *outcome = PrivateWriteOutcome::Committed,
            Err((replacement_outcome, error)) => {
                *outcome = replacement_outcome;
                errors.push(error);
            }
        }
    }
    if *outcome == PrivateWriteOutcome::Committed {
        if let Err(error) = sync_directories(directory, &missing) {
            errors.push(error);
        }
    } else if *outcome != PrivateWriteOutcome::Unknown
        && let Err(error) = fs::remove_file(&temporary)
        && error.kind() != io::ErrorKind::NotFound
    {
        errors.push(ConfigError::Io(error));
    }
    joined(errors)
}

fn temporary_file(directory: &Path) -> Result<(PathBuf, fs::File), ConfigError> {
    for _ in 0..128 {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|error| ConfigError::Io(io::Error::other(error.to_string())))?;
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = directory.join(format!(".pixiv-private-{suffix}"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ConfigError::Io(error)),
        }
    }
    Err(ConfigError::Io(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create private staging file",
    )))
}

#[cfg(unix)]
pub(super) fn close(file: fs::File) -> io::Result<()> {
    use std::os::fd::IntoRawFd;
    let descriptor = file.into_raw_fd();
    if unsafe { libc::close(descriptor) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
pub(super) fn close(file: fs::File) -> io::Result<()> {
    use std::os::windows::io::IntoRawHandle;
    let handle = file.into_raw_handle();
    if unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
pub(super) fn close(file: fs::File) -> io::Result<()> {
    drop(file);
    Ok(())
}

#[cfg(not(windows))]
fn replace(source: &Path, target: &Path) -> Result<(), (PrivateWriteOutcome, ConfigError)> {
    fs::rename(source, target)
        .map_err(|error| (PrivateWriteOutcome::NotCommitted, ConfigError::Io(error)))
}

#[cfg(windows)]
fn replace(source: &Path, target: &Path) -> Result<(), (PrivateWriteOutcome, ConfigError)> {
    super::private_replace_windows::replace(source, target)
}

#[cfg(unix)]
fn sync_directories(directory: &Path, missing: &[PathBuf]) -> Result<(), ConfigError> {
    let mut errors = Vec::new();
    for directory in std::iter::once(directory.to_path_buf()).chain(
        missing
            .iter()
            .map(|path| self::directory(path).to_path_buf()),
    ) {
        match fs::File::open(directory) {
            Ok(file) => {
                if let Err(error) = file.sync_all() {
                    errors.push(ConfigError::Io(error));
                }
                if let Err(error) = close(file) {
                    errors.push(ConfigError::Io(error));
                }
            }
            Err(error) => errors.push(ConfigError::Io(error)),
        }
    }
    joined(errors)
}

#[cfg(not(unix))]
fn sync_directories(_directory: &Path, _missing: &[PathBuf]) -> Result<(), ConfigError> {
    Ok(())
}
