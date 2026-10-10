use crate::{Error, Reason, Result, error::Cause, resource::ResourceResponse};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaveProgress {
    pub total: i64,
    pub done: i64,
}
#[derive(Clone, Default)]
pub struct SaveOptions {
    pub path: String,
    pub progress: Option<Arc<dyn Fn(SaveProgress) + Send + Sync>>,
}
impl std::fmt::Debug for SaveOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SaveOptions")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedResource {
    pub path: String,
    pub size: i64,
    pub content_type: String,
}

pub(crate) struct Destination {
    file: Option<File>,
    temporary: PathBuf,
}
impl Drop for Destination {
    fn drop(&mut self) {
        drop(self.file.take());
        if fs::remove_file(&self.temporary).is_err() {
            let _ = fs::remove_dir(&self.temporary);
        }
    }
}
impl Destination {
    pub(crate) fn create(path: &Path) -> io::Result<Self> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut directories = fs::DirBuilder::new();
        directories.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directories.mode(0o700);
        }
        directories.create(parent)?;
        for _ in 0..128 {
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes)
                .map_err(|_| io::Error::other("cannot allocate atomic destination"))?;
            let temporary = parent.join(format!(".atomic-write-{:x}", u128::from_ne_bytes(bytes)));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temporary) {
                Ok(file) => {
                    return Ok(Self {
                        file: Some(file),
                        temporary,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot allocate atomic destination",
        ))
    }
    pub(crate) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.as_mut().unwrap().write_all(bytes)
    }
    pub(crate) fn publish(&mut self, path: &Path) -> io::Result<()> {
        self.file.as_ref().unwrap().sync_all()?;
        drop(self.file.take());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.temporary, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(&self.temporary, path)
    }
}
pub(crate) fn validate_path(options: &SaveOptions, operation: &'static str) -> Result<()> {
    if options.path.trim().is_empty() {
        return Err(Error::new(Reason::InvalidArgument, operation)
            .with_detail("destination path is required"));
    }
    Ok(())
}
pub(crate) async fn write<R: AsyncRead + Unpin + Send>(
    mut response: ResourceResponse<R>,
    options: SaveOptions,
    operation: &'static str,
) -> Result<SavedResource> {
    if !(200..300).contains(&response.status_code) {
        return Err(Error::new(Reason::UpstreamError, operation)
            .with_detail("resource returned a non-success status"));
    }
    let local = |error: io::Error| {
        let mut classified =
            Error::new(Reason::LocalStateError, operation).with_detail("cannot write resource");
        if let Some(cause) = error
            .get_ref()
            .and_then(|cause| cause.downcast_ref::<Cause>())
            .filter(|cause| matches!(cause, Cause::Canceled | Cause::DeadlineExceeded))
        {
            classified = classified.with_cause(cause.clone());
        }
        classified
    };
    let mut destination = Destination::create(Path::new(&options.path)).map_err(local)?;
    let total = response.content_length();
    let content_type = response.content_type().to_owned();
    let mut done = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = response.body.read(&mut buffer).await.map_err(local)?;
        if count == 0 {
            break;
        }
        done += count as i64;
        if let Some(progress) = &options.progress {
            progress(SaveProgress { total, done });
        }
        destination.write(&buffer[..count]).map_err(local)?;
    }
    destination
        .publish(Path::new(&options.path))
        .map_err(local)?;
    Ok(SavedResource {
        path: options.path,
        size: done,
        content_type,
    })
}
