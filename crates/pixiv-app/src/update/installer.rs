use super::source::{ReleaseSource, ReleaseSourceKind, ReleaseSourceSelector};
use super::{
    BuildInfo, CallerContext, ExternalError, InstallSource, Release, ReleaseAsset, ReleaseCache,
    ReleaseInstaller, SourceDetector, UpdateFuture, check_context, go_quote, join, message, wrap,
};
use crate::reverse_search::http::{HttpRequest, HttpTransport};
use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use flate2::read::MultiGzDecoder;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::Arc,
};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

pub const RELEASE_SIGNING_KEY_ID: &str = "ed25519-2c27e77742d3c33a";
pub const RELEASE_SIGNING_PUBLIC_KEY: [u8; 32] = [
    0xee, 0xb2, 0xe2, 0xef, 0xdd, 0x60, 0xb0, 0x61, 0x6c, 0x0f, 0x61, 0x2b, 0xef, 0x59, 0xe1, 0xe5,
    0x55, 0x40, 0x5f, 0xf8, 0xab, 0xc0, 0xdb, 0xeb, 0x59, 0x5c, 0x09, 0xdb, 0x9a, 0x78, 0xb1, 0x44,
];
pub fn production_trusted_keys() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([(
        RELEASE_SIGNING_KEY_ID.into(),
        RELEASE_SIGNING_PUBLIC_KEY.to_vec(),
    )])
}
pub trait ExecutableLocator: Send + Sync {
    fn executable(&self) -> Result<PathBuf, ExternalError>;
}
#[derive(Default)]
pub struct NativeExecutableLocator;
impl ExecutableLocator for NativeExecutableLocator {
    fn executable(&self) -> Result<PathBuf, ExternalError> {
        std::env::current_exe().map_err(|e| Box::new(e) as ExternalError)
    }
}
pub trait BinaryChecker: Send + Sync {
    fn check(
        &self,
        context: CallerContext,
        path: PathBuf,
        expected_tag: String,
    ) -> UpdateFuture<'_, Result<(), ExternalError>>;
}
pub trait FileReplacer: Send + Sync {
    fn replace(&self, source: &Path, target: &Path) -> Result<(), ExternalError>;
}
pub trait InstallFile: Send {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), ExternalError>;
    fn sync(&mut self) -> Result<(), ExternalError>;
    fn set_permissions(&mut self, mode: u32) -> Result<(), ExternalError>;
    fn close(&mut self) -> Result<(), ExternalError>;
}
pub trait InstallFileSystem: Send + Sync {
    fn read(&self, path: &Path) -> Result<Vec<u8>, ExternalError>;
    fn resolve_executable(&self, path: &Path) -> Result<PathBuf, ExternalError>;
    fn create_dir_all(&self, path: &Path, mode: u32) -> Result<(), ExternalError>;
    fn set_permissions(&self, path: &Path, mode: u32) -> Result<(), ExternalError>;
    fn create_temp_dir(&self, parent: &Path, prefix: &str) -> Result<PathBuf, ExternalError>;
    fn create_file(&self, path: &Path, mode: u32) -> Result<Box<dyn InstallFile>, ExternalError>;
    fn create_temp_file(
        &self,
        parent: &Path,
        prefix: &str,
    ) -> Result<(PathBuf, Box<dyn InstallFile>), ExternalError>;
    fn rename(&self, source: &Path, target: &Path) -> Result<(), ExternalError>;
    fn remove(&self, path: &Path) -> Result<(), ExternalError>;
    fn remove_all(&self, path: &Path) -> Result<(), ExternalError>;
}
#[derive(Default)]
pub struct NativeInstallFileSystem;
fn io_text(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "no such file or directory".into(),
        io::ErrorKind::AlreadyExists => "file exists".into(),
        io::ErrorKind::PermissionDenied => "permission denied".into(),
        io::ErrorKind::NotADirectory => "not a directory".into(),
        io::ErrorKind::IsADirectory => "is a directory".into(),
        io::ErrorKind::DirectoryNotEmpty => "directory not empty".into(),
        _ => error.to_string(),
    }
}
#[derive(Debug)]
struct FileOperationError {
    operation: String,
    cause: io::Error,
}
impl fmt::Display for FileOperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.operation, io_text(&self.cause))
    }
}
impl Error for FileOperationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}
fn file_error(operation: impl Into<String>, cause: io::Error) -> ExternalError {
    Box::new(FileOperationError {
        operation: operation.into(),
        cause,
    })
}
fn is_not_found(error: &(dyn Error + 'static)) -> bool {
    if error
        .downcast_ref::<io::Error>()
        .is_some_and(|e| e.kind() == io::ErrorKind::NotFound)
    {
        return true;
    }
    error.source().is_some_and(is_not_found)
}
fn path_quote(path: &Path) -> String {
    go_quote(&path.to_string_lossy())
}
fn random_name(parent: &Path, prefix: &str) -> Result<PathBuf, ExternalError> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(|e| message(e.to_string()))?;
    Ok(parent.join(format!("{prefix}{}", u64::from_le_bytes(bytes))))
}
struct NativeInstallFile {
    file: Option<File>,
    path: PathBuf,
}
impl InstallFile for NativeInstallFile {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), ExternalError> {
        self.file
            .as_mut()
            .ok_or_else(|| message("file already closed"))?
            .write_all(bytes)
            .map_err(|e| file_error(format!("write {}", self.path.display()), e))
    }
    fn sync(&mut self) -> Result<(), ExternalError> {
        self.file
            .as_mut()
            .ok_or_else(|| message("file already closed"))?
            .sync_all()
            .map_err(|e| file_error(format!("sync {}", self.path.display()), e))
    }
    fn set_permissions(&mut self, mode: u32) -> Result<(), ExternalError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            self.file
                .as_mut()
                .ok_or_else(|| message("file already closed"))?
                .set_permissions(fs::Permissions::from_mode(mode))
                .map_err(|e| file_error(format!("chmod {}", self.path.display()), e))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        Ok(())
    }
    fn close(&mut self) -> Result<(), ExternalError> {
        if let Some(file) = self.file.take() {
            #[cfg(unix)]
            {
                use std::os::fd::IntoRawFd;
                let fd = file.into_raw_fd();
                if unsafe { libc::close(fd) } != 0 {
                    return Err(file_error(
                        format!("close {}", self.path.display()),
                        io::Error::last_os_error(),
                    ));
                }
            }
            #[cfg(not(unix))]
            drop(file);
        }
        Ok(())
    }
}
impl InstallFileSystem for NativeInstallFileSystem {
    fn read(&self, path: &Path) -> Result<Vec<u8>, ExternalError> {
        fs::read(path).map_err(|e| file_error(format!("read {}", path.display()), e))
    }
    fn resolve_executable(&self, path: &Path) -> Result<PathBuf, ExternalError> {
        let target = std::path::absolute(path).map_err(|e| {
            wrap(
                format!("resolve current executable {}", path_quote(path)),
                Box::new(e),
            )
        })?;
        if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            fs::canonicalize(&target).map_err(|e| {
                let missing = if e.kind() == io::ErrorKind::NotFound {
                    fs::read_link(&target).ok().map(|link| {
                        if link.is_absolute() {
                            link
                        } else {
                            target.parent().unwrap_or(Path::new(".")).join(link)
                        }
                    })
                } else {
                    None
                };
                wrap(
                    format!("resolve executable symlink {}", path_quote(&target)),
                    file_error(
                        format!("lstat {}", missing.as_deref().unwrap_or(&target).display()),
                        e,
                    ),
                )
            })
        } else {
            Ok(target)
        }
    }
    fn create_dir_all(&self, path: &Path, mode: u32) -> Result<(), ExternalError> {
        if fs::metadata(path).is_ok_and(|metadata| !metadata.is_dir()) {
            return Err(file_error(
                format!("mkdir {}", path.display()),
                io::Error::from(io::ErrorKind::NotADirectory),
            ));
        }
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        builder
            .create(path)
            .map_err(|e| file_error(format!("mkdir {}", path.display()), e))
    }
    fn set_permissions(&self, path: &Path, mode: u32) -> Result<(), ExternalError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(mode))
                .map_err(|e| file_error(format!("chmod {}", path.display()), e))?;
        }
        #[cfg(not(unix))]
        let _ = (path, mode);
        Ok(())
    }
    fn create_temp_dir(&self, parent: &Path, prefix: &str) -> Result<PathBuf, ExternalError> {
        fs::metadata(parent).map_err(|e| file_error(format!("stat {}", parent.display()), e))?;
        loop {
            let path = random_name(parent, prefix)?;
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(file_error(format!("mkdir {}", path.display()), error)),
            }
        }
    }
    fn create_file(&self, path: &Path, mode: u32) -> Result<Box<dyn InstallFile>, ExternalError> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let file = options
            .open(path)
            .map_err(|e| file_error(format!("open {}", path.display()), e))?;
        Ok(Box::new(NativeInstallFile {
            file: Some(file),
            path: path.into(),
        }))
    }
    fn create_temp_file(
        &self,
        parent: &Path,
        prefix: &str,
    ) -> Result<(PathBuf, Box<dyn InstallFile>), ExternalError> {
        loop {
            let path = random_name(parent, prefix)?;
            match self.create_file(&path, 0o600) {
                Ok(file) => return Ok((path, file)),
                Err(error)
                    if error
                        .source()
                        .and_then(|e| e.downcast_ref::<io::Error>())
                        .is_some_and(|e| e.kind() == io::ErrorKind::AlreadyExists) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
    }
    fn rename(&self, source: &Path, target: &Path) -> Result<(), ExternalError> {
        fs::rename(source, target).map_err(|error| {
            let error = if fs::metadata(target).is_ok_and(|m| m.is_dir())
                && error.kind() == io::ErrorKind::IsADirectory
            {
                io::Error::from(io::ErrorKind::AlreadyExists)
            } else {
                error
            };
            file_error(
                format!("rename {} {}", source.display(), target.display()),
                error,
            )
        })
    }
    fn remove(&self, path: &Path) -> Result<(), ExternalError> {
        let result = if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        };
        result.map_err(|e| file_error(format!("remove {}", path.display()), e))
    }
    fn remove_all(&self, path: &Path) -> Result<(), ExternalError> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(file_error(format!("unlinkat {}", path.display()), error)),
            Ok(metadata) => {
                let result = if metadata.is_dir() {
                    fs::remove_dir_all(path)
                } else {
                    fs::remove_file(path)
                };
                result.map_err(|e| file_error(format!("unlinkat {}", path.display()), e))
            }
        }
    }
}
#[derive(Default)]
pub struct ReleaseInstallerOptions {
    pub http_transport: Option<Arc<dyn HttpTransport>>,
    pub trusted_keys: BTreeMap<String, Vec<u8>>,
    pub executable_path: Option<Arc<dyn ExecutableLocator>>,
    pub goos: String,
    pub goarch: String,
    pub binary_checker: Option<Arc<dyn BinaryChecker>>,
    pub replacer: Option<Arc<dyn FileReplacer>>,
    pub file_system: Option<Arc<dyn InstallFileSystem>>,
    pub enable_public_release_sources: bool,
    pub source_selector: Option<Arc<ReleaseSourceSelector>>,
}
struct UpdateMaterial {
    fs: Arc<dyn InstallFileSystem>,
    work: PathBuf,
    stage: Option<PathBuf>,
    active: bool,
}
impl Drop for UpdateMaterial {
    fn drop(&mut self) {
        if self.active {
            if let Some(stage) = &self.stage {
                let _ = self.fs.remove(stage);
            }
            let _ = self.fs.remove_all(&self.work);
        }
    }
}
pub struct SignedReleaseInstaller {
    transport: Arc<dyn HttpTransport>,
    keys: BTreeMap<String, Vec<u8>>,
    locator: Arc<dyn ExecutableLocator>,
    goos: String,
    goarch: String,
    checker: Arc<dyn BinaryChecker>,
    replacer: Arc<dyn FileReplacer>,
    fs: Arc<dyn InstallFileSystem>,
    selector: Option<Arc<ReleaseSourceSelector>>,
}
pub fn native_goos() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        value => value,
    }
}
pub fn native_goarch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        value => value,
    }
}
impl SignedReleaseInstaller {
    pub fn new(options: ReleaseInstallerOptions) -> Self {
        let transport = options
            .http_transport
            .unwrap_or_else(super::http::default_transport);
        let goos = if options.goos.is_empty() {
            native_goos().into()
        } else {
            options.goos
        };
        let selector = options.source_selector.or_else(|| {
            options
                .enable_public_release_sources
                .then(|| Arc::new(ReleaseSourceSelector::default(transport.clone())))
        });
        Self {
            transport,
            keys: options.trusted_keys.clone(),
            locator: options
                .executable_path
                .unwrap_or_else(|| Arc::new(NativeExecutableLocator)),
            goos: goos.clone(),
            goarch: if options.goarch.is_empty() {
                native_goarch().into()
            } else {
                options.goarch
            },
            checker: options
                .binary_checker
                .unwrap_or_else(|| Arc::new(ProcessReleaseBinaryChecker)),
            replacer: options
                .replacer
                .unwrap_or_else(|| Arc::new(NativeReleaseFileReplacer::new(&goos))),
            fs: options
                .file_system
                .unwrap_or_else(|| Arc::new(NativeInstallFileSystem)),
            selector,
        }
    }
    async fn download_url(
        &self,
        context: CallerContext,
        asset: &ReleaseAsset,
        url: &str,
    ) -> Result<Vec<u8>, ExternalError> {
        let request = HttpRequest {
            method: "GET".into(),
            url: url.into(),
            logical_host: None,
            headers: BTreeMap::new(),
            body: None,
            content_length: 0,
            context,
        };
        let mut response = self
            .transport
            .send(request)
            .await
            .map_err(|e| wrap(format!("request asset {}", go_quote(&asset.name)), e))?
            .ok_or_else(|| message("http: nil Response"))?;
        if response.status != 200 {
            if let Some(body) = response.body.as_mut() {
                let _ = body.close().await;
            }
            let reason = reqwest::StatusCode::from_u16(response.status)
                .ok()
                .and_then(|s| s.canonical_reason())
                .unwrap_or("");
            return Err(message(format!(
                "asset {} returned HTTP {} {reason}",
                go_quote(&asset.name),
                response.status
            )));
        }
        let mut data = Vec::new();
        let mut error = None;
        if let Some(body) = response.body.as_mut() {
            loop {
                let mut chunk = [0; 32768];
                let read = body.read(&mut chunk).await;
                data.extend_from_slice(&chunk[..read.count.min(chunk.len())]);
                if let Some(cause) = read.error {
                    error = Some(wrap(format!("read asset {}", go_quote(&asset.name)), cause));
                    break;
                }
                if read.eof {
                    break;
                }
                if read.count == 0 {
                    tokio::task::yield_now().await;
                }
            }
            let _ = body.close().await;
        }
        match error {
            Some(error) => Err(error),
            None => Ok(data),
        }
    }
    async fn download(
        &self,
        context: CallerContext,
        asset: &ReleaseAsset,
        sources: &[ReleaseSource],
    ) -> Result<Vec<u8>, ExternalError> {
        if sources.is_empty() {
            return self.download_url(context, asset, &asset.download_url).await;
        }
        let mut errors = Vec::new();
        for source in sources {
            let result = match source.asset_url(&asset.download_url) {
                Ok(url) => self.download_url(context.clone(), asset, &url).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(body) => return Ok(body),
                Err(error) => errors.push(wrap(
                    format!("release source {}", go_quote(source.id())),
                    error,
                )),
            }
        }
        Err(wrap(
            format!(
                "download asset {} from every release source",
                go_quote(&asset.name)
            ),
            join(errors),
        ))
    }
    async fn install_owned(
        &self,
        context: CallerContext,
        release: Release,
    ) -> Result<(), ExternalError> {
        if self.keys.is_empty() {
            return Err(message("trusted release signing key is not configured"));
        }
        check_context(&context, "prepare release update")?;
        let version = super::release::SemanticVersion::parse(&release.tag_name).map_err(|e| {
            wrap(
                format!("parse release tag {}", go_quote(&release.tag_name)),
                e,
            )
        })?;
        if release.version.is_empty() || release.version != version.to_string() {
            return Err(message(format!(
                "release version {} does not match tag {}",
                go_quote(&release.version),
                go_quote(&release.tag_name)
            )));
        }
        let name = archive_name(&release.version, &self.goos, &self.goarch);
        let assets = select_assets(&release.assets, &name)?;
        for asset in &assets {
            let expected = format!(
                "https://github.com/FlanChanXwO/pixiv-cli/releases/download/{}/{}",
                release.tag_name, asset.name
            );
            if asset.download_url != expected {
                return Err(message(format!(
                    "release asset {} has untrusted download URL {}: expected exact GitHub HTTPS release URL {}",
                    go_quote(&asset.name),
                    go_quote(&asset.download_url),
                    go_quote(&expected)
                )));
            }
        }
        let sources = if let Some(selector) = &self.selector {
            selector
                .ordered(
                    context.clone(),
                    ReleaseSourceKind::Asset,
                    &assets[1].download_url,
                )
                .await
                .map_err(|e| wrap("select source for release assets", e))?
        } else {
            Vec::new()
        };
        let checksums = self
            .download(context.clone(), assets[1], &sources)
            .await
            .map_err(|e| wrap("download checksums.txt", e))?;
        let manifest = self
            .download(context.clone(), assets[2], &sources)
            .await
            .map_err(|e| wrap("download checksums.json", e))?;
        verify_manifest(&manifest, &checksums, &self.keys)?;
        verify_checksum(&checksums, &name, None)?;
        let archive = self
            .download(context.clone(), assets[0], &sources)
            .await
            .map_err(|e| wrap(format!("download release archive {}", go_quote(&name)), e))?;
        check_context(&context, "extract release archive")?;
        verify_checksum(&checksums, &name, Some(&archive))?;
        let target = self
            .locator
            .executable()
            .map_err(|e| wrap("locate current executable", e))?;
        let target = self.fs.resolve_executable(&target)?;
        let parent = target.parent().unwrap_or(Path::new("."));
        let work = self
            .fs
            .create_temp_dir(parent, ".pixiv-update-")
            .map_err(|e| {
                wrap(
                    format!(
                        "create update temporary directory beside {}",
                        path_quote(&target)
                    ),
                    e,
                )
            })?;
        let mut material = UpdateMaterial {
            fs: self.fs.clone(),
            work: work.clone(),
            stage: None,
            active: true,
        };
        let result = async {
            check_context(&context, "extract release archive")?;
            let candidate = work.join(binary_name(&self.goos));
            extract_binary(
                &archive,
                &name,
                &candidate,
                binary_name(&self.goos),
                self.fs.as_ref(),
            )?;
            check_context(&context, "preflight downloaded executable")?;
            self.checker
                .check(context.clone(), candidate.clone(), release.tag_name)
                .await
                .map_err(|e| {
                    wrap(
                        format!("preflight downloaded executable {}", path_quote(&candidate)),
                        e,
                    )
                })?;
            check_context(&context, "stage verified update")?;
            let (staged, mut file) = self
                .fs
                .create_temp_file(parent, ".pixiv-update-stage-")
                .map_err(|e| {
                    wrap(
                        format!("create staged update file beside {}", path_quote(&target)),
                        e,
                    )
                })?;
            material.stage = Some(staged.clone());
            file.close().map_err(|e| {
                wrap(
                    format!("close staged update file {}", path_quote(&staged)),
                    e,
                )
            })?;
            check_context(&context, "stage verified update")?;
            self.fs.remove(&staged).map_err(|e| {
                wrap(
                    format!("prepare staged update file {}", path_quote(&staged)),
                    e,
                )
            })?;
            check_context(&context, "stage verified update")?;
            self.fs.rename(&candidate, &staged).map_err(|e| {
                wrap(
                    format!("stage verified update file {}", path_quote(&staged)),
                    e,
                )
            })?;
            check_context(&context, "replace release executable")?;
            check_context(&context, "replace release executable")?;
            self.fs.remove(&work).map_err(|e| {
                wrap(
                    format!(
                        "remove verified update temporary directory {}",
                        path_quote(&work)
                    ),
                    e,
                )
            })?;
            check_context(&context, "replace release executable")?;
            self.replacer.replace(&staged, &target).map_err(|e| {
                wrap(
                    format!("atomically replace executable {}", path_quote(&target)),
                    e,
                )
            })
        }
        .await;
        material.active = false;
        if let Err(mut error) = result {
            if let Some(staged) = material.stage.take()
                && !must_preserve_replacement_source(error.as_ref())
                && let Err(cleanup) = self.fs.remove(&staged)
                && !is_not_found(cleanup.as_ref())
            {
                error = join(vec![
                    error,
                    wrap(
                        format!("remove staged update file {}", path_quote(&staged)),
                        cleanup,
                    ),
                ]);
            }
            if let Err(cleanup) = self.fs.remove_all(&work)
                && !is_not_found(cleanup.as_ref())
            {
                error = join(vec![
                    error,
                    wrap(
                        format!("remove update temporary directory {}", path_quote(&work)),
                        cleanup,
                    ),
                ]);
            }
            return Err(error);
        }
        Ok(())
    }
}
impl ReleaseInstaller for SignedReleaseInstaller {
    fn install(
        &self,
        context: CallerContext,
        release: Release,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(self.install_owned(context, release))
    }
}
fn archive_name(version: &str, goos: &str, arch: &str) -> String {
    format!(
        "pixiv-cli_{version}_{goos}_{arch}{}",
        if goos == "windows" { ".zip" } else { ".tar.gz" }
    )
}
fn binary_name(goos: &str) -> &'static str {
    if goos == "windows" {
        "pixiv.exe"
    } else {
        "pixiv"
    }
}
fn select_assets<'a>(
    assets: &'a [ReleaseAsset],
    name: &str,
) -> Result<[&'a ReleaseAsset; 3], ExternalError> {
    let names = [name, "checksums.txt", "checksums.json"];
    let mut selected = [None; 3];
    for asset in assets {
        if let Some(index) = names.iter().position(|name| *name == asset.name) {
            if selected[index].is_some() {
                return Err(message(format!(
                    "release contains duplicate {}asset {}",
                    if index == 0 { "archive " } else { "" },
                    go_quote(names[index])
                )));
            }
            selected[index] = Some(asset);
        }
    }
    for (index, asset) in selected.iter().enumerate() {
        if asset.is_none() {
            return Err(message(format!(
                "release has no {}asset {}",
                if index == 0 { "platform archive " } else { "" },
                go_quote(names[index])
            )));
        }
    }
    let selected = selected.map(Option::unwrap);
    for asset in selected {
        if asset.download_url.is_empty() {
            return Err(message(format!(
                "release asset {} has no download URL",
                go_quote(&asset.name)
            )));
        }
    }
    Ok(selected)
}
#[derive(Default)]
struct Manifest {
    key_id: String,
    checksums_sha256: String,
    signature: String,
}
impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct ManifestVisitor;
        impl<'de> Visitor<'de> for ManifestVisitor {
            type Value = Manifest;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("checksums manifest")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Manifest, E> {
                Ok(Manifest::default())
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Manifest, M::Error> {
                let mut out = Manifest::default();
                while let Some(key) = map.next_key::<String>()? {
                    let canonical = key.to_ascii_lowercase();
                    let slot = match canonical.as_str() {
                        "key_id" => &mut out.key_id,
                        "checksums_sha256" => &mut out.checksums_sha256,
                        "signature" => &mut out.signature,
                        _ => {
                            return Err(de::Error::custom(format!(
                                "json: unknown field {}",
                                go_quote(&key)
                            )));
                        }
                    };
                    let value = map.next_value::<serde_json::Value>()?;
                    match value {
                        serde_json::Value::Null => {}
                        serde_json::Value::String(value) => *slot = value,
                        value => {
                            let kind = match value {
                                serde_json::Value::Number(_) => "number",
                                serde_json::Value::Bool(_) => "bool",
                                serde_json::Value::Array(_) => "array",
                                _ => "object",
                            };
                            return Err(de::Error::custom(format!(
                                "json: cannot unmarshal {kind} into Go struct field checksumsManifest.{canonical} of type string"
                            )));
                        }
                    }
                }
                Ok(out)
            }
        }
        decoder.deserialize_any(ManifestVisitor)
    }
}
fn json_message(error: &serde_json::Error) -> String {
    if error.is_eof() {
        return "unexpected EOF".into();
    }
    let value = error.to_string();
    value
        .rsplit_once(" at line ")
        .map_or(value.clone(), |(message, _)| message.to_string())
}
fn verify_manifest(
    bytes: &[u8],
    checksums: &[u8],
    keys: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ExternalError> {
    let text = String::from_utf8_lossy(bytes);
    let mut decoder = serde_json::Deserializer::from_str(&text);
    let manifest = Manifest::deserialize(&mut decoder)
        .map_err(|e| message(format!("decode checksums manifest: {}", json_message(&e))))?;
    match decoder.into_iter::<serde_json::Value>().next() {
        Some(Ok(_)) => {
            return Err(message(
                "decode checksums manifest: contains more than one JSON value",
            ));
        }
        None => {}
        Some(Err(error)) => {
            let tail = text.trim_end();
            let cause = if let Some(ch) = tail.chars().last().filter(|ch| *ch == '!') {
                format!("invalid character '{ch}' looking for beginning of value")
            } else {
                json_message(&error)
            };
            return Err(message(format!(
                "decode checksums manifest trailing data: {cause}"
            )));
        }
    }
    let key = keys.get(&manifest.key_id).ok_or_else(|| {
        message(format!(
            "checksums manifest references unknown key ID {}",
            go_quote(&manifest.key_id)
        ))
    })?;
    if key.len() != 32 {
        return Err(message(format!(
            "trusted public key {} has invalid length",
            go_quote(&manifest.key_id)
        )));
    }
    if manifest.checksums_sha256 != hex(&Sha256::digest(checksums)) {
        return Err(message(
            "checksums manifest SHA-256 does not match checksums.txt",
        ));
    }
    let clean = manifest.signature.replace(['\r', '\n'], "");
    let engine = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::RequireCanonical)
            .with_decode_allow_trailing_bits(true),
    );
    let signature = engine.decode(&clean).map_err(|error| {
        let index = match error {
            base64::DecodeError::InvalidByte(index, _)
            | base64::DecodeError::InvalidLastSymbol(index, _)
            | base64::DecodeError::InvalidLength(index) => index,
            base64::DecodeError::InvalidPadding => (clean.len() / 4) * 4,
        };
        message(format!(
            "decode checksums manifest signature: illegal base64 data at input byte {index}"
        ))
    })?;
    if UnparsedPublicKey::new(&ED25519, key)
        .verify(checksums, &signature)
        .is_err()
    {
        return Err(message(format!(
            "checksums manifest Ed25519 signature verification failed for key ID {}",
            go_quote(&manifest.key_id)
        )));
    }
    Ok(())
}
fn go_quote_bytes(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    let mut remaining = bytes;
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(text) => {
                let quoted = go_quote(text);
                out.push_str(&quoted[1..quoted.len() - 1]);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let quoted = go_quote(
                        std::str::from_utf8(&remaining[..valid]).expect("validated UTF-8 prefix"),
                    );
                    out.push_str(&quoted[1..quoted.len() - 1]);
                }
                out.push_str(&format!("\\x{:02x}", remaining[valid]));
                remaining = &remaining[valid + 1..];
            }
        }
    }
    out.push('"');
    out
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn verify_checksum(
    bytes: &[u8],
    filename: &str,
    archive: Option<&[u8]>,
) -> Result<(), ExternalError> {
    let text = String::from_utf8_lossy(bytes);
    let mut expected = None;
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 2 {
            return Err(message(format!(
                "parse checksums.txt line {}",
                go_quote(line)
            )));
        }
        if fields[1].strip_prefix('*').unwrap_or(fields[1]) != filename {
            continue;
        }
        if expected.is_some() {
            return Err(message(format!(
                "checksums.txt has duplicate entry for {}",
                go_quote(filename)
            )));
        }
        let digest = fields[0];
        if digest.len() != 64 || digest.to_lowercase() != digest {
            return Err(message(format!(
                "checksums.txt has invalid SHA-256 for {}",
                go_quote(filename)
            )));
        }
        if let Some(byte) = digest.bytes().find(|byte| !byte.is_ascii_hexdigit()) {
            return Err(message(format!(
                "checksums.txt has invalid SHA-256 for {}: encoding/hex: invalid byte: U+{:04X} '{}'",
                go_quote(filename),
                byte,
                byte as char
            )));
        }
        expected = Some(digest);
    }
    let expected = expected.ok_or_else(|| {
        message(format!(
            "checksums.txt has no entry for {}",
            go_quote(filename)
        ))
    })?;
    if let Some(archive) = archive
        && expected != hex(&Sha256::digest(archive))
    {
        return Err(message(format!(
            "release archive {} SHA-256 does not match checksums.txt",
            go_quote(filename)
        )));
    }
    Ok(())
}
fn validate_path(name: &str) -> Result<(), ExternalError> {
    let clean = name
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>();
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.split('/').any(|s| s == "..")
        || clean.is_empty()
    {
        return Err(message(format!(
            "release archive has unsafe path {}",
            go_quote(name)
        )));
    }
    Ok(())
}
fn tar_error(error: io::Error) -> ExternalError {
    let text = error.to_string();
    let message = if error.kind() == io::ErrorKind::UnexpectedEof
        || text.contains("failed to fill whole buffer")
    {
        "unexpected EOF"
    } else if text.contains("header field too long") {
        "archive/tar: header field too long"
    } else if text.contains("checksum") || text.contains("numeric") || text.contains("header") {
        "archive/tar: invalid tar header"
    } else if text.contains("corrupt") || text.contains("deflate") {
        "flate: corrupt input"
    } else {
        &text
    };
    super::message(format!("read tar release archive: {message}"))
}
fn gzip(bytes: &[u8]) -> Result<MultiGzDecoder<Cursor<&[u8]>>, ExternalError> {
    let decoder = MultiGzDecoder::new(Cursor::new(bytes));
    if decoder.header().is_none() {
        return Err(message("open gzip release archive: gzip: invalid header"));
    }
    Ok(decoder)
}
fn write_binary(
    path: &Path,
    reader: &mut dyn Read,
    fs: &dyn InstallFileSystem,
    zip: bool,
) -> Result<(), ExternalError> {
    let mut file = fs.create_file(path, 0o755).map_err(|e| {
        wrap(
            format!("create extracted release binary {}", path_quote(path)),
            e,
        )
    })?;
    let result = (|| {
        let mut chunk = [0; 32768];
        loop {
            let count = reader.read(&mut chunk).map_err(|e| {
                let cause = if zip && e.to_string().to_ascii_lowercase().contains("checksum") {
                    message("zip: checksum error")
                } else {
                    Box::new(e) as ExternalError
                };
                wrap(
                    format!("write extracted release binary {}", path_quote(path)),
                    cause,
                )
            })?;
            if count == 0 {
                break;
            }
            file.write_all(&chunk[..count]).map_err(|e| {
                wrap(
                    format!("write extracted release binary {}", path_quote(path)),
                    e,
                )
            })?;
        }
        file.sync().map_err(|e| {
            wrap(
                format!("sync extracted release binary {}", path_quote(path)),
                e,
            )
        })?;
        file.set_permissions(0o755).map_err(|e| {
            wrap(
                format!(
                    "set extracted release binary permissions {}",
                    path_quote(path)
                ),
                e,
            )
        })?;
        Ok(())
    })();
    let close = file.close().map_err(|e| {
        wrap(
            format!("close extracted release binary {}", path_quote(path)),
            e,
        )
    });
    result.and(close)
}

