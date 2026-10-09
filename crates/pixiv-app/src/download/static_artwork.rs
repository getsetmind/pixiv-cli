use super::{
    DownloadAttempt, DownloadRequest, DownloadSaveClient, DownloadedFile, DownloadedItem, failure,
    message, static_filename, validate_quality, validate_ugoira_format,
};
use crate::{auth_bundle::go_quote, lifecycle::Context, scheduler::SchedulerError};
use futures_util::{StreamExt, stream::FuturesUnordered};
use pixiv_sdk::{
    models::{Artwork, ArtworkKind},
    pixiv::artwork_variant_resource,
};
use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
};

struct ArtworkResult {
    item: Option<DownloadedItem>,
    error: Option<SchedulerError>,
    missing_capability: bool,
}
impl ArtworkResult {
    fn error(error: SchedulerError) -> Self {
        Self {
            item: None,
            error: Some(error),
            missing_capability: false,
        }
    }
}

pub(super) async fn download_batch(
    context: &Context,
    client: &(impl DownloadSaveClient + ?Sized),
    ids: Vec<i64>,
    request: &DownloadRequest,
) -> DownloadAttempt {
    let mut attempt = DownloadAttempt::default();
    if let Err(error) = validate_quality(&request.quality)
        .and_then(|()| validate_ugoira_format(&request.ugoira_format))
        .and_then(|()| static_filename::validate_directory(request.directory_template.trim()))
    {
        attempt.error = Some(error);
        return attempt;
    }
    if let Some(error) = context.error() {
        attempt.error = Some(error.into());
        return attempt;
    }
    let workers = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(ids.len());
    let mut active = FuturesUnordered::new();
    let mut next = 0;
    let mut results = (0..ids.len()).map(|_| None).collect::<Vec<_>>();
    while next < workers {
        active.push(download_slot(context, client, next, ids[next], request));
        next += 1;
    }
    while let Some((index, result)) = active.next().await {
        results[index] = Some(result);
        if next < ids.len() && context.error().is_none() {
            active.push(download_slot(context, client, next, ids[next], request));
            next += 1;
        }
    }
    let mut worker_context_error = None;
    for (index, result) in results.into_iter().enumerate() {
        let Some(result) = result else {
            continue;
        };
        if let Some(item) = result.item {
            attempt.report.items.push(item);
            attempt.report.committed = true;
        }
        if result.missing_capability {
            attempt.error = Some(message("artwork download is not yet supported"));
            continue;
        }
        if let Some(error) = result.error {
            if error.is_canceled() || error.is_deadline_exceeded() {
                if worker_context_error.is_none() {
                    worker_context_error = Some(error);
                }
            } else {
                let mut rejected = failure(String::new(), "", error);
                rejected.illust_id = ids[index];
                attempt.report.failures.push(rejected);
            }
        }
    }
    if let Some(error) = context.error() {
        attempt.error = Some(error.into());
    } else if attempt.error.is_none() {
        attempt.error = worker_context_error;
    }
    attempt
}
async fn download_slot(
    context: &Context,
    client: &(impl DownloadSaveClient + ?Sized),
    index: usize,
    id: i64,
    request: &DownloadRequest,
) -> (usize, ArtworkResult) {
    let Some(metadata) = client.artwork(context.clone(), id) else {
        return (
            index,
            ArtworkResult {
                item: None,
                error: None,
                missing_capability: true,
            },
        );
    };
    let artwork = match metadata.await {
        Ok(artwork) => artwork,
        Err(error) => return (index, ArtworkResult::error(error)),
    };
    let result = download_artwork(context, client, artwork, request).await;
    (index, result)
}
async fn download_artwork(
    context: &Context,
    client: &(impl DownloadSaveClient + ?Sized),
    artwork: Artwork,
    request: &DownloadRequest,
) -> ArtworkResult {
    let base = match artwork_directory(&artwork, request) {
        Ok(base) => base,
        Err(error) => return ArtworkResult::error(error),
    };
    if let Err(error) = std::fs::create_dir_all(&base) {
        return ArtworkResult::error(message(format!("mkdir {}: {}", base.display(), error)));
    }
    let kind = match artwork.kind {
        ArtworkKind::Illust => "illustration",
        ArtworkKind::Manga => "manga",
        ArtworkKind::Ugoira => "ugoira",
        ArtworkKind::Unknown => "unknown",
    };
    if artwork.kind == ArtworkKind::Ugoira {
        return ArtworkResult::error(message("ugoira download is not yet supported"));
    }
    let selected = match select_pages(&artwork, &request.pages) {
        Ok(selected) => selected,
        Err(error) => return ArtworkResult::error(error),
    };
    if let Err(error) = static_filename::validate_template(&request.filename_template) {
        return ArtworkResult::error(error);
    }
    let mut out = DownloadedItem {
        illust_id: artwork.id,
        title: artwork.title.clone(),
        author: artwork.user.name.clone(),
        kind: kind.into(),
        files: vec![],
    };
    for index in selected {
        let page = &artwork.pages[index];
        let result = async {
            if page.image.resource.url.is_empty() {
                return Err(message(format!(
                    "illust {} page {} has no image URL",
                    artwork.id,
                    index + 1
                )));
            }
            let basename =
                static_filename::generate(&artwork, index as i64, &request.filename_template)?;
            if basename.is_empty() {
                return Err(message(format!(
                    "illust {} page {} produced an empty filename",
                    artwork.id,
                    index + 1
                )));
            }
            let quality = if request.quality.is_empty() {
                "original"
            } else {
                &request.quality
            };
            let reference = artwork_variant_resource(&page.image.resource, quality)?;
            let path = base.join(format!(
                "{basename}{}",
                download_extension(&page.image.resource.url)
            ));
            let saved = client
                .save_ref(context.clone(), reference, path.clone())
                .await?;
            let final_path = match publish_extension(&path, &saved.content_type) {
                Ok(path) => path,
                Err(error) => {
                    if let Err(cleanup) = std::fs::remove_file(&saved.path)
                        && cleanup.kind() != std::io::ErrorKind::NotFound
                    {
                        return Err(message(format!(
                            "{error}; remove invalid downloaded file: {cleanup}"
                        )));
                    }
                    return Err(error);
                }
            };
            Ok(DownloadedFile {
                path: final_path,
                page: index as i64 + 1,
                bytes: saved.size,
            })
        }
        .await;
        match result {
            Ok(file) => out.files.push(file),
            Err(error) => {
                return ArtworkResult {
                    item: if out.files.is_empty() {
                        None
                    } else {
                        Some(out)
                    },
                    error: Some(error),
                    missing_capability: false,
                };
            }
        }
    }
    ArtworkResult {
        item: Some(out),
        error: None,
        missing_capability: false,
    }
}
fn artwork_directory(
    artwork: &Artwork,
    request: &DownloadRequest,
) -> Result<PathBuf, SchedulerError> {
    let mut base = PathBuf::from(&request.download_path);
    if !base.is_absolute() {
        base = std::env::current_dir()
            .map_err(|error| message(error.to_string()))?
            .join(base);
    }
    base = super::clean_path(&base);
    let directory = request.directory_template.trim();
    if !directory.is_empty() {
        base.push(static_filename::build_directory(directory, artwork)?);
    } else if artwork.page_count > 1 || artwork.kind == ArtworkKind::Ugoira {
        base.push(static_filename::sanitize(&format!(
            "{} - {}",
            artwork.id,
            if artwork.title.is_empty() {
                "Untitled"
            } else {
                &artwork.title
            }
        )));
    }
    Ok(base)
}
fn select_pages(artwork: &Artwork, pages: &[i64]) -> Result<Vec<usize>, SchedulerError> {
    let total = if artwork.page_count <= 0 {
        artwork.pages.len() as i64
    } else {
        artwork.page_count
    };
    if total <= 0 || artwork.pages.is_empty() {
        return Err(message(format!(
            "illust {} has no page metadata",
            artwork.id
        )));
    }
    if pages.is_empty() {
        return Ok((0..artwork.pages.len()).collect());
    }
    let mut selected = BTreeSet::new();
    for &page in pages {
        if page < 1 || page > total || page > artwork.pages.len() as i64 {
            return Err(message(format!(
                "page {page} does not exist (page_count={total})"
            )));
        }
        selected.insert(page as usize - 1);
    }
    Ok(selected.into_iter().collect())
}
fn download_extension(url: &str) -> String {
    let escaped = pixiv_sdk::oauth::LoginUrl::parse(url)
        .map(|url| url.escaped_path().to_owned())
        .unwrap_or_default();
    let path = super::decode_component(&escaped, false).unwrap_or_default();
    let basename = path.rsplit('/').next().unwrap_or_default();
    let extension = basename_extension(basename);
    static_filename::sanitize(extension)
        .chars()
        .map(|c| {
            if c < '\u{20}' || c == '\u{7f}' {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim_end_matches(['.', ' '])
        .into()
}
fn extension(mime: &str) -> Option<&'static str> {
    match mime {
        "image/jpeg" => Some(".jpg"),
        "image/png" => Some(".png"),
        "image/gif" => Some(".gif"),
        "image/webp" => Some(".webp"),
        _ => None,
    }
}
fn signature(path: &Path) -> Option<&'static str> {
    let mut buffer = [0_u8; 512];
    let count = std::fs::File::open(path)
        .and_then(|mut file| file.read(&mut buffer))
        .unwrap_or_default();
    let bytes = &buffer[..count];
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 14 && bytes.starts_with(b"RIFF") && &bytes[8..14] == b"WEBPVP" {
        Some("image/webp")
    } else {
        None
    }
}
fn basename_extension(basename: &str) -> &str {
    basename
        .rfind('.')
        .map(|index| &basename[index..])
        .unwrap_or_default()
}
fn suffix_mime(path: &Path) -> &'static str {
    let basename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    match basename_extension(basename).to_ascii_lowercase().as_str() {
        ".jpg" | ".jpeg" => "image/jpeg",
        ".png" => "image/png",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        _ => "application/octet-stream",
    }
}
fn publish_extension(path: &Path, content_type: &str) -> Result<PathBuf, SchedulerError> {
    let reported = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let detected = if extension(&reported).is_some() {
        reported.as_str()
    } else {
        signature(path).unwrap_or_else(|| {
            if reported.is_empty() {
                suffix_mime(path)
            } else {
                &reported
            }
        })
    };
    let Some(want) = extension(detected) else {
        return Err(message(format!(
            "unsupported image content type {}",
            go_quote(if reported.is_empty() {
                detected
            } else {
                &reported
            })
        )));
    };
    let basename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let current = basename_extension(basename);
    if current.eq_ignore_ascii_case(want) || want == ".jpg" && current.eq_ignore_ascii_case(".jpeg")
    {
        return Ok(path.into());
    }
    let stem = &basename[..basename.len() - current.len()];
    let target = path.with_file_name(format!("{stem}{want}"));
    std::fs::hard_link(path, &target).map_err(|error| {
        message(format!(
            "publish detected image extension: link {} {}: {}",
            path.display(),
            target.display(),
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "file exists".into()
            } else {
                error.to_string()
            }
        ))
    })?;
    if let Err(error) = std::fs::remove_file(path) {
        let _ = std::fs::remove_file(&target);
        return Err(message(format!("remove mismatched image path: {error}")));
    }
    Ok(target)
}
