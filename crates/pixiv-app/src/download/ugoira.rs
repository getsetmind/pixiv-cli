use super::{
    AnimationEncoder, DownloadRequest, DownloadSaveClient, DownloadWarning, DownloadedFile,
    DownloadedItem, EncoderInput, UgoiraFrameReport, file_replace, message,
    static_artwork::ArtworkResult, static_filename, zip_directory,
};
use crate::{lifecycle::Context, scheduler::SchedulerError};
use pixiv_sdk::models::{Artwork, UgoiraFrame};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

struct TemporaryArchive {
    path: PathBuf,
    preserve: bool,
}
impl TemporaryArchive {
    fn create(base: &Path) -> Result<Self, SchedulerError> {
        for _ in 0..128 {
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random).map_err(|e| message(e.to_string()))?;
            let name = random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            let path = base.join(format!("ugoira-{name}.zip"));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    if let Err(error) = crate::config::private_file::close(file) {
                        let _ = fs::remove_file(&path);
                        return Err(message(error.to_string()));
                    };
                    return Ok(Self {
                        path,
                        preserve: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(message(error.to_string())),
            }
        }
        Err(message("could not create ugoira staging file"))
    }
    fn publish(&mut self, path: &Path) -> Result<(), SchedulerError> {
        match file_replace::replace_file(&self.path, path) {
            Ok(()) => {
                self.preserve = true;
                Ok(())
            }
            Err(error) => {
                self.preserve = error.preserve_source;
                Err(error.error)
            }
        }
    }
}
impl Drop for TemporaryArchive {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn basename(
    artwork: &Artwork,
    template: &str,
) -> Result<(String, Vec<DownloadWarning>), SchedulerError> {
    let generated = static_filename::generate(artwork, 0, template);
    if let Ok(name) = &generated
        && !name.is_empty()
    {
        return Ok((name.clone(), vec![]));
    }
    let fallback = static_filename::generate(artwork, 0, "")
        .map_err(|e| message(format!("ugoira filename fallback failed: {e}")))?;
    if fallback.is_empty() {
        return Err(message("ugoira filename fallback produced an empty name"));
    }
    let warning = DownloadWarning {
        illust_id: artwork.id,
        kind: "ugoira".into(),
        message: if generated.is_err() {
            "ugoira filename template failed; using default filename"
        } else {
            "ugoira filename template produced an empty name; using default filename"
        }
        .into(),
    };
    Ok((fallback, vec![warning]))
}
fn classified(
    error: SchedulerError,
    code: &str,
    path: PathBuf,
    missing: Vec<String>,
) -> ArtworkResult {
    ArtworkResult {
        error: Some(error),
        code: code.into(),
        path,
        missing,
        ..Default::default()
    }
}

pub(super) async fn download(
    context: &Context,
    client: &(impl DownloadSaveClient + ?Sized),
    encoder: &dyn AnimationEncoder,
    artwork: Artwork,
    request: &DownloadRequest,
    base: &Path,
) -> ArtworkResult {
    let quality = if request.quality.is_empty() {
        "original"
    } else {
        &request.quality
    };
    if quality != "original" {
        return ArtworkResult::error(message(format!(
            "ugoira quality {} is unsupported; only original is supported",
            crate::auth_bundle::go_quote(quality)
        )));
    }
    if !request.pages.is_empty() {
        return ArtworkResult::error(message("ugoira page selection is unsupported"));
    }
    let format = if request.ugoira_format.is_empty() {
        "gif"
    } else {
        &request.ugoira_format
    };
    let archive_only = matches!(format, "zip" | "raw");
    let (name, warnings) = match basename(&artwork, &request.filename_template) {
        Ok(pair) => pair,
        Err(error) => return ArtworkResult::error(error),
    };
    let mut result = async {
        let Some(metadata) = client.ugoira_metadata(context.clone(), artwork.id) else {
            return ArtworkResult::error(message("ugoira download is not yet supported"));
        };
        let metadata = match metadata.await {
            Ok(metadata) => metadata,
            Err(error) => return ArtworkResult::error(error),
        };
        let archive = metadata
            .archives
            .iter()
            .find(|a| a.quality == "original")
            .or_else(|| metadata.archives.first());
        let Some(archive) = archive.filter(|a| !a.resource.url.is_empty()) else {
            let error = message(format!("ugoira {} has no downloadable archive", artwork.id));
            return if archive_only {
                classified(error, "ugoira_archive_missing", PathBuf::new(), vec![])
            } else {
                ArtworkResult::error(error)
            };
        };
        let mut temporary = match TemporaryArchive::create(base) {
            Ok(t) => t,
            Err(error) => {
                return if archive_only {
                    classified(error, "write_failed", PathBuf::new(), vec![])
                } else {
                    ArtworkResult::error(error)
                };
            }
        };
        let saved = match client
            .save_ref(
                context.clone(),
                archive.resource.reference.clone(),
                temporary.path.clone(),
            )
            .await
        {
            Ok(saved) => saved,
            Err(error) => return ArtworkResult::error(error),
        };
        let mut item = DownloadedItem {
            illust_id: artwork.id,
            title: artwork.title.clone(),
            author: artwork.user.name.clone(),
            kind: "ugoira".into(),
            ..Default::default()
        };
        if !archive_only {
            let output = base.join(format!("{name}.{format}"));
            let input = EncoderInput {
                zip_path: temporary.path.clone(),
                output_path: output.clone(),
                work_dir: base.into(),
                frames: Some(metadata.frames),
                format: format.into(),
                max_edge: 0,
            };
            if let Err(error) = encoder.encode(context.clone(), input).await {
                return ArtworkResult::error(error);
            };
            let bytes = fs::metadata(&output).map(|m| m.len() as i64).unwrap_or(0);
            item.files.push(DownloadedFile {
                path: output,
                page: 1,
                bytes,
            });
            return ArtworkResult {
                item: Some(item),
                ..Default::default()
            };
        }
        let (report, code, error) = inspect(&temporary.path, &metadata.frames);
        if let Some(error) = error {
            let quarantine = Path::new(&request.download_path)
                .join(".quarantine")
                .join(format!("{}.zip", artwork.id));
            if let Err(error) = fs::create_dir_all(quarantine.parent().unwrap()) {
                return classified(
                    message(error.to_string()),
                    "write_failed",
                    PathBuf::new(),
                    vec![],
                );
            }
            if let Err(error) = temporary.publish(&quarantine) {
                return classified(error, "write_failed", PathBuf::new(), vec![]);
            }
            return classified(error, &code, quarantine, report.missing.unwrap_or_default());
        }
        let output = base.join(format!("{name}.zip"));
        if let Err(error) = temporary.publish(&output) {
            return classified(error, "write_failed", PathBuf::new(), vec![]);
        }
        item.quality = archive.quality.clone();
        item.frames = metadata.frames;
        item.files.push(DownloadedFile {
            path: output,
            page: 1,
            bytes: saved.size,
        });
        let undeclared = report.undeclared.is_some();
        item.frame_report = Some(report);
        let mut result = ArtworkResult {
            item: Some(item),
            ..Default::default()
        };
        if undeclared {
            result.warnings.push(DownloadWarning {
                illust_id: artwork.id,
                kind: "ugoira".into(),
                message: "ugoira archive contains undeclared frames".into(),
            })
        };
        result
    }
    .await;
    let mut ordered = warnings;
    ordered.append(&mut result.warnings);
    result.warnings = ordered;
    result
}
fn safe_name(name: &[u8]) -> bool {
    !name.is_empty()
        && !name.contains(&b'\\')
        && !name.starts_with(b"/")
        && !name.starts_with(b"..")
        && name
            .split(|b| *b == b'/')
            .all(|part| !part.is_empty() && part != b"." && part != b"..")
}
fn inspect(
    path: &Path,
    declared: &[UgoiraFrame],
) -> (UgoiraFrameReport, String, Option<SchedulerError>) {
    let mut report = UgoiraFrameReport {
        declared: declared.len(),
        ..Default::default()
    };
    let names = match zip_directory::read_names(path) {
        Ok(names) => names,
        Err(error) => {
            return (
                report,
                "ugoira_archive_missing".into(),
                Some(message(format!("ugoira archive cannot be read: {error}"))),
            );
        }
    };
    let declared_set = declared
        .iter()
        .map(|f| f.filename.as_bytes())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut actual = vec![];
    for name in names {
        if name.ends_with(b"/") || name.starts_with(b"__MACOSX/") {
            continue;
        }
        if !safe_name(&name) {
            return (
                report,
                "insecure_frame_name".into(),
                Some(message("ugoira archive contains an unsafe entry name")),
            );
        }
        if !seen.insert(name.clone()) {
            return (
                report,
                "ugoira_frame_mismatch".into(),
                Some(message(format!(
                    "ugoira archive contains duplicate entry {}",
                    quote_bytes(&name)
                ))),
            );
        }
        actual.push(name);
        report.actual = actual.len();
    }
    let undeclared = actual
        .iter()
        .filter(|name| !declared_set.contains(name.as_slice()))
        .map(|name| go_json_text(name))
        .collect::<Vec<_>>();
    if !undeclared.is_empty() {
        report.undeclared = Some(undeclared)
    }
    let missing = declared
        .iter()
        .filter(|f| !seen.contains(f.filename.as_bytes()))
        .map(|f| f.filename.clone())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let count = missing.len();
        report.missing = Some(missing);
        return (
            report,
            "ugoira_frame_mismatch".into(),
            Some(message(format!(
                "ugoira archive is missing {count} declared frames"
            ))),
        );
    }
    if report.actual == 0 {
        return (
            report,
            "ugoira_archive_missing".into(),
            Some(message("ugoira archive has no frames")),
        );
    }
    (report, String::new(), None)
}

