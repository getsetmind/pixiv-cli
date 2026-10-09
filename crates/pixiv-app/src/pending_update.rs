use crate::auth_bundle::go_quote;
use std::{
    collections::VecDeque,
    error::Error,
    ffi::OsString,
    fmt, fs, io,
    path::{Component, Path, PathBuf},
};

pub trait PendingUpdateEnvironment {
    fn is_windows(&self) -> bool;
    fn executable_path(&self) -> io::Result<PathBuf>;
    fn absolute_path(&self, path: &Path) -> io::Result<PathBuf>;
    fn is_symlink(&self, path: &Path) -> io::Result<bool>;
    fn evaluate_symlinks(&self, path: &Path) -> io::Result<PathBuf>;
    fn remove(&self, path: &Path) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemPendingUpdateEnvironment;

impl PendingUpdateEnvironment for SystemPendingUpdateEnvironment {
    fn is_windows(&self) -> bool {
        cfg!(windows)
    }

    fn executable_path(&self) -> io::Result<PathBuf> {
        std::env::current_exe()
    }

    fn absolute_path(&self, path: &Path) -> io::Result<PathBuf> {
        let path = if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            path
        };
        std::path::absolute(path).map(|absolute| clean_path(&absolute))
    }

    fn is_symlink(&self, path: &Path) -> io::Result<bool> {
        fs::symlink_metadata(path).map(|metadata| metadata.file_type().is_symlink())
    }

    fn evaluate_symlinks(&self, path: &Path) -> io::Result<PathBuf> {
        evaluate_system_symlinks(path)
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        remove_file_or_empty_directory(path)
    }
}

#[derive(Debug)]
pub enum PendingUpdateError {
    LocateExecutable { source: io::Error },
    ResolveExecutable { path: PathBuf, source: io::Error },
    ResolveSymlink { path: PathBuf, source: io::Error },
    ResolveSymlinkTarget { path: PathBuf, source: io::Error },
    RemoveOldExecutable { path: PathBuf, source: io::Error },
}

impl fmt::Display for PendingUpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (context, path, source) = match self {
            Self::LocateExecutable { source } => {
                return write!(
                    formatter,
                    "locate executable for pending update cleanup: {source}"
                );
            }
            Self::ResolveExecutable { path, source } => {
                ("resolve current executable", path, source)
            }
            Self::ResolveSymlink { path, source } => ("resolve executable symlink", path, source),
            Self::ResolveSymlinkTarget { path, source } => {
                ("resolve executable symlink target", path, source)
            }
            Self::RemoveOldExecutable { path, source } => {
                ("remove pending old executable", path, source)
            }
        };
        write!(
            formatter,
            "{context} {}: {source}",
            go_quote(&path.to_string_lossy())
        )
    }
}

impl Error for PendingUpdateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::LocateExecutable { source }
            | Self::ResolveExecutable { source, .. }
            | Self::ResolveSymlink { source, .. }
            | Self::ResolveSymlinkTarget { source, .. }
            | Self::RemoveOldExecutable { source, .. } => Some(source),
        }
    }
}

pub struct PendingUpdateCleanup<E> {
    environment: E,
}

impl<E: PendingUpdateEnvironment> PendingUpdateCleanup<E> {
    pub fn new(environment: E) -> Self {
        Self { environment }
    }

    pub fn cleanup(&self) -> Result<(), PendingUpdateError> {
        if !self.environment.is_windows() {
            return Ok(());
        }
        let executable = self
            .environment
            .executable_path()
            .map_err(|source| PendingUpdateError::LocateExecutable { source })?;
        let mut target = self
            .environment
            .absolute_path(&executable)
            .map_err(|source| PendingUpdateError::ResolveExecutable {
                path: executable,
                source,
            })?;
        if self.environment.is_symlink(&target).unwrap_or(false) {
            let resolved = self
                .environment
                .evaluate_symlinks(&target)
                .map_err(|source| PendingUpdateError::ResolveSymlink {
                    path: target.clone(),
                    source,
                })?;
            target = self
                .environment
                .absolute_path(&resolved)
                .map_err(|source| PendingUpdateError::ResolveSymlinkTarget {
                    path: resolved,
                    source,
                })?;
        }
        let mut old = target.into_os_string();
        old.push(".old");
        let old = PathBuf::from(old);
        match self.environment.remove(&old) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(PendingUpdateError::RemoveOldExecutable { path: old, source }),
        }
    }
}

pub fn cleanup_pending_windows_update() -> Result<(), PendingUpdateError> {
    PendingUpdateCleanup::new(SystemPendingUpdateEnvironment).cleanup()
}

fn clean_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if result.file_name().is_some_and(|name| name != "..") {
                    result.pop();
                } else if !result.has_root() {
                    result.push("..");
                }
            }
            component => result.push(component.as_os_str()),
        }
    }
    result
}

#[derive(Clone)]
enum PathPart {
    Prefix(OsString),
    Root,
    Parent,
    Normal(OsString),
    Directory,
}