struct GoTarReader<R> {
    reader: R,
    queued: Cursor<Vec<u8>>,
    remaining: u64,
    done: bool,
}
impl<R: Read> GoTarReader<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            queued: Cursor::new(Vec::new()),
            remaining: 0,
            done: false,
        }
    }
    fn block(&mut self) -> io::Result<Option<[u8; 512]>> {
        let mut block = [0; 512];
        let mut count = 0;
        while count < block.len() {
            let read = self.reader.read(&mut block[count..])?;
            if read == 0 {
                if count == 0 {
                    return Ok(None);
                }
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            count += read;
        }
        Ok(Some(block))
    }
    fn special(&mut self, size: u64) -> io::Result<Vec<u8>> {
        if size > 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "archive/tar: header field too long",
            ));
        }
        let mut bytes = vec![0; size as usize];
        self.reader.read_exact(&mut bytes)?;
        let mut padding = [0; 512];
        self.reader
            .read_exact(&mut padding[..((512 - size % 512) % 512) as usize])?;
        Ok(bytes)
    }
    fn next_member(&mut self) -> io::Result<()> {
        let mut pax = BTreeMap::new();
        let mut longname = Vec::new();
        let mut longlink = Vec::new();
        loop {
            let Some(block) = self.block()? else {
                self.queued = Cursor::new(vec![0; 1024]);
                self.done = true;
                return Ok(());
            };
            if block.iter().all(|b| *b == 0) {
                if let Some(next) = self.block()?
                    && next.iter().any(|b| *b != 0)
                {
                    return Err(tar_header_invalid());
                }
                self.queued = Cursor::new(vec![0; 1024]);
                self.done = true;
                return Ok(());
            }
            let mut header = tar::Header::new_old();
            header.as_mut_bytes().copy_from_slice(&block);
            let checksum = go_tar_octal(&block[148..156])?;
            let unsigned = block
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    if (148..156).contains(&i) {
                        32
                    } else {
                        i64::from(*b)
                    }
                })
                .sum::<i64>();
            let signed = block
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    if (148..156).contains(&i) {
                        32
                    } else {
                        i64::from(*b as i8)
                    }
                })
                .sum::<i64>();
            if checksum != unsigned && checksum != signed {
                return Err(tar_header_invalid());
            }
            for field in [
                &block[100..108],
                &block[108..116],
                &block[116..124],
                &block[136..148],
            ] {
                go_tar_number(field)?;
            }
            if &block[257..263] == b"ustar\0"
                || (&block[257..263] == b"ustar " && &block[263..265] == b" \0")
            {
                go_tar_number(&block[329..337])?;
                go_tar_number(&block[337..345])?;
                if &block[508..512] == b"tar\0" && &block[257..263] == b"ustar\0" {
                    go_tar_number(&block[476..488])?;
                    go_tar_number(&block[488..500])?;
                }
            }
            let kind = block[156];
            let raw_size = go_tar_number(&block[124..136])?;
            let header_only = matches!(kind, b'1' | b'2' | b'3' | b'4' | b'5' | b'6');
            if raw_size < 0 && !header_only {
                return Err(tar_header_invalid());
            }
            let raw_size = if header_only { 0 } else { raw_size };
            if matches!(kind, b'x' | b'g') {
                let body = self.special(raw_size as u64)?;
                pax = parse_go_pax(&body)?;
                if kind == b'x' {
                    continue;
                }
                let original = go_tar_name(&block);
                let name = pax
                    .get(b"path".as_slice())
                    .filter(|v| !v.is_empty())
                    .cloned()
                    .unwrap_or(original);
                let mut out = Vec::new();
                tar_metadata(&mut out, b'L', &name);
                header.set_size(0);
                header.set_cksum();
                out.extend_from_slice(header.as_bytes());
                self.queued = Cursor::new(out);
                self.remaining = 0;
                return Ok(());
            }
            if matches!(kind, b'L' | b'K') {
                let body = self.special(raw_size as u64)?;
                let value = body.split(|b| *b == 0).next().unwrap_or_default().to_vec();
                if kind == b'L' {
                    longname = value;
                } else {
                    longlink = value;
                }
                continue;
            }
            validate_go_pax_fields(&pax)?;
            let mut name = go_tar_name(&block);
            if let Some(path) = pax.get(b"path".as_slice()).filter(|v| !v.is_empty()) {
                name = path.clone();
            }
            if !longname.is_empty() {
                name = longname;
            }
            let mut size = raw_size as u64;
            if let Some(value) = pax.get(b"size".as_slice()).filter(|v| !v.is_empty()) {
                let parsed = go_pax_integer(value)?;
                if parsed < 0 && !matches!(kind, b'1' | b'2' | b'3' | b'4' | b'5' | b'6') {
                    return Err(tar_header_invalid());
                }
                size = if parsed < 0 { 0 } else { parsed as u64 };
            }
            if kind == 0 {
                header.set_entry_type(if name.ends_with(b"/") {
                    tar::EntryType::Directory
                } else {
                    tar::EntryType::Regular
                });
            }
            if matches!(
                header.entry_type().as_byte(),
                b'1' | b'2' | b'3' | b'4' | b'5' | b'6'
            ) {
                size = 0;
            }
            if kind == b'S' {
                self.old_sparse(&block)?;
                // The library rejects Go-valid sparse layouts before their bodies can be skipped.
                header.set_entry_type(tar::EntryType::Continuous);
            } else if let Some(version_1) = go_sparse_version(&pax) {
                if matches!(
                    header.entry_type().as_byte(),
                    b'1' | b'2' | b'3' | b'4' | b'5' | b'6'
                ) {
                    return Err(tar_header_invalid());
                }
                let logical = go_sparse_size(&pax, size)?;
                let sparse = if version_1 {
                    self.sparse_map_1(&mut size, logical)?
                } else {
                    go_sparse_pax(&pax, size)?
                };
                if let Some(value) = pax
                    .get(b"GNU.sparse.name".as_slice())
                    .filter(|v| !v.is_empty())
                {
                    name = value.clone();
                }
                canonical_sparse(&mut pax, &sparse, size);
            }
            header.set_size(size);
            if &block[257..263] == b"ustar\0" {
                header.as_mut_bytes()[263..265].copy_from_slice(b"00");
            }
            header.set_cksum();
            let mut out = Vec::new();
            if !pax.is_empty() {
                let mut canonical = Vec::new();
                for (key, value) in &pax {
                    if !value.is_empty() {
                        canonical.extend_from_slice(&go_pax_record(key, value));
                    }
                }
                if !canonical.is_empty() {
                    tar_metadata(&mut out, b'x', &canonical);
                }
            }
            tar_metadata(&mut out, b'L', &name);
            if !longlink.is_empty() {
                tar_metadata(&mut out, b'K', &longlink);
            }
            out.extend_from_slice(header.as_bytes());
            self.queued = Cursor::new(out);
            self.remaining = size
                .checked_add((512 - size % 512) % 512)
                .ok_or_else(tar_header_invalid)?;
            return Ok(());
        }
    }
}
impl<R: Read> Read for GoTarReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let count = std::io::Read::read(&mut self.queued, output)?;
            if count > 0 {
                return Ok(count);
            }
            if self.remaining > 0 {
                let size = output
                    .len()
                    .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
                let count = self.reader.read(&mut output[..size])?;
                if count == 0 {
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                }
                self.remaining -= count as u64;
                return Ok(count);
            }
            if self.done {
                return Ok(0);
            }
            self.next_member()?;
        }
    }
}
#[derive(Clone)]
struct GoSparse {
    size: u64,
    data: Vec<(u64, u64)>,
}
fn sparse_too_long() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "archive/tar: sparse map too long",
    )
}
fn go_sparse_version(pax: &BTreeMap<Vec<u8>, Vec<u8>>) -> Option<bool> {
    let get = |key: &[u8]| pax.get(key).map(Vec::as_slice).unwrap_or_default();
    match (get(b"GNU.sparse.major"), get(b"GNU.sparse.minor")) {
        (b"0", b"0" | b"1") => Some(false),
        (b"1", b"0") => Some(true),
        (b"", b"") if !get(b"GNU.sparse.map").is_empty() => Some(false),
        _ => None,
    }
}
fn go_sparse_size(pax: &BTreeMap<Vec<u8>, Vec<u8>>, physical: u64) -> io::Result<u64> {
    let value = pax
        .get(b"GNU.sparse.size".as_slice())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            pax.get(b"GNU.sparse.realsize".as_slice())
                .filter(|v| !v.is_empty())
        });
    match value {
        Some(value) => u64::try_from(go_pax_integer(value)?).map_err(|_| tar_header_invalid()),
        None => Ok(physical),
    }
}
fn go_sparse_entries(tokens: &[&[u8]], count: i64, size: u64) -> io::Result<GoSparse> {
    let count = usize::try_from(count).map_err(|_| tar_header_invalid())?;
    if count.checked_mul(2) != Some(tokens.len()) {
        return Err(tar_header_invalid());
    }
    if count > 1024 * 1024 {
        return Err(sparse_too_long());
    }
    let mut data = Vec::with_capacity(count);
    let mut previous = 0;
    for pair in tokens.chunks_exact(2) {
        let offset = u64::try_from(go_pax_integer(pair[0])?).map_err(|_| tar_header_invalid())?;
        let length = u64::try_from(go_pax_integer(pair[1])?).map_err(|_| tar_header_invalid())?;
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= i64::MAX as u64 && *end <= size)
            .ok_or_else(tar_header_invalid)?;
        if previous > offset {
            return Err(tar_header_invalid());
        }
        previous = end;
        data.push((offset, length));
    }
    Ok(GoSparse { size, data })
}
fn go_sparse_pax(pax: &BTreeMap<Vec<u8>, Vec<u8>>, physical: u64) -> io::Result<GoSparse> {
    let count = go_pax_integer(
        pax.get(b"GNU.sparse.numblocks".as_slice())
            .map(Vec::as_slice)
            .unwrap_or_default(),
    )?;
    let map = pax
        .get(b"GNU.sparse.map".as_slice())
        .map(Vec::as_slice)
        .unwrap_or_default();
    let tokens = if map.is_empty() {
        Vec::new()
    } else {
        map.split(|b| *b == b',').collect()
    };
    go_sparse_entries(&tokens, count, go_sparse_size(pax, physical)?)
}
fn canonical_sparse(pax: &mut BTreeMap<Vec<u8>, Vec<u8>>, sparse: &GoSparse, physical: u64) {
    pax.retain(|key, _| !key.starts_with(b"GNU.sparse."));
    for (key, value) in [
        (b"GNU.sparse.major".as_slice(), b"0".to_vec()),
        (b"GNU.sparse.minor".as_slice(), b"1".to_vec()),
        (
            b"GNU.sparse.size".as_slice(),
            sparse.size.to_string().into_bytes(),
        ),
        (
            b"GNU.sparse.numblocks".as_slice(),
            sparse.data.len().to_string().into_bytes(),
        ),
        (
            b"GNU.sparse.map".as_slice(),
            sparse
                .data
                .iter()
                .flat_map(|(offset, length)| [offset.to_string(), length.to_string()])
                .collect::<Vec<_>>()
                .join(",")
                .into_bytes(),
        ),
        (b"size".as_slice(), physical.to_string().into_bytes()),
    ] {
        pax.insert(key.to_vec(), value);
    }
}
impl<R: Read> GoTarReader<R> {
    fn sparse_map_1(&mut self, physical: &mut u64, logical: u64) -> io::Result<GoSparse> {
        let mut body = Vec::new();
        let mut needed = 1;
        let mut newlines = 0;
        let mut count = None;
        loop {
            if body.len() >= 1024 * 1024 {
                return Err(sparse_too_long());
            }
            if *physical < 512 {
                let mut last = vec![0; *physical as usize];
                self.reader.read_exact(&mut last)?;
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            let block = self
                .block()?
                .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
            *physical -= 512;
            newlines += block.iter().filter(|b| **b == b'\n').count();
            body.extend_from_slice(&block);
            if newlines < needed {
                continue;
            }
            if count.is_none() {
                let first = body.split(|b| *b == b'\n').next().unwrap_or_default();
                let parsed = go_pax_integer(first)?;
                let parsed = usize::try_from(parsed).map_err(|_| tar_header_invalid())?;
                needed = parsed
                    .checked_mul(2)
                    .and_then(|n| n.checked_add(1))
                    .ok_or_else(tar_header_invalid)?;
                count = Some(parsed);
            }
            if newlines >= needed {
                let tokens = body
                    .split(|b| *b == b'\n')
                    .skip(1)
                    .take(needed - 1)
                    .collect::<Vec<_>>();
                return go_sparse_entries(
                    &tokens,
                    count.expect("parsed sparse count") as i64,
                    logical,
                );
            }
        }
    }
    fn old_sparse(&mut self, block: &[u8; 512]) -> io::Result<GoSparse> {
        if &block[257..263] != b"ustar " || &block[263..265] != b" \0" {
            return Err(tar_header_invalid());
        }
        let size =
            u64::try_from(go_tar_number(&block[483..495])?).map_err(|_| tar_header_invalid())?;
        let mut data = Vec::new();
        let mut fields = block[386..483].to_vec();
        let mut total = fields.len();
        loop {
            if total >= 1024 * 1024 {
                return Err(sparse_too_long());
            }
            for pair in fields[..fields.len() / 24 * 24].chunks_exact(24) {
                if pair[0] == 0 {
                    break;
                }
                let offset = go_tar_number(&pair[..12])?;
                let length = go_tar_number(&pair[12..])?;
                data.push((offset, length));
            }
            if fields[fields.len() - 1] == 0 {
                break;
            }
            let extended = self
                .block()?
                .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
            fields = extended[..505].to_vec();
            total += 512;
        }
        let strings = data
            .iter()
            .flat_map(|(offset, length)| [offset.to_string(), length.to_string()])
            .collect::<Vec<_>>();
        let tokens = strings.iter().map(|s| s.as_bytes()).collect::<Vec<_>>();
        go_sparse_entries(&tokens, data.len() as i64, size)
    }
}
struct GoSparseReader<'a, R> {
    reader: &'a mut R,
    sparse: GoSparse,
    physical: u64,
    position: u64,
    region: usize,
}
impl<R: Read> Read for GoSparseReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.position == self.sparse.size {
            return if self.physical == 0 {
                Ok(0)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "archive/tar: sparse file contains unreferenced data",
                ))
            };
        }
        while self.region < self.sparse.data.len() {
            let (offset, length) = self.sparse.data[self.region];
            if self.position < offset + length {
                break;
            }
            self.region += 1;
        }
        let (offset, length) = self
            .sparse
            .data
            .get(self.region)
            .copied()
            .unwrap_or((self.sparse.size, 0));
        if self.position < offset {
            let count = output
                .len()
                .min(usize::try_from(offset - self.position).unwrap_or(usize::MAX));
            output[..count].fill(0);
            self.position += count as u64;
            return Ok(count);
        }
        if self.physical == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "archive/tar: sparse file references non-existent data",
            ));
        }
        let count = output.len().min(
            usize::try_from((offset + length - self.position).min(self.physical))
                .unwrap_or(usize::MAX),
        );
        let count = self.reader.read(&mut output[..count])?;
        if count == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        self.position += count as u64;
        self.physical -= count as u64;
        Ok(count)
    }
}
fn entry_sparse<R: Read>(entry: &mut tar::Entry<'_, R>) -> io::Result<Option<GoSparse>> {
    let mut pax = BTreeMap::new();
    if let Some(fields) = entry.pax_extensions()? {
        for field in fields {
            let field = field?;
            pax.insert(field.key_bytes().to_vec(), field.value_bytes().to_vec());
        }
    }
    if go_sparse_version(&pax).is_some() {
        go_sparse_pax(&pax, entry.size()).map(Some)
    } else {
        Ok(None)
    }
}

