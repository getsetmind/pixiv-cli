mod file_replace;
mod native_encoder;
mod ugoira;
mod zip_directory;
pub use native_encoder::NativeAnimationEncoder;
mod static_artwork;
mod static_filename;

use crate::{
    auth_bundle::go_quote, diagnostics::Event, lifecycle::Context, scheduler::SchedulerError,
};
use pixiv_sdk::{
    Client,
    models::{Artwork, UgoiraFrame, UgoiraMetadata},
    oauth::LoginUrl,
    reference::{
        REFERENCE_KIND_ARTWORK, REFERENCE_KIND_USER, REFERENCE_KIND_USER_BOOKMARKS, parse_url,
    },
    resource::{ResourceRef, SaveOptions, SavedResource},
    transport::HttpTransport,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Instant,
};

#[derive(Clone, Debug, Default)]
pub struct DownloadRequest {
    pub download_path: String,
    pub filename_template: String,
    pub directory_template: String,
    pub pages: Vec<i64>,
    pub quality: String,
    pub ugoira_format: String,
}
#[derive(Clone, Debug, Default)]
pub struct DownloadedFile {
    pub path: PathBuf,
    pub page: i64,
    pub bytes: i64,
}
#[derive(Clone, Debug, Default)]
pub struct DownloadedItem {
    pub illust_id: i64,
    pub title: String,
    pub author: String,
    pub kind: String,
    pub files: Vec<DownloadedFile>,
    pub quality: String,
    pub frames: Vec<UgoiraFrame>,
    pub frame_report: Option<UgoiraFrameReport>,
}
#[derive(Clone, Debug)]
pub struct DownloadFailure {
    pub url: String,
    pub illust_id: i64,
    pub kind: String,
    pub message: String,
    pub code: String,
    pub path: PathBuf,
    pub missing: Vec<String>,
    pub cause: Arc<SchedulerError>,
}
#[derive(Clone, Debug, Default)]
pub struct DownloadWarning {
    pub illust_id: i64,
    pub kind: String,
    pub message: String,
}
#[derive(Clone, Debug, Default)]
pub struct DownloadReport {
    pub items: Vec<DownloadedItem>,
    pub failures: Vec<DownloadFailure>,
    pub warnings: Vec<DownloadWarning>,
    pub committed: bool,
}
#[derive(Debug, Default)]
pub struct DownloadAttempt {
    pub report: DownloadReport,
    pub error: Option<SchedulerError>,
}

#[derive(Clone, Debug, Default)]
pub struct UgoiraFrameReport {
    pub declared: usize,
    pub actual: usize,
    pub undeclared: Option<Vec<String>>,
    pub missing: Option<Vec<String>>,
}
#[derive(Clone, Debug)]
pub struct EncoderInput {
    pub zip_path: PathBuf,
    pub output_path: PathBuf,
    pub work_dir: PathBuf,
    pub frames: Option<Vec<UgoiraFrame>>,
    pub format: String,
    pub max_edge: u32,
}
pub type EncoderFuture<'a> = Pin<Box<dyn Future<Output = Result<(), SchedulerError>> + Send + 'a>>;
pub trait AnimationEncoder: Send + Sync {
    fn encode(&self, context: Context, input: EncoderInput) -> EncoderFuture<'_>;
}
pub type UgoiraMetadataFuture<'a> =
    Pin<Box<dyn Future<Output = Result<UgoiraMetadata, SchedulerError>> + Send + 'a>>;

pub type SaveFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + 'a>>;
pub type ArtworkFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Artwork, SchedulerError>> + Send + 'a>>;
pub trait DownloadSaveClient: Send + Sync {
    fn artwork(&self, _context: Context, _id: i64) -> Option<ArtworkFuture<'_>> {
        None
    }
    fn ugoira_metadata(&self, _context: Context, _id: i64) -> Option<UgoiraMetadataFuture<'_>> {
        None
    }
    fn supports_direct_urls(&self) -> bool {
        true
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_>;
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_>;
}