fn path_parts(path: &Path) -> VecDeque<PathPart> {
    let mut parts: VecDeque<_> = path
        .components()
        .filter_map(|component| match component {
            Component::Prefix(prefix) => Some(PathPart::Prefix(prefix.as_os_str().to_owned())),
            Component::RootDir => Some(PathPart::Root),
            Component::CurDir => None,
            Component::ParentDir => Some(PathPart::Parent),
            Component::Normal(name) => Some(PathPart::Normal(name.to_owned())),
        })
        .collect();
    let trailing_separator = path
        .as_os_str()
        .as_encoded_bytes()
        .last()
        .is_some_and(|byte| *byte == b'/' || cfg!(windows) && *byte == b'\\');
    let bytes = path.as_os_str().as_encoded_bytes();
    let trailing_dot = bytes.ends_with(b"/.") || cfg!(windows) && bytes.ends_with(b"\\.");
    if trailing_separator || trailing_dot {
        parts.push_back(PathPart::Directory);
    }
    parts
}

fn evaluate_system_symlinks(path: &Path) -> io::Result<PathBuf> {
    let mut pending = path_parts(path);
    let mut result = PathBuf::new();
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        match part {
            PathPart::Prefix(prefix) => result = PathBuf::from(prefix),
            PathPart::Root => result.push(std::path::MAIN_SEPARATOR_STR),
            PathPart::Parent => {
                if result.file_name().is_some_and(|name| name != "..") {
                    result.pop();
                } else if !result.has_root() {
                    result.push("..");
                }
            }
            PathPart::Directory => {}
            PathPart::Normal(name) => {
                result.push(name);
                let metadata = fs::symlink_metadata(&result)?;
                if !metadata.file_type().is_symlink() {
                    if !metadata.is_dir() && !pending.is_empty() {
                        return Err(io::Error::new(
                            io::ErrorKind::NotADirectory,
                            "not a directory",
                        ));
                    }
                    continue;
                }
                links += 1;
                if links > 255 {
                    return Err(io::Error::other("EvalSymlinks: too many links"));
                }
                let link = fs::read_link(&result)?;
                result.pop();
                let mut linked_parts = path_parts(&link);
                if matches!(
                    linked_parts.front(),
                    Some(PathPart::Prefix(_) | PathPart::Root)
                ) {
                    result.clear();
                }
                linked_parts.append(&mut pending);
                pending = linked_parts;
            }
        }
    }
    let result = clean_path(&result);
    #[cfg(windows)]
    {
        normalized_windows_path(&result)
    }
    #[cfg(not(windows))]
    {
        Ok(result)
    }
}

#[cfg(windows)]
fn normalized_windows_path(path: &Path) -> io::Result<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let canonical = fs::canonicalize(path)?;
    let wide: Vec<_> = canonical.as_os_str().encode_wide().collect();
    let unc: Vec<_> = r"\\?\UNC\".encode_utf16().collect();
    let verbatim: Vec<_> = r"\\?\".encode_utf16().collect();
    if wide.starts_with(&unc) {
        let mut normalized: Vec<_> = r"\\".encode_utf16().collect();
        normalized.extend_from_slice(&wide[unc.len()..]);
        Ok(PathBuf::from(OsString::from_wide(&normalized)))
    } else if wide.starts_with(&verbatim) && wide.get(5) == Some(&(b':' as u16)) {
        Ok(PathBuf::from(OsString::from_wide(&wide[verbatim.len()..])))
    } else {
        Ok(canonical)
    }
}

#[cfg(not(windows))]
fn remove_file_or_empty_directory(path: &Path) -> io::Result<()> {
    let file_error = match fs::remove_file(path) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotADirectory => Err(file_error),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn remove_file_or_empty_directory(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        DeleteFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY, GetFileAttributesW,
        INVALID_FILE_ATTRIBUTES, RemoveDirectoryW, SetFileAttributesW,
    };
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::from_raw_os_error(87));
    }
    let verbatim: Vec<_> = r"\\?\".encode_utf16().collect();
    if wide.len() >= 248 && path.is_absolute() && !wide.starts_with(&verbatim) {
        let unc: Vec<_> = r"\\".encode_utf16().collect();
        let (prefix, offset) = if wide.starts_with(&unc) {
            (r"\\?\UNC\", 2)
        } else {
            (r"\\?\", 0)
        };
        wide = prefix
            .encode_utf16()
            .chain(wide[offset..].iter().copied())
            .collect();
    }
    wide.push(0);
    let pointer = wide.as_ptr();
    unsafe {
        if DeleteFileW(pointer) != 0 {
            return Ok(());
        }
        let mut file_error = io::Error::last_os_error();
        if RemoveDirectoryW(pointer) != 0 {
            return Ok(());
        }
        let directory_error = io::Error::last_os_error();
        if directory_error.raw_os_error() != file_error.raw_os_error() {
            let attributes = GetFileAttributesW(pointer);
            if attributes == INVALID_FILE_ATTRIBUTES {
                file_error = io::Error::last_os_error();
            } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                file_error = directory_error;
            } else if attributes & FILE_ATTRIBUTE_READONLY != 0
                && SetFileAttributesW(pointer, attributes & !FILE_ATTRIBUTE_READONLY) != 0
            {
                if DeleteFileW(pointer) != 0 {
                    return Ok(());
                }
                file_error = io::Error::last_os_error();
            }
        }
        Err(file_error)
    }
}