fn quote_bytes(mut bytes: &[u8]) -> String {
    let mut output = String::from("\"");
    while !bytes.is_empty() {
        let (valid, invalid) = match std::str::from_utf8(bytes) {
            Ok(text) => (text.len(), 0),
            Err(error) => (
                error.valid_up_to(),
                error
                    .error_len()
                    .unwrap_or(bytes.len() - error.valid_up_to()),
            ),
        };
        if valid > 0 {
            let quoted =
                crate::auth_bundle::go_quote(std::str::from_utf8(&bytes[..valid]).unwrap());
            output.push_str(&quoted[1..quoted.len() - 1]);
        }
        for byte in &bytes[valid..valid + invalid] {
            output.push_str(&format!("\\x{byte:02x}"));
        }
        bytes = &bytes[valid + invalid..];
    }
    output.push('"');
    output
}

fn go_json_text(mut bytes: &[u8]) -> String {
    let mut output = String::new();
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(text) => {
                output.push_str(text);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                output.push_str(std::str::from_utf8(&bytes[..valid]).unwrap());
                let invalid = error.error_len().unwrap_or(bytes.len() - valid);
                for _ in 0..invalid {
                    output.push('\u{fffd}');
                }
                bytes = &bytes[valid + invalid..];
            }
        }
    }
    output
}