pub struct NativeDownloadSaveClient {
    client: Arc<Client<HttpTransport>>,
}
impl NativeDownloadSaveClient {
    pub fn new(client: Arc<Client<HttpTransport>>) -> Self {
        Self { client }
    }
}
impl DownloadSaveClient for NativeDownloadSaveClient {
    fn ugoira_metadata(&self, context: Context, id: i64) -> Option<UgoiraMetadataFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {
                biased;
                error = context.cancelled() => Err(error.into()),
                result = self.client.ugoira_metadata(id) => result.map_err(Into::into),
            }
        }))
    }
    fn artwork(&self, context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(async move {
            tokio::select! {
                biased;
                error = context.cancelled() => Err(error.into()),
                result = self.client.artwork(id) => result.map_err(Into::into),
            }
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {
                biased;
                error = context.cancelled() => Err(error.into()),
                result = self.client.save_resource(reference, SaveOptions { path: path.to_string_lossy().into_owned(), progress: None }) => result.map_err(Into::into),
            }
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {
                biased;
                error = context.cancelled() => Err(error.into()),
                result = self.client.save_resource_url(&url, SaveOptions { path: path.to_string_lossy().into_owned(), progress: None }) => result.map_err(Into::into),
            }
        })
    }
}

const REDACTED_SOURCE: &str = "[redacted source]";
fn message(value: impl Into<String>) -> SchedulerError {
    SchedulerError::Message(value.into())
}
fn failure(url: String, kind: &str, error: SchedulerError) -> DownloadFailure {
    DownloadFailure {
        url,
        illust_id: 0,
        kind: kind.into(),
        message: error.to_string(),
        code: String::new(),
        path: PathBuf::new(),
        missing: vec![],
        cause: Arc::new(error),
    }
}

pub async fn download_sources(
    context: &Context,
    client: &(impl DownloadSaveClient + ?Sized),
    sources: &[String],
    request: &DownloadRequest,
) -> DownloadAttempt {
    download_sources_with_encoder(context, client, sources, request, &NativeAnimationEncoder).await
}

pub async fn download_sources_with_encoder(
    context: &Context,
    client: &(impl DownloadSaveClient + ?Sized),
    sources: &[String],
    request: &DownloadRequest,
    encoder: &dyn AnimationEncoder,
) -> DownloadAttempt {
    let started = Instant::now();
    context.emit(Event {
        module: "Pixiv download".into(),
        kind: "started".into(),
        operation: format!("download {} sources", sources.len()),
        ..Default::default()
    });
    let attempt = execute_download_sources(context, client, sources, request, encoder).await;
    context.emit(Event {
        module: "Pixiv download".into(),
        kind: if attempt.error.is_some() {
            "failed"
        } else {
            "completed"
        }
        .into(),
        operation: "download sources".into(),
        reason: if attempt.error.is_some() {
            "command failed"
        } else {
            ""
        }
        .into(),
        count: attempt.report.items.len() as i64,
        duration_ns: started.elapsed().as_nanos().min(i64::MAX as u128) as i64,
        ..Default::default()
    });
    attempt
}

