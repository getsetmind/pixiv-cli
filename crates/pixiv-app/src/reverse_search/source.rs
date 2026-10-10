use super::{CallerContext, Error, ErrorCode, ReverseFuture, SourceKind, SourceLoader};
use pixiv_sdk::fanbox::transport::{Headers, RawBody, RawRequest, RawResponse, RawTransport};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
use url::Url;

pub enum RedirectDecision {
    Follow,
    Stop,
}
pub trait RedirectHook: Send + Sync {
    fn check(&self, next_url: &str, via: &[String])
    -> std::result::Result<RedirectDecision, Error>;
}
#[derive(Default)]
pub struct SourceLoaderOptions {
    pub temp_dir: PathBuf,
    pub http_transport: Option<Arc<dyn RawTransport>>,
    pub redirect_hook: Option<Arc<dyn RedirectHook>>,
}
pub struct Loader {
    options: SourceLoaderOptions,
    default_transport: OnceLock<std::result::Result<Arc<dyn RawTransport>, Error>>,
}
impl Loader {
    pub fn new(options: SourceLoaderOptions) -> Self {
        Self {
            options,
            default_transport: OnceLock::new(),
        }
    }
    async fn load_inner(
        &self,
        context: CallerContext,
        source: &str,
    ) -> std::result::Result<Arc<Snapshot>, Error> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        let lower = source.to_ascii_lowercase();
        if lower.starts_with("http:") || lower.starts_with("https:") {
            return self.load_url(context, source).await;
        }
        let info = fs::metadata(source).map_err(|cause| {
            if has_url_scheme(source) {
                Error::new(
                    ErrorCode::InvalidSource,
                    "image source must use HTTP or HTTPS",
                    None,
                )
            } else {
                Error::new(
                    ErrorCode::SourceReadFailed,
                    "could not read image source",
                    Some(Error::external(cause)),
                )
            }
        })?;
        if !info.is_file() {
            return Err(not_regular());
        }
        let mut input = File::open(source).map_err(|cause| {
            Error::new(
                ErrorCode::SourceReadFailed,
                "could not read image source",
                Some(Error::external(cause)),
            )
        })?;
        let info = input.metadata().map_err(|cause| {
            Error::new(
                ErrorCode::SourceReadFailed,
                "could not inspect image source",
                Some(Error::external(cause)),
            )
        })?;
        if !info.is_file() {
            return Err(not_regular());
        }
        let mut temporary = self.temporary()?;
        let mut buffer = [0; 32 * 1024];
        loop {
            if let Some(error) = context.error() {
                return Err(error.into());
            }
            let count = input.read(&mut buffer).map_err(|cause| {
                context.error().map(Error::from).unwrap_or_else(|| {
                    Error::new(
                        ErrorCode::SourceReadFailed,
                        "could not read image source",
                        Some(Error::external(cause)),
                    )
                })
            })?;
            if count == 0 {
                break;
            }
            temporary
                .write(&buffer[..count])
                .map_err(|error| context.error().map(Error::from).unwrap_or(error))?;
            tokio::task::yield_now().await;
        }
        temporary.finish(SourceKind::File)
    }
    fn temporary(&self) -> std::result::Result<TemporarySnapshot, Error> {
        let directory = if self.options.temp_dir.as_os_str().is_empty() {
            std::env::temp_dir()
        } else {
            self.options.temp_dir.clone()
        };
        let mut random = [0_u8; 16];
        for _ in 0..100 {
            getrandom::fill(&mut random).map_err(|cause| {
                Error::new(
                    ErrorCode::SnapshotFailed,
                    "could not create image snapshot",
                    Some(Error::external(std::io::Error::other(cause.to_string()))),
                )
            })?;
            let name = random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let path = directory.join(format!("pixiv-reverse-search-{name}.tmp"));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    let temporary = TemporarySnapshot {
                        path,
                        file: Some(file),
                        digest: Sha256::new(),
                        size: 0,
                        retained: false,
                    };
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        temporary
                            .file
                            .as_ref()
                            .expect("created snapshot")
                            .set_permissions(fs::Permissions::from_mode(0o600))
                            .map_err(|cause| {
                                Error::new(
                                    ErrorCode::SnapshotFailed,
                                    "could not secure image snapshot",
                                    Some(Error::external(cause)),
                                )
                            })?;
                    }
                    return Ok(temporary);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(cause) => {
                    return Err(Error::new(
                        ErrorCode::SnapshotFailed,
                        "could not create image snapshot",
                        Some(Error::external(cause)),
                    ));
                }
            }
        }
        Err(Error::new(
            ErrorCode::SnapshotFailed,
            "could not create image snapshot",
            None,
        ))
    }
    async fn load_url(
        &self,
        context: CallerContext,
        source: &str,
    ) -> std::result::Result<Arc<Snapshot>, Error> {
        let mut url = validate_source_url(source)?;
        let transport = if let Some(transport) = &self.options.http_transport {
            transport.clone()
        } else {
            self.default_transport
                .get_or_init(|| {
                    super::http::ReqwestTransport::new("")
                        .map(|transport| Arc::new(transport) as Arc<dyn RawTransport>)
                        .map_err(Error::from_box)
                })
                .clone()
                .map_err(|cause| fetch_failure(&context, cause))?
        };
        let mut via = Vec::new();
        let mut headers = Headers::new();
        loop {
            let request = RawRequest {
                method: "GET".to_owned(),
                url: url.to_string(),
                logical_host: None,
                headers: headers.clone(),
                body: None,
                content_length: 0,
                context: context.clone(),
            };
            let response = transport.send(request).await;
            let mut response: RawResponse = match response {
                Ok(Some(response)) => response,
                Ok(None) => {
                    return Err(fetch_failure(
                        &context,
                        Error::external(std::io::Error::other(
                            "HTTP transport returned no response",
                        )),
                    ));
                }
                Err(error) => return Err(fetch_failure(&context, Error::from_box(error))),
            };
            if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                let location = header(&response.headers, "location").to_owned();
                if !location.is_empty() {
                    let next = url.join(&location);
                    drain_redirect(&mut response).await;
                    let next =
                        next.map_err(|cause| fetch_failure(&context, Error::external(cause)))?;
                    via.push(url.to_string());
                    let decision = if let Some(hook) = &self.options.redirect_hook {
                        hook.check(next.as_str(), &via)
                            .map_err(|cause| fetch_failure(&context, cause))?
                    } else {
                        if via.len() >= 10 {
                            return Err(fetch_failure(
                                &context,
                                Error::external(std::io::Error::other(
                                    "stopped after 10 redirects",
                                )),
                            ));
                        }
                        RedirectDecision::Follow
                    };
                    if matches!(decision, RedirectDecision::Stop) {
                        return Err(Error::new(
                            ErrorCode::SourceHttpStatus,
                            "image source returned an unsuccessful HTTP status",
                            None,
                        ));
                    }
                    validate_source_url(next.as_str())
                        .map_err(|cause| fetch_failure(&context, cause))?;
                    headers = Headers::new();
                    if !(url.scheme() == "https" && next.scheme() == "http") {
                        let mut referer = url.clone();
                        referer.set_fragment(None);
                        headers.insert("Referer".to_owned(), vec![referer.to_string()]);
                    }
                    url = next;
                    continue;
                }
            }
            let outcome = if !(200..300).contains(&response.status) {
                Err(Error::new(
                    ErrorCode::SourceHttpStatus,
                    "image source returned an unsuccessful HTTP status",
                    None,
                ))
            } else {
                self.copy_body(context.clone(), response.body.as_deref_mut())
                    .await
            };
            if let Some(body) = &mut response.body {
                let _ = body.close().await;
            }
            return outcome;
        }
    }
    async fn copy_body(
        &self,
        context: CallerContext,
        body: Option<&mut (dyn RawBody + 'static)>,
    ) -> std::result::Result<Arc<Snapshot>, Error> {
        let mut temporary = self.temporary()?;
        if let Some(body) = body {
            let mut buffer = [0; 32 * 1024];
            loop {
                if let Some(error) = context.error() {
                    return Err(error.into());
                }
                let read = body.read(&mut buffer).await;
                if read.count > buffer.len() {
                    return Err(Error::new(
                        ErrorCode::SourceReadFailed,
                        "could not read image source",
                        None,
                    ));
                }
                temporary
                    .write(&buffer[..read.count])
                    .map_err(|error| context.error().map(Error::from).unwrap_or(error))?;
                if let Some(cause) = read.error {
                    return Err(context.error().map(Error::from).unwrap_or_else(|| {
                        Error::new(
                            ErrorCode::SourceReadFailed,
                            "could not read image source",
                            Some(Error::from_box(cause)),
                        )
                    }));
                }
                if read.eof {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }
        temporary.finish(SourceKind::Url)
    }
}
impl SourceLoader for Loader {
    fn load<'a>(
        &'a self,
        context: CallerContext,
        source: &'a str,
    ) -> ReverseFuture<'a, std::result::Result<Arc<Snapshot>, Error>> {
        Box::pin(self.load_inner(context, source))
    }
}
#[derive(Debug)]
pub struct Snapshot {
    state: Mutex<SnapshotState>,
    kind: SourceKind,
    sha256: String,
    size: i64,
}
#[derive(Debug)]
struct SnapshotState {
    path: PathBuf,
    closed: bool,
}
impl Snapshot {
    pub fn kind(&self) -> SourceKind {
        self.kind
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn size(&self) -> i64 {
        self.size
    }
    pub fn open(&self) -> std::result::Result<File, Error> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.closed {
            return Err(Error::new(
                ErrorCode::SnapshotFailed,
                "image snapshot is closed",
                None,
            ));
        }
        File::open(&state.path).map_err(|cause| {
            Error::new(
                ErrorCode::SnapshotFailed,
                "could not open image snapshot",
                Some(Error::external(cause)),
            )
        })
    }
    pub fn close(&self) -> std::result::Result<(), Error> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.closed {
            return Ok(());
        }
        let removal = fs::remove_file(&state.path).or_else(|error| {
            if state.path.is_dir() {
                fs::remove_dir(&state.path)
            } else {
                Err(error)
            }
        });
        match removal {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => {
                return Err(Error::new(
                    ErrorCode::SnapshotFailed,
                    "could not remove image snapshot",
                    Some(Error::external(cause)),
                ));
            }
        }
        state.closed = true;
        Ok(())
    }
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
struct TemporarySnapshot {
    path: PathBuf,
    file: Option<File>,
    digest: Sha256,
    size: i64,
    retained: bool,
}
impl TemporarySnapshot {
    fn write(&mut self, bytes: &[u8]) -> std::result::Result<(), Error> {
        self.file
            .as_mut()
            .expect("unfinished snapshot")
            .write_all(bytes)
            .map_err(|cause| {
                Error::new(
                    ErrorCode::SourceReadFailed,
                    "could not read image source",
                    Some(Error::external(cause)),
                )
            })?;
        self.digest.update(bytes);
        self.size += bytes.len() as i64;
        Ok(())
    }
    fn finish(mut self, kind: SourceKind) -> std::result::Result<Arc<Snapshot>, Error> {
        close_file(self.file.take().expect("unfinished snapshot")).map_err(|cause| {
            Error::new(
                ErrorCode::SnapshotFailed,
                "could not finalize image snapshot",
                Some(Error::external(cause)),
            )
        })?;
        let sha256 = self
            .digest
            .clone()
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.retained = true;
        Ok(Arc::new(Snapshot {
            state: Mutex::new(SnapshotState {
                path: self.path.clone(),
                closed: false,
            }),
            kind,
            sha256,
            size: self.size,
        }))
    }
}
impl Drop for TemporarySnapshot {
    fn drop(&mut self) {
        if !self.retained {
            drop(self.file.take());
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn close_file(file: File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::IntoRawFd;
        let descriptor = file.into_raw_fd();
        if unsafe { libc::close(descriptor) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::IntoRawHandle;
        let handle = file.into_raw_handle();
        if unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        drop(file);
        Ok(())
    }
}
fn not_regular() -> Error {
    Error::new(
        ErrorCode::SourceNotRegularFile,
        "image source must be a regular file",
        None,
    )
}
fn fetch_failure(context: &CallerContext, cause: Error) -> Error {
    context.error().map(Error::from).unwrap_or_else(|| {
        Error::new(
            ErrorCode::SourceReadFailed,
            "could not fetch image source",
            Some(cause),
        )
    })
}
fn has_url_scheme(source: &str) -> bool {
    let Some((scheme, _)) = source.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}
fn validate_source_url(source: &str) -> std::result::Result<Url, Error> {
    let invalid = || {
        Error::new(
            ErrorCode::InvalidSource,
            "image source URL is invalid",
            None,
        )
    };
    let Some((scheme, rest)) = source.split_once(':') else {
        return Err(invalid());
    };
    if !rest.starts_with("//")
        || rest[2..]
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .is_empty()
    {
        return Err(invalid());
    }
    let url = Url::parse(source).map_err(|cause| {
        Error::new(
            ErrorCode::InvalidSource,
            "image source URL is invalid",
            Some(Error::external(cause)),
        )
    })?;
    if url.host_str().is_none() {
        return Err(invalid());
    }
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(Error::new(
            ErrorCode::InvalidSource,
            "image source must use HTTP or HTTPS",
            None,
        ));
    }
    if rest[2..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .contains('@')
    {
        return Err(Error::new(
            ErrorCode::InvalidSource,
            "image source URL must not contain user information",
            None,
        ));
    }
    Ok(url)
}
fn header<'a>(headers: &'a Headers, name: &str) -> &'a str {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, values)| values.first())
        .map(String::as_str)
        .unwrap_or("")
}
async fn drain_redirect(response: &mut RawResponse) {
    if let Some(body) = &mut response.body {
        if response.content_length <= 2048 {
            let mut remaining = 2048;
            let mut buffer = [0; 2048];
            while remaining > 0 {
                let read = body.read(&mut buffer[..remaining]).await;
                if read.count > remaining {
                    break;
                }
                remaining -= read.count;
                if read.eof || read.error.is_some() {
                    break;
                }
                if read.count == 0 {
                    tokio::task::yield_now().await;
                }
            }
        }
        let _ = body.close().await;
    }
}
