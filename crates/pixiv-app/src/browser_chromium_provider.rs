use crate::{
    browser_chromium::ChromiumCookieDecoder,
    browser_cookies::{BrowserCookieError, BrowserFiles, BrowserProfile, CookieQuery, SecretBytes},
    browser_sqlite::SqliteCommand,
    lifecycle::Context,
};
use std::{ffi::OsString, io, path::PathBuf, sync::Arc};
pub struct ChromiumCookieProvider {
    root: PathBuf,
    files: Arc<dyn BrowserFiles>,
    sqlite: Arc<SqliteCommand>,
    decoder: ChromiumCookieDecoder,
}
impl ChromiumCookieProvider {
    pub fn new(
        root: PathBuf,
        files: Arc<dyn BrowserFiles>,
        sqlite: Arc<SqliteCommand>,
        decoder: ChromiumCookieDecoder,
    ) -> Self {
        Self {
            root,
            files,
            sqlite,
            decoder,
        }
    }
    pub fn discover_profiles(
        &self,
        context: &Context,
    ) -> Result<Vec<BrowserProfile>, BrowserCookieError> {
        let Self { root, files, .. } = self;
        if let Some(error) = context.error() {
            return Err(BrowserCookieError::Context(error));
        }
        let entries = files.read_dir(root).map_err(storage_discovery_error)?;
        let mut profiles = Vec::new();
        for entry in entries {
            let id = entry.name.as_encoded_bytes();
            if !entry.is_dir || !safe_directory_id(id) {
                continue;
            }
            let path = root.join(&entry.name);
            match files.metadata(&path.join("Cookies")) {
                Ok(metadata) if !metadata.is_dir => profiles.push(BrowserProfile {
                    id: id.to_vec(),
                    name: id.to_vec(),
                    path,
                }),
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    return Err(BrowserCookieError::PermissionDenied);
                }
                _ => {}
            }
        }
        if profiles.is_empty() {
            return Err(BrowserCookieError::NotInstalled);
        }
        Ok(profiles)
    }
    pub fn read(
        &self,
        context: &Context,
        query: &CookieQuery,
        profile_id: &[u8],
    ) -> Result<Vec<SecretBytes>, BrowserCookieError> {
        let Self {
            root,
            files,
            sqlite,
            decoder,
        } = self;
        if !safe_directory_id(profile_id) {
            return Err(BrowserCookieError::InvalidProfileId);
        }
        let id = native_profile_name(profile_id)?;
        let path = root.join(id).join("Cookies");
        files.metadata(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::PermissionDenied {
                BrowserCookieError::PermissionDenied
            } else {
                BrowserCookieError::DatabaseNotFound
            }
        })?;
        let params = std::collections::BTreeMap::from([
            ("@h1".into(), query.host().into()),
            (
                "@h2".into(),
                query
                    .host()
                    .strip_prefix('.')
                    .unwrap_or(query.host())
                    .into(),
            ),
            ("@n".into(), query.name().into()),
        ]);
        let rows = sqlite.query(context, &path, "SELECT host_key, value, hex(encrypted_value) FROM cookies WHERE (host_key = @h1 OR host_key = @h2) AND name = @n;", &params)?;
        decoder.decode_rows(context, rows).into_result()
    }
    pub fn close(&self) -> Result<(), BrowserCookieError> {
        Ok(())
    }
}
fn storage_discovery_error(error: io::Error) -> BrowserCookieError {
    if error.kind() == io::ErrorKind::PermissionDenied {
        BrowserCookieError::PermissionDenied
    } else {
        BrowserCookieError::NotInstalled
    }
}
fn safe_directory_id(id: &[u8]) -> bool {
    if id.is_empty() || id.starts_with(b".") || id.contains(&b'/') {
        return false;
    }
    #[cfg(windows)]
    if id.contains(&b'\\') || id.contains(&b':') {
        return false;
    }
    true
}
#[cfg(unix)]
fn native_profile_name(id: &[u8]) -> Result<OsString, BrowserCookieError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(OsString::from_vec(id.to_vec()))
}
#[cfg(not(unix))]
fn native_profile_name(id: &[u8]) -> Result<OsString, BrowserCookieError> {
    std::str::from_utf8(id)
        .map(OsString::from)
        .map_err(|_| BrowserCookieError::InvalidProfileId)
}