async fn execute_download_sources(
    context: &Context,
    client: &(impl DownloadSaveClient + ?Sized),
    sources: &[String],
    request: &DownloadRequest,
    encoder: &dyn AnimationEncoder,
) -> DownloadAttempt {
    let mut attempt = DownloadAttempt::default();
    if sources.is_empty() {
        attempt.error = Some(message("at least one download source is required"));
        return attempt;
    }
    let mut refs = vec![];
    let mut urls = vec![];
    let mut artwork_ids = BTreeSet::new();
    for source in sources {
        if let Some(error) = context.error() {
            attempt.error = Some(error.into());
            return attempt;
        }
        if let Ok(reference) = parse_url(source) {
            match reference.kind.as_str() {
                REFERENCE_KIND_ARTWORK => {
                    artwork_ids.insert(reference.id);
                    continue;
                }
                REFERENCE_KIND_USER | REFERENCE_KIND_USER_BOOKMARKS => {
                    attempt.error = Some(message("user artwork expansion is not yet supported"));
                    return attempt;
                }
                _ => {
                    attempt.error = Some(message("download source is invalid"));
                    return attempt;
                }
            }
        }
        if let Ok(id) = source.trim().parse::<i64>()
            && id > 0
        {
            artwork_ids.insert(id);
            continue;
        }
        let trimmed = source.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            urls.push(trimmed.to_owned());
            continue;
        }
        match ResourceRef::parse(source) {
            Ok(reference) => refs.push(reference),
            Err(error) => {
                let mut rejected = failure(REDACTED_SOURCE.into(), "", error.into());
                rejected.message = "download source is invalid".into();
                attempt.report.failures.push(rejected);
            }
        }
    }
    if !artwork_ids.is_empty() {
        let batch = static_artwork::download_batch(
            context,
            client,
            artwork_ids.into_iter().collect(),
            request,
            encoder,
        )
        .await;
        attempt.report.items.extend(batch.report.items);
        attempt.report.failures.extend(batch.report.failures);
        attempt.report.warnings.extend(batch.report.warnings);
        attempt.report.committed |= batch.report.committed;
        if batch.error.is_some() {
            attempt.error = batch.error;
            return attempt;
        }
    }
    for reference in refs {
        if let Some(error) = context.error() {
            attempt.error = Some(error.into());
            return attempt;
        }
        let source = reference.to_string();
        let path = match direct_ref_path(&request.download_path, &reference) {
            Ok(path) => path,
            Err(error) => {
                attempt.report.failures.push(failure(source, "", error));
                continue;
            }
        };
        match client
            .save_ref(context.clone(), reference, path.clone())
            .await
        {
            Ok(saved) => commit(&mut attempt.report, path, saved.size),
            Err(error) => {
                if let Some(error) = context.error() {
                    attempt.error = Some(error.into());
                    return attempt;
                }
                attempt.report.failures.push(failure(source, "", error));
            }
        }
    }
    for source in urls {
        if let Some(error) = context.error() {
            attempt.error = Some(error.into());
            return attempt;
        }
        let path = match direct_url_path(&request.download_path, &source) {
            Ok(path) => path,
            Err(error) => {
                attempt
                    .report
                    .failures
                    .push(failure(REDACTED_SOURCE.into(), "resource", error));
                continue;
            }
        };
        if !client.supports_direct_urls() {
            attempt.report.failures.push(failure(
                REDACTED_SOURCE.into(),
                "resource",
                message("download client does not support direct resource URLs"),
            ));
            continue;
        }
        match client
            .save_url(context.clone(), source.clone(), path.clone())
            .await
        {
            Ok(saved) => commit(&mut attempt.report, path, saved.size),
            Err(error) => {
                if let Some(error) = context.error() {
                    attempt.error = Some(error.into());
                    return attempt;
                }
                attempt.report.failures.push(failure(
                    REDACTED_SOURCE.into(),
                    "resource",
                    redact_error(&source, error),
                ));
            }
        }
    }
    attempt
}
fn commit(report: &mut DownloadReport, path: PathBuf, bytes: i64) {
    report.items.push(DownloadedItem {
        kind: "resource".into(),
        files: vec![DownloadedFile {
            path,
            page: 1,
            bytes,
        }],
        ..Default::default()
    });
    report.committed = true;
}
fn direct_ref_path(root: &str, reference: &ResourceRef) -> Result<PathBuf, SchedulerError> {
    require_root(root)?;
    Ok(clean_path(&Path::new(root).join(format!(
        "resource-{:x}",
        Sha256::digest(reference.as_str().as_bytes())
    ))))
}
fn require_root(root: &str) -> Result<(), SchedulerError> {
    if root.trim().is_empty() {
        Err(message(
            "download path is required for direct resource sources",
        ))
    } else {
        Ok(())
    }
}
fn clean_path(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(output.components().next_back(), Some(Component::Normal(_))) {
                    output.pop();
                } else if !output.has_root() {
                    output.push("..");
                }
            }
            other => output.push(other.as_os_str()),
        }
    }
    if output.as_os_str().is_empty() {
        output.push(".");
    }
    output
}
fn decode_component(value: &str, query: bool) -> Option<String> {
    let bytes = value.as_bytes();
    let mut result = vec![];
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let high = (*bytes.get(index + 1)? as char).to_digit(16)?;
                let low = (*bytes.get(index + 2)? as char).to_digit(16)?;
                result.push((high * 16 + low) as u8);
                index += 3;
            }
            b'+' if query => {
                result.push(b' ');
                index += 1;
            }
            byte => {
                result.push(byte);
                index += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&result).into_owned())
}
fn direct_url_path(root: &str, source: &str) -> Result<PathBuf, SchedulerError> {
    require_root(root)?;
    let parsed =
        LoginUrl::parse(source.trim()).ok_or_else(|| message("direct resource URL is invalid"))?;
    let escaped = parsed.escaped_path();
    let unusable = || message("direct resource URL has no usable basename");
    if escaped.is_empty() || escaped.ends_with('/') {
        return Err(unusable());
    }
    let basename = escaped.rsplit('/').next().ok_or_else(unusable)?;
    let name = decode_component(basename, false).ok_or_else(unusable)?;
    let name: String = name
        .chars()
        .map(|c| {
            if c < '\u{20}' || c == '\u{7f}' || "\\/*?:\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim_end_matches(['.', ' ']);
    if name.is_empty() || matches!(name, "." | "..") {
        return Err(unusable());
    }
    let serialized = parsed.with_fragment(parsed.fragment());
    let hash = Sha256::digest(serialized.as_bytes());
    let suffix = hash[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let (stem, extension) = match name.rfind('.') {
        Some(index) if index > 0 => (&name[..index], &name[index..]),
        _ => (name, ""),
    };
    Ok(clean_path(
        &Path::new(root).join(format!("{stem}-{suffix}{extension}")),
    ))
}
fn redact_error(source: &str, error: SchedulerError) -> SchedulerError {
    let original = error.to_string();
    let mut redacted = original.replace(source, REDACTED_SOURCE);
    if let Some(parsed) = LoginUrl::parse(source) {
        if !parsed.raw_query().is_empty() {
            redacted = redacted.replace(parsed.raw_query(), "[redacted]");
        }
        let mut keys = BTreeSet::new();
        for entry in parsed
            .raw_query()
            .split('&')
            .filter(|entry| !entry.contains(';'))
        {
            let (key, value) = entry.split_once('=').unwrap_or((entry, ""));
            if let (Some(key), Some(_)) =
                (decode_component(key, true), decode_component(value, true))
                && !key.is_empty()
            {
                keys.insert(key);
            }
        }
        for key in keys {
            let marker = format!("{key}=");
            let mut from = 0;
            while let Some(relative) = redacted[from..].find(&marker) {
                let start = from + relative + marker.len();
                let end = redacted[start..]
                    .find(['&', ' ', '\t', '\r', '\n', '"', '\'', '<', '>'])
                    .map_or(redacted.len(), |index| start + index);
                redacted.replace_range(start..end, "[redacted]");
                from = start + "[redacted]".len();
            }
        }
    }
    if redacted == original {
        error
    } else {
        message(redacted)
    }
}

pub fn validate_quality(quality: &str) -> Result<(), SchedulerError> {
    if matches!(
        quality,
        "" | "original" | "regular" | "small" | "thumb" | "mini"
    ) {
        Ok(())
    } else {
        Err(message(
            "quality must be one of original, regular, small, thumb, mini",
        ))
    }
}
pub fn validate_ugoira_format(format: &str) -> Result<(), SchedulerError> {
    if matches!(format, "" | "gif" | "apng" | "zip" | "raw") {
        Ok(())
    } else {
        Err(message("ugoira format must be one of gif, apng, zip, raw"))
    }
}
pub fn parse_pages(raw: &str) -> Result<Vec<i64>, SchedulerError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(vec![]);
    }
    let mut pages = BTreeSet::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(message("page selection contains an empty entry"));
        }
        let (start, end) = if let Some((first, last)) = part.split_once('-') {
            let invalid = || message(format!("invalid page range {}", go_quote(part)));
            let start = first.trim().parse::<i64>().map_err(|_| invalid())?;
            let end = last.trim().parse::<i64>().map_err(|_| invalid())?;
            if start <= 0 || end < start {
                return Err(invalid());
            }
            if end - start > 100000 {
                return Err(message(format!(
                    "page range {} is too large",
                    go_quote(part)
                )));
            }
            (start, end)
        } else {
            let number = part
                .parse::<i64>()
                .map_err(|_| message(format!("invalid page number {}", go_quote(part))))?;
            if number <= 0 {
                return Err(message(format!("invalid page number {}", go_quote(part))));
            }
            (number, number)
        };
        pages.extend(start..=end);
    }
    Ok(pages.into_iter().collect())
}