fn tar_header_invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "archive/tar: invalid tar header",
    )
}
fn go_tar_number(field: &[u8]) -> io::Result<i64> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        let negative = field[0] & 0x40 != 0;
        let mut value = 0i128;
        for (index, byte) in field.iter().copied().enumerate() {
            let byte = if index == 0 { byte & 0x7f } else { byte };
            let byte = if negative {
                if index == 0 { byte ^ 0x7f } else { !byte }
            } else {
                byte
            };
            value = value
                .checked_mul(256)
                .and_then(|v| v.checked_add(i128::from(byte)))
                .ok_or_else(tar_header_invalid)?;
        }
        let value = if negative { -1 - value } else { value };
        return i64::try_from(value).map_err(|_| tar_header_invalid());
    }
    go_tar_octal(field)
}
fn go_tar_octal(field: &[u8]) -> io::Result<i64> {
    let start = field
        .iter()
        .position(|b| !matches!(b, b' ' | 0))
        .unwrap_or(field.len());
    let end = field
        .iter()
        .rposition(|b| !matches!(b, b' ' | 0))
        .map_or(start, |n| n + 1);
    let field = &field[start..end];
    if field.is_empty() {
        return Ok(0);
    }
    let field = field.split(|b| *b == 0).next().unwrap_or_default();
    let text = std::str::from_utf8(field).map_err(|_| tar_header_invalid())?;
    u64::from_str_radix(text, 8)
        .map(|value| value as i64)
        .map_err(|_| tar_header_invalid())
}

fn go_tar_name(block: &[u8; 512]) -> Vec<u8> {
    let field = |bytes: &[u8]| bytes.split(|b| *b == 0).next().unwrap_or_default().to_vec();
    let name = field(&block[..100]);
    let mut prefix = Vec::new();
    if &block[257..263] == b"ustar\0" {
        let end = if &block[508..512] == b"tar\0" {
            476
        } else {
            500
        };
        prefix = field(&block[345..end]);
    } else if &block[257..263] == b"ustar "
        && &block[263..265] == b" \0"
        && [&block[345..357], &block[357..369]]
            .into_iter()
            .any(|value| value[0] != 0 && go_tar_number(value).is_err())
    {
        let legacy = field(&block[345..500]);
        if legacy.is_ascii() {
            prefix = legacy;
        }
    }
    if prefix.is_empty() {
        name
    } else {
        [prefix, b"/".to_vec(), name].concat()
    }
}

fn go_pax_integer(value: &[u8]) -> io::Result<i64> {
    std::str::from_utf8(value)
        .ok()
        .and_then(|v| v.parse().ok())
        .ok_or_else(tar_header_invalid)
}
fn parse_go_pax(mut bytes: &[u8]) -> io::Result<BTreeMap<Vec<u8>, Vec<u8>>> {
    let mut map = BTreeMap::new();
    let mut sparse = Vec::new();
    while !bytes.is_empty() {
        let space = bytes
            .iter()
            .position(|b| *b == b' ')
            .ok_or_else(tar_header_invalid)?;
        let length = go_pax_integer(&bytes[..space])?;
        let length = usize::try_from(length).map_err(|_| tar_header_invalid())?;
        if length < 5 || length > bytes.len() || length <= space + 2 || bytes[length - 1] != b'\n' {
            return Err(tar_header_invalid());
        }
        let record = &bytes[space + 1..length - 1];
        let equal = record
            .iter()
            .position(|b| *b == b'=')
            .ok_or_else(tar_header_invalid)?;
        let key = &record[..equal];
        let value = &record[equal + 1..];
        if key.is_empty()
            || key.contains(&0)
            || ((matches!(key, b"path" | b"linkpath" | b"uname" | b"gname")) && value.contains(&0))
        {
            return Err(tar_header_invalid());
        }
        if matches!(key, b"GNU.sparse.offset" | b"GNU.sparse.numbytes") {
            if (sparse.len() % 2 == 0) != (key == b"GNU.sparse.offset") || value.contains(&b',') {
                return Err(tar_header_invalid());
            }
            sparse.push(value.to_vec());
        } else {
            map.insert(key.to_vec(), value.to_vec());
        }
        bytes = &bytes[length..];
    }
    if !sparse.is_empty() {
        map.insert(b"GNU.sparse.map".to_vec(), sparse.join(&b','));
    }
    Ok(map)
}
fn validate_go_pax_fields(pax: &BTreeMap<Vec<u8>, Vec<u8>>) -> io::Result<()> {
    for (key, value) in pax {
        if value.is_empty() {
            continue;
        }
        if matches!(key.as_slice(), b"uid" | b"gid" | b"size") {
            go_pax_integer(value)?;
        }
        if matches!(key.as_slice(), b"atime" | b"mtime" | b"ctime") {
            let dot = value.iter().position(|b| *b == b'.').unwrap_or(value.len());
            go_pax_integer(&value[..dot])?;
            if dot < value.len() && value[dot + 1..].iter().any(|b| !b.is_ascii_digit()) {
                return Err(tar_header_invalid());
            }
        }
    }
    Ok(())
}
fn go_pax_record(key: &[u8], value: &[u8]) -> Vec<u8> {
    let mut length = key.len() + value.len() + 3;
    loop {
        let next = length.to_string().len() + key.len() + value.len() + 3;
        if next == length {
            break;
        }
        length = next;
    }
    [
        length.to_string().into_bytes(),
        b" ".to_vec(),
        key.to_vec(),
        b"=".to_vec(),
        value.to_vec(),
        b"\n".to_vec(),
    ]
    .concat()
}
fn tar_metadata(out: &mut Vec<u8>, kind: u8, body: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.as_mut_bytes()[..100].fill(0);
    header.as_mut_bytes()[..16].copy_from_slice(b"pixiv-metadata\0\0");
    header.set_entry_type(tar::EntryType::new(kind));
    header.set_size(body.len() as u64);
    header.set_cksum();
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(body);
    out.resize(out.len() + (512 - body.len() % 512) % 512, 0);
}
// The TAR library stops at one zero block; Go also validates the following block.
fn validate_tar_tail(mut reader: impl Read) -> Result<(), ExternalError> {
    let mut block = [0u8; 512];
    let mut count = 0;
    while count < block.len() {
        let read = reader.read(&mut block[count..]).map_err(tar_error)?;
        if read == 0 {
            if count == 0 {
                return Ok(());
            }
            return Err(message("read tar release archive: unexpected EOF"));
        }
        count += read;
    }
    if block.iter().any(|byte| *byte != 0) {
        return Err(message(
            "read tar release archive: archive/tar: invalid tar header",
        ));
    }
    Ok(())
}
// The ZIP library indexes by name and discards duplicates, unlike Go's ordered directory.
struct ZipMember {
    name: String,
    kind: u32,
    record: Vec<u8>,
}
fn zip_invalid() -> ExternalError {
    message("open zip release archive: zip: not a valid zip file")
}
fn zip_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}
fn zip_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}
fn zip_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}
fn zip_directory_stop(
    members: Vec<ZipMember>,
    count: u64,
    error: ExternalError,
) -> Result<Vec<ZipMember>, ExternalError> {
    if members.len() as u16 == count as u16 {
        Ok(members)
    } else {
        Err(error)
    }
}
fn zip_record_valid(record: &[u8], name_length: usize, extra_length: usize) -> bool {
    let mut extra = &record[46 + name_length..46 + name_length + extra_length];
    let uncompressed = zip_u32(record, 24).unwrap_or_default() == u32::MAX;
    let compressed = zip_u32(record, 20).unwrap_or_default() == u32::MAX;
    let mut offset = u64::from(zip_u32(record, 42).unwrap_or_default());
    while extra.len() >= 4 {
        let tag = zip_u16(extra, 0).unwrap_or_default();
        let length = usize::from(zip_u16(extra, 2).unwrap_or_default());
        extra = &extra[4..];
        if length > extra.len() {
            break;
        }
        let field = &extra[..length];
        extra = &extra[length..];
        if tag == 1 {
            let mut position = 0;
            for required in [uncompressed, compressed] {
                if required {
                    if zip_u64(field, position).is_none() {
                        return false;
                    }
                    position += 8;
                }
            }
            if offset == u64::from(u32::MAX) {
                let Some(value) = zip_u64(field, position) else {
                    return false;
                };
                offset = value;
            }
        }
    }
    true
}
fn zip_members(bytes: &[u8], base: u64) -> Result<Vec<ZipMember>, ExternalError> {
    let end = (bytes.len().saturating_sub(65557)..bytes.len().saturating_sub(21))
        .rev()
        .find(|at| {
            bytes.get(*at..*at + 4) == Some(b"PK\x05\x06")
                && zip_u16(bytes, *at + 20)
                    .is_some_and(|length| *at + 22 + usize::from(length) <= bytes.len())
        })
        .ok_or_else(zip_invalid)?;
    let mut count = u64::from(zip_u16(bytes, end + 10).ok_or_else(zip_invalid)?);
    let mut offset = u64::from(zip_u32(bytes, end + 16).ok_or_else(zip_invalid)?);
    if count == u64::from(u16::MAX) || offset == u64::from(u32::MAX) {
        let locator = end.checked_sub(20).ok_or_else(zip_invalid)?;
        if bytes.get(locator..locator + 4) != Some(b"PK\x06\x07") {
            return Err(zip_invalid());
        }
        let record = usize::try_from(
            zip_u64(bytes, locator + 8)
                .ok_or_else(zip_invalid)?
                .checked_add(base)
                .ok_or_else(zip_invalid)?,
        )
        .map_err(|_| zip_invalid())?;
        if bytes.get(record..record + 4) != Some(b"PK\x06\x06") {
            return Err(zip_invalid());
        }
        count = zip_u64(bytes, record + 32).ok_or_else(zip_invalid)?;
        offset = zip_u64(bytes, record + 48).ok_or_else(zip_invalid)?;
    }
    let mut at = usize::try_from(offset.checked_add(base).ok_or_else(zip_invalid)?)
        .map_err(|_| zip_invalid())?;
    let mut members = Vec::new();
    loop {
        let available = bytes.len().saturating_sub(at);
        if available == 0 {
            return Err(message("open zip release archive: EOF"));
        }
        if available < 46 {
            return zip_directory_stop(
                members,
                count,
                message("open zip release archive: unexpected EOF"),
            );
        }
        if bytes.get(at..at + 4) != Some(b"PK\x01\x02") {
            return zip_directory_stop(members, count, zip_invalid());
        }
        let name_len = usize::from(zip_u16(bytes, at + 28).ok_or_else(zip_invalid)?);
        let extra_len = usize::from(zip_u16(bytes, at + 30).ok_or_else(zip_invalid)?);
        let comment_len = usize::from(zip_u16(bytes, at + 32).ok_or_else(zip_invalid)?);
        let end = at
            .checked_add(46)
            .and_then(|v| v.checked_add(name_len))
            .and_then(|v| v.checked_add(extra_len))
            .and_then(|v| v.checked_add(comment_len))
            .ok_or_else(zip_invalid)?;
        let Some(record) = bytes.get(at..end) else {
            if available == 46 {
                return Err(message("open zip release archive: EOF"));
            }
            return zip_directory_stop(
                members,
                count,
                message("open zip release archive: unexpected EOF"),
            );
        };
        let record = record.to_vec();
        if !zip_record_valid(&record, name_len, extra_len) {
            return zip_directory_stop(members, count, zip_invalid());
        }
        let name = String::from_utf8_lossy(&record[46..46 + name_len]).into_owned();
        let creator = record[5];
        let attributes = zip_u32(&record, 38).ok_or_else(zip_invalid)?;
        let mut kind = if matches!(creator, 3 | 19) {
            match (attributes >> 16) & 0o170000 {
                kind @ (0o010000 | 0o020000 | 0o040000 | 0o060000 | 0o100000 | 0o120000
                | 0o140000) => kind,
                _ => 0,
            }
        } else if matches!(creator, 0 | 11 | 14) && attributes & 0x10 != 0 {
            0o040000
        } else {
            0
        };
        if name.ends_with('/') && kind != 0o120000 {
            kind = 0o040000;
        }
        members.push(ZipMember { name, kind, record });
        at = end;
    }
}
fn single_zip_member(
    bytes: &[u8],
    member: &ZipMember,
    base: u64,
) -> Result<Vec<u8>, ExternalError> {
    let record = member.record.clone();
    let base = usize::try_from(base).map_err(|_| zip_invalid())?;
    let mut archive = bytes.get(base..).ok_or_else(zip_invalid)?.to_vec();
    let directory = archive.len() as u64;
    let size = record.len() as u64;
    archive.extend_from_slice(&record);
    let zip64 = directory > u64::from(u32::MAX) || size > u64::from(u32::MAX);
    if zip64 {
        let zip64_offset = archive.len() as u64;
        archive.extend_from_slice(b"PK\x06\x06");
        archive.extend_from_slice(&44u64.to_le_bytes());
        archive.extend_from_slice(&45u16.to_le_bytes());
        archive.extend_from_slice(&45u16.to_le_bytes());
        archive.extend_from_slice(&[0; 8]);
        archive.extend_from_slice(&1u64.to_le_bytes());
        archive.extend_from_slice(&1u64.to_le_bytes());
        archive.extend_from_slice(&size.to_le_bytes());
        archive.extend_from_slice(&directory.to_le_bytes());
        archive.extend_from_slice(b"PK\x06\x07");
        archive.extend_from_slice(&0u32.to_le_bytes());
        archive.extend_from_slice(&zip64_offset.to_le_bytes());
        archive.extend_from_slice(&1u32.to_le_bytes());
    }
    archive.extend_from_slice(b"PK\x05\x06");
    archive.extend_from_slice(&[0; 4]);
    archive.extend_from_slice(&1u16.to_le_bytes());
    archive.extend_from_slice(&1u16.to_le_bytes());
    archive.extend_from_slice(&if zip64 { u32::MAX } else { size as u32 }.to_le_bytes());
    archive.extend_from_slice(&if zip64 { u32::MAX } else { directory as u32 }.to_le_bytes());
    archive.extend_from_slice(&0u16.to_le_bytes());
    Ok(archive)
}
fn extract_binary(
    bytes: &[u8],
    archive_name: &str,
    destination: &Path,
    binary_name: &str,
    fs: &dyn InstallFileSystem,
) -> Result<(), ExternalError> {
    if archive_name.ends_with(".tar.gz") {
        let mut prescan = tar::Archive::new(GoTarReader::new(gzip(bytes)?));
        for entry in prescan.entries().map_err(tar_error)? {
            let entry = entry.map_err(tar_error)?;
            validate_path(&String::from_utf8_lossy(&entry.path_bytes()))?;
        }
        validate_tar_tail(prescan.into_inner())?;
        let mut archive = tar::Archive::new(GoTarReader::new(gzip(bytes)?));
        let mut found = false;
        for entry in archive.entries().map_err(tar_error)? {
            let mut entry = entry.map_err(tar_error)?;
            let name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
            let kind = entry.header().entry_type();
            if kind.is_symlink() || kind.is_hard_link() {
                return Err(message(format!(
                    "release archive contains link entry {}",
                    go_quote(&name)
                )));
            }
            if name.trim_end_matches('/').rsplit('/').next() != Some(binary_name) {
                continue;
            }
            if !kind.is_file() {
                return Err(message(format!(
                    "release archive binary {} is not a regular file",
                    go_quote(&name)
                )));
            }
            if found {
                return Err(message(format!(
                    "release archive contains duplicate binary {}",
                    go_quote(binary_name)
                )));
            }
            if let Some(sparse) = entry_sparse(&mut entry).map_err(tar_error)? {
                let physical = entry.size();
                let mut reader = GoSparseReader {
                    reader: &mut entry,
                    sparse,
                    physical,
                    position: 0,
                    region: 0,
                };
                write_binary(destination, &mut reader, fs, false)?;
            } else {
                write_binary(destination, &mut entry, fs, false)?;
            }
            found = true;
        }
        validate_tar_tail(archive.into_inner())?;
        if !found {
            return Err(message(format!(
                "release archive has no regular binary {}",
                go_quote(binary_name)
            )));
        }
        return Ok(());
    }
    if archive_name.ends_with(".zip") {
        let archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| zip_invalid())?;
        let base = archive.offset();
        let members = zip_members(bytes, base)?;
        for member in &members {
            validate_path(&member.name)?;
        }
        let mut found = false;
        for member in members {
            if member.kind == 0o120000 {
                return Err(message(format!(
                    "release archive contains link entry {}",
                    go_quote(&member.name)
                )));
            }
            if member.name.trim_end_matches('/').rsplit('/').next() != Some(binary_name) {
                continue;
            }
            if member.kind != 0 && member.kind != 0o100000 {
                return Err(message(format!(
                    "release archive binary {} is not a regular file",
                    go_quote(&member.name)
                )));
            }
            if found {
                return Err(message(format!(
                    "release archive contains duplicate binary {}",
                    go_quote(binary_name)
                )));
            }
            let bytes = single_zip_member(bytes, &member, base)?;
            let mut archive =
                zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| zip_invalid())?;
            let mut entry = archive.by_index(0).map_err(|e| {
                wrap(
                    format!("open release archive binary {}", go_quote(&member.name)),
                    Box::new(e),
                )
            })?;
            write_binary(destination, &mut entry, fs, true)?;
            found = true;
        }
        if !found {
            return Err(message(format!(
                "release archive has no regular binary {}",
                go_quote(binary_name)
            )));
        }
        return Ok(());
    }

    Err(message(format!(
        "unsupported release archive {}",
        go_quote(archive_name)
    )))
}
#[derive(Debug)]
pub struct ReplacementSourcePreservationError {
    cause: ExternalError,
}
impl ReplacementSourcePreservationError {
    pub fn new(cause: ExternalError) -> Self {
        Self { cause }
    }
}
impl fmt::Display for ReplacementSourcePreservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.cause.fmt(f)
    }
}
impl Error for ReplacementSourcePreservationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}
pub fn must_preserve_replacement_source(error: &(dyn Error + 'static)) -> bool {
    if error.is::<ReplacementSourcePreservationError>() {
        return true;
    }
    if let Some(joined) = error.downcast_ref::<super::JoinedError>()
        && joined
            .0
            .iter()
            .any(|cause| must_preserve_replacement_source(cause.as_ref()))
    {
        return true;
    }
    error.source().is_some_and(must_preserve_replacement_source)
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReplacementResult {
    pub committed: bool,
    pub preserve_source: bool,
}
pub trait WindowsReplacementApi: Send + Sync {
    fn utf16(&self, path: &Path) -> Result<Vec<u16>, ExternalError>;
    fn find_replace(&self) -> Result<(), ExternalError>;
    fn move_file(&self, source: &[u16], target: &[u16], flags: u32) -> Result<(), ExternalError>;
    fn replace_file(
        &self,
        target: &[u16],
        source: &[u16],
        backup: &[u16],
        flags: u32,
        reserved_a: usize,
        reserved_b: usize,
    ) -> Result<(), ExternalError>;
}
pub struct WindowsFileReplacer {
    api: Arc<dyn WindowsReplacementApi>,
}
impl WindowsFileReplacer {
    pub fn new(api: Arc<dyn WindowsReplacementApi>) -> Self {
        Self { api }
    }
    fn attempt(
        &self,
        source: &Path,
        target: &Path,
        backup: &Path,
    ) -> Result<(ReplacementResult, bool), ExternalError> {
        let from = self.api.utf16(source)?;
        let to = self.api.utf16(target)?;
        match fs::symlink_metadata(target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.api.move_file(&from, &to, 0)?;
                return Ok((
                    ReplacementResult {
                        committed: true,
                        preserve_source: false,
                    },
                    false,
                ));
            }
            Err(error) => return Err(file_error(format!("lstat {}", target.display()), error)),
            Ok(_) => {}
        }
        self.api.find_replace()?;
        let old = self.api.utf16(backup)?;
        match self.api.replace_file(&to, &from, &old, 0, 0, 0) {
            Ok(()) => Ok((
                ReplacementResult {
                    committed: true,
                    preserve_source: false,
                },
                true,
            )),
            Err(error) => {
                if !has_windows_code(error.as_ref(), 1177) {
                    return Err(error);
                }
                let restored = (|| {
                    let old = self.api.utf16(backup)?;
                    let target = self.api.utf16(target)?;
                    self.api.move_file(&old, &target, 0)
                })();
                match restored {
                    Ok(()) => Err(error),
                    Err(restore) => Err(Box::new(ReplacementSourcePreservationError::new(join(
                        vec![error, wrap("restore replaced target from backup", restore)],
                    )))),
                }
            }
        }
    }
    pub fn replace_retained(
        &self,
        source: &Path,
        target: &Path,
        backup: &Path,
    ) -> Result<(), ExternalError> {
        if backup.as_os_str().is_empty() {
            return self.replace_disposable(source, target).map(|_| ());
        }
        self.attempt(source, target, backup).map(|_| ())
    }
    pub fn replace_private(
        &self,
        source: &Path,
        target: &Path,
    ) -> (ReplacementResult, Option<ExternalError>) {
        let backup = PathBuf::from(format!("{}.recovery", source.display()));
        match self.attempt(source, target, &backup) {
            Ok((result, created)) => {
                let error = if result.committed && created {
                    NativeInstallFileSystem
                        .remove(&backup)
                        .err()
                        .map(|e| wrap("remove committed replacement backup", e))
                } else {
                    None
                };
                (result, error)
            }
            Err(error) => {
                let result = ReplacementResult {
                    committed: false,
                    preserve_source: must_preserve_replacement_source(error.as_ref()),
                };
                (result, Some(error))
            }
        }
    }
    pub fn replace_disposable(
        &self,
        source: &Path,
        target: &Path,
    ) -> Result<ReplacementResult, ExternalError> {
        let (result, error) = self.replace_private(source, target);
        match error {
            Some(error) => Err(error),
            None => Ok(result),
        }
    }
}
fn has_windows_code(error: &(dyn Error + 'static), code: i32) -> bool {
    error
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.raw_os_error() == Some(code))
        || error
            .source()
            .is_some_and(|cause| has_windows_code(cause, code))
}
#[cfg(windows)]
type ReplaceProcedure = unsafe extern "system" fn(
    *const u16,
    *const u16,
    *const u16,
    u32,
    *const std::ffi::c_void,
    *const std::ffi::c_void,
) -> i32;
#[cfg(windows)]
type MoveProcedure = unsafe extern "system" fn(*const u16, *const u16, u32) -> i32;
#[cfg(windows)]
#[derive(Default)]
pub struct NativeWindowsReplacementApi {
    replace: std::sync::Mutex<Option<ReplaceProcedure>>,
}
#[cfg(windows)]
fn system_procedure(name: &[u8]) -> Result<unsafe extern "system" fn() -> isize, ExternalError> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    let kernel = "kernel32.dll"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let module = unsafe { GetModuleHandleW(kernel.as_ptr()) };
    if module.is_null() {
        return Err(Box::new(io::Error::last_os_error()));
    }
    unsafe { GetProcAddress(module, name.as_ptr()) }
        .ok_or_else(|| Box::new(io::Error::last_os_error()) as ExternalError)
}
#[cfg(windows)]
impl WindowsReplacementApi for NativeWindowsReplacementApi {
    fn utf16(&self, path: &Path) -> Result<Vec<u16>, ExternalError> {
        use std::os::windows::ffi::OsStrExt;
        let mut value = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if value.contains(&0) {
            return Err(Box::new(io::Error::from(io::ErrorKind::InvalidInput)));
        }
        value.push(0);
        Ok(value)
    }
    fn find_replace(&self) -> Result<(), ExternalError> {
        let mut procedure = self
            .replace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if procedure.is_none() {
            *procedure = Some(unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, ReplaceProcedure>(
                    system_procedure(b"ReplaceFileW\0")?,
                )
            });
        }
        Ok(())
    }
    fn move_file(&self, source: &[u16], target: &[u16], flags: u32) -> Result<(), ExternalError> {
        let procedure = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, MoveProcedure>(
                system_procedure(b"MoveFileExW\0")?,
            )
        };
        if unsafe { procedure(source.as_ptr(), target.as_ptr(), flags) } == 0 {
            Err(Box::new(io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
    fn replace_file(
        &self,
        target: &[u16],
        source: &[u16],
        backup: &[u16],
        flags: u32,
        reserved_a: usize,
        reserved_b: usize,
    ) -> Result<(), ExternalError> {
        self.find_replace()?;
        let procedure = self
            .replace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .expect("resolved system procedure");
        if unsafe {
            procedure(
                target.as_ptr(),
                source.as_ptr(),
                backup.as_ptr(),
                flags,
                reserved_a as *const _,
                reserved_b as *const _,
            )
        } == 0
        {
            Err(Box::new(io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
}
pub struct NativeReleaseFileReplacer {
    retain_old: bool,
}
impl NativeReleaseFileReplacer {
    pub fn new(goos: &str) -> Self {
        Self {
            retain_old: goos == "windows",
        }
    }
}
impl FileReplacer for NativeReleaseFileReplacer {
    fn replace(&self, source: &Path, target: &Path) -> Result<(), ExternalError> {
        #[cfg(windows)]
        {
            let windows =
                WindowsFileReplacer::new(Arc::new(NativeWindowsReplacementApi::default()));
            if self.retain_old {
                windows.replace_retained(
                    source,
                    target,
                    &PathBuf::from(format!("{}.old", target.display())),
                )
            } else {
                windows.replace_disposable(source, target).map(|_| ())
            }
        }
        #[cfg(not(windows))]
        {
            let _ = self.retain_old;
            NativeInstallFileSystem.rename(source, target)
        }
    }
}
#[derive(Default)]
pub struct ProcessReleaseBinaryChecker;
#[derive(Debug)]
pub struct ProcessExitError {
    pub status: ExitStatus,
    pub stderr: Vec<u8>,
}
impl ProcessExitError {
    pub fn exit_code(&self) -> i32 {
        self.status.code().unwrap_or(-1)
    }
    pub fn process_state(&self) -> String {
        if let Some(code) = self.status.code() {
            return format!("exit status {code}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if self.status.signal() == Some(libc::SIGKILL) {
                return "signal: killed".into();
            }
        }
        self.status.to_string()
    }
}
impl fmt::Display for ProcessExitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.process_state())
    }
}
impl Error for ProcessExitError {}
struct KillOnDrop(CancellationToken);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
fn retained_stderr(bytes: Vec<u8>) -> Vec<u8> {
    if bytes.len() <= 65536 {
        return bytes;
    }
    let mut retained = bytes[..32768].to_vec();
    retained.extend_from_slice(
        format!("\n... omitting {} bytes ...\n", bytes.len() - 65536).as_bytes(),
    );
    retained.extend_from_slice(&bytes[bytes.len() - 32768..]);
    retained
}
impl BinaryChecker for ProcessReleaseBinaryChecker {
    fn check(
        &self,
        context: CallerContext,
        path: PathBuf,
        expected_tag: String,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            check_context(&context, "run --version")?;
            let stop = CancellationToken::new();
            let guard = KillOnDrop(stop.clone());
            let worker = tokio::spawn(async move {
                check_context(&context, "run --version")?;
                let mut child = tokio::process::Command::new(&path)
                    .arg("--version")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|e| {
                        wrap(
                            "run --version",
                            file_error(format!("fork/exec {}", path.display()), e),
                        )
                    })?;
                let mut stdout = child.stdout.take().expect("piped stdout");
                let mut stderr = child.stderr.take().expect("piped stderr");
                let drain = async {
                    let mut out = Vec::new();
                    let mut err = Vec::new();
                    let (output, error) =
                        tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
                    output?;
                    error?;
                    Ok::<_, io::Error>((out, err))
                };
                let wait = async {
                    tokio::select! {biased;status=child.wait()=>status,_=context.cancelled()=>{let _=child.start_kill();child.wait().await},_=stop.cancelled()=>{let _=child.start_kill();child.wait().await}}
                };
                let (status, output) = tokio::join!(wait, drain);
                let status = status.map_err(|e| wrap("run --version", Box::new(e)))?;
                let (output, stderr) = output.map_err(|e| wrap("run --version", Box::new(e)))?;
                if !status.success() {
                    return Err(wrap(
                        "run --version",
                        Box::new(ProcessExitError {
                            status,
                            stderr: retained_stderr(stderr),
                        }),
                    ));
                }
                let want = format!("pixiv {expected_tag}\n");
                if output != want.as_bytes() {
                    return Err(message(format!(
                        "downloaded executable reports version output {}, want {}",
                        go_quote_bytes(&output),
                        go_quote(&want)
                    )));
                }
                Ok(())
            });
            let result = worker
                .await
                .map_err(|e| message(format!("wait for downloaded executable: {e}")))?;
            drop(guard);
            result
        })
    }
}
pub struct FileReleaseCache {
    directory: PathBuf,
    path: PathBuf,
    fs: Arc<dyn InstallFileSystem>,
    replacer: Arc<dyn FileReplacer>,
}
impl FileReleaseCache {
    pub fn new(directory: impl Into<PathBuf>, path: impl Into<PathBuf>) -> Self {
        Self::with_ports(
            directory,
            path,
            Arc::new(NativeInstallFileSystem),
            Arc::new(NativeReleaseFileReplacer::new("")),
        )
    }
    pub fn with_ports(
        directory: impl Into<PathBuf>,
        path: impl Into<PathBuf>,
        fs: Arc<dyn InstallFileSystem>,
        replacer: Arc<dyn FileReplacer>,
    ) -> Self {
        Self {
            directory: directory.into(),
            path: path.into(),
            fs,
            replacer,
        }
    }
}
impl ReleaseCache for FileReleaseCache {
    fn read(
        &self,
        context: CallerContext,
    ) -> UpdateFuture<'_, Result<Option<Vec<u8>>, ExternalError>> {
        Box::pin(async move {
            check_context(&context, "read GitHub Releases cache")?;
            match self.fs.read(&self.path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if is_not_found(error.as_ref()) => Ok(None),
                Err(error) => Err(wrap(
                    format!("read GitHub Releases cache {}", path_quote(&self.path)),
                    error,
                )),
            }
        })
    }
    fn write(
        &self,
        context: CallerContext,
        data: Vec<u8>,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            check_context(&context, "write GitHub Releases cache")?;
            self.fs
                .create_dir_all(&self.directory, 0o700)
                .map_err(|e| {
                    wrap(
                        format!(
                            "create GitHub Releases cache directory {}",
                            path_quote(&self.directory)
                        ),
                        e,
                    )
                })?;
            #[cfg(unix)]
            self.fs
                .set_permissions(&self.directory, 0o700)
                .map_err(|e| {
                    wrap(
                        format!(
                            "set GitHub Releases cache directory permissions to 0700 {}",
                            path_quote(&self.directory)
                        ),
                        e,
                    )
                })?;
            let (path, mut file) = self
                .fs
                .create_temp_file(&self.directory, ".github-releases-")
                .map_err(|e| {
                    wrap(
                        format!(
                            "create temporary GitHub Releases cache in {}",
                            path_quote(&self.directory)
                        ),
                        e,
                    )
                })?;
            let mut result = (|| {
                file.set_permissions(0o600).map_err(|e| {
                    wrap(
                        format!(
                            "set temporary GitHub Releases cache permissions {}",
                            path_quote(&path)
                        ),
                        e,
                    )
                })?;
                file.write_all(&data).map_err(|e| {
                    wrap(
                        format!("encode GitHub Releases cache {}", path_quote(&path)),
                        e,
                    )
                })?;
                file.sync().map_err(|e| {
                    wrap(
                        format!("sync temporary GitHub Releases cache {}", path_quote(&path)),
                        e,
                    )
                })?;
                file.close().map_err(|e| {
                    wrap(
                        format!(
                            "close temporary GitHub Releases cache {}",
                            path_quote(&path)
                        ),
                        e,
                    )
                })?;
                self.replacer.replace(&path, &self.path).map_err(|e| {
                    wrap(
                        format!(
                            "atomically replace GitHub Releases cache {}",
                            path_quote(&self.path)
                        ),
                        e,
                    )
                })
            })();
            let _ = file.close();
            if !result
                .as_ref()
                .err()
                .is_some_and(|e| must_preserve_replacement_source(e.as_ref()))
                && let Err(error) = self.fs.remove(&path)
                && result.is_ok()
                && !is_not_found(error.as_ref())
            {
                result = Err(wrap(
                    format!(
                        "remove temporary GitHub Releases cache {}",
                        path_quote(&path)
                    ),
                    error,
                ));
            }
            result
        })
    }
}
pub trait SourceDetectionEnvironment: Send + Sync {
    fn executable(&self) -> Result<PathBuf, ExternalError>;
    fn eval_symlinks(&self, path: &Path) -> Result<PathBuf, ExternalError>;
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ExternalError>;
    fn build_main(&self) -> Option<String>;
    fn getenv(&self, key: &str) -> String;
    fn default_gopath(&self) -> String;
}
#[derive(Default)]
pub struct NativeSourceDetectionEnvironment;
impl SourceDetectionEnvironment for NativeSourceDetectionEnvironment {
    fn executable(&self) -> Result<PathBuf, ExternalError> {
        NativeExecutableLocator.executable()
    }
    fn eval_symlinks(&self, path: &Path) -> Result<PathBuf, ExternalError> {
        fs::canonicalize(path).map_err(|e| file_error(format!("lstat {}", path.display()), e))
    }
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ExternalError> {
        fs::read(path).map_err(|e| file_error(format!("open {}", path.display()), e))
    }
    fn build_main(&self) -> Option<String> {
        // Rust executables do not contain Go's debug.ReadBuildInfo main-module record.
        None
    }
    fn getenv(&self, key: &str) -> String {
        std::env::var(key).unwrap_or_default()
    }
    fn default_gopath(&self) -> String {
        let home =
            std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
        if home.is_empty() {
            String::new()
        } else {
            Path::new(&home).join("go").to_string_lossy().into_owned()
        }
    }
}
pub struct NativeSourceDetector {
    goos: String,
    environment: Arc<dyn SourceDetectionEnvironment>,
}
impl Default for NativeSourceDetector {
    fn default() -> Self {
        Self::new(native_goos(), Arc::new(NativeSourceDetectionEnvironment))
    }
}
impl NativeSourceDetector {
    pub fn new(goos: impl Into<String>, environment: Arc<dyn SourceDetectionEnvironment>) -> Self {
        Self {
            goos: goos.into(),
            environment,
        }
    }
}
impl SourceDetector for NativeSourceDetector {
    fn detect(&self, info: &BuildInfo) -> Result<InstallSource, ExternalError> {
        if info.is_development() {
            return Ok(InstallSource::Development);
        }
        let raw = self
            .environment
            .executable()
            .map_err(|e| wrap("determine executable path", e))?;
        let actual = self.environment.eval_symlinks(&raw).map_err(|e| {
            wrap(
                format!("resolve executable symlink {}", path_quote(&raw)),
                e,
            )
        })?;
        if let Some((formula, keg)) = homebrew_keg(&actual, binary_name(&self.goos)) {
            let receipt_path = keg.join("INSTALL_RECEIPT.json");
            let receipt = self.environment.read_file(&receipt_path).map_err(|e| {
                wrap(
                    format!(
                        "read Homebrew receipt {} for executable {}",
                        path_quote(&receipt_path),
                        path_quote(&actual)
                    ),
                    e,
                )
            })?;
            let parsed: serde_json::Value = serde_json::from_slice(&receipt).map_err(|e| {
                wrap(
                    format!(
                        "parse Homebrew receipt {} for executable {}",
                        path_quote(&receipt_path),
                        path_quote(&actual)
                    ),
                    message(if e.is_eof() {
                        "unexpected end of JSON input".into()
                    } else {
                        json_message(&e)
                    }),
                )
            })?;
            let source_path = parsed["source"]["path"].as_str().unwrap_or("").trim();
            let receipt_formula = (|| {
                if source_path.is_empty() {
                    return Err(message("missing source.path"));
                }
                let file = source_path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("");
                if !file.ends_with(".rb") {
                    return Err(message(format!(
                        "source.path {} does not name a formula file",
                        go_quote(source_path)
                    )));
                }
                let name = file
                    .strip_suffix(".rb")
                    .expect("validated formula extension");
                if name.is_empty() {
                    return Err(message(format!(
                        "source.path {} has an empty formula name",
                        go_quote(source_path)
                    )));
                }
                Ok(name)
            })()
            .map_err(|e| {
                wrap(
                    format!(
                        "validate Homebrew receipt {} for keg {}",
                        path_quote(&receipt_path),
                        path_quote(&keg)
                    ),
                    e,
                )
            })?;
            if receipt_formula != formula {
                return Err(message(format!(
                    "Homebrew receipt {} formula {} does not match keg formula {}",
                    path_quote(&receipt_path),
                    go_quote(receipt_formula),
                    go_quote(&formula)
                )));
            }
            return match formula.as_str() {
                "pixiv-cli" => Ok(InstallSource::HomebrewStable),
                "pixiv-cli-beta" => Ok(InstallSource::HomebrewBeta),
                _ => Err(message(format!(
                    "unsupported Homebrew formula {} in keg {} for executable {}",
                    go_quote(&formula),
                    path_quote(&keg),
                    path_quote(&actual)
                ))),
            };
        }
        if self.environment.build_main().as_deref() != Some("github.com/FlanChanXwO/pixiv-cli") {
            return Ok(InstallSource::Release);
        }
        let mut bin = self.environment.getenv("GOBIN");
        if bin.is_empty() {
            let mut gopath = self.environment.getenv("GOPATH");
            if gopath.is_empty() {
                gopath = self.environment.default_gopath();
            }
            let first = gopath
                .split(if cfg!(windows) { ';' } else { ':' })
                .next()
                .unwrap_or("");
            if first.is_empty() {
                return Ok(InstallSource::Release);
            }
            bin = Path::new(first).join("bin").to_string_lossy().into_owned();
        }
        let expected = Path::new(&bin).join(binary_name(&self.goos));
        let resolved = match self.environment.eval_symlinks(&expected) {
            Ok(path) => path,
            Err(error) if is_not_found(error.as_ref()) => return Ok(InstallSource::Release),
            Err(error) => {
                return Err(wrap(
                    format!(
                        "resolve Go install executable symlink {}",
                        path_quote(&expected)
                    ),
                    error,
                ));
            }
        };
        let actual = actual.to_string_lossy();
        let resolved = resolved.to_string_lossy();
        if if self.goos == "windows" {
            actual.to_lowercase() == resolved.to_lowercase()
        } else {
            actual == resolved
        } {
            Ok(InstallSource::GoInstall)
        } else {
            Ok(InstallSource::Release)
        }
    }
}
fn homebrew_keg(path: &Path, binary: &str) -> Option<(String, PathBuf)> {
    if path.file_name()?.to_str()? != binary {
        return None;
    }
    let bin = path.parent()?;
    if bin.file_name()?.to_str()? != "bin" {
        return None;
    }
    let keg = bin.parent()?;
    let formula = keg.parent()?;
    if formula.parent()?.file_name()?.to_str()? != "Cellar" {
        return None;
    }
    Some((
        formula.file_name()?.to_string_lossy().into_owned(),
        keg.into(),
    ))
}
