use crate::lifecycle::ContextError;
use std::{
    ffi::OsString,
    fmt,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserCookieError {
    UnknownBrowser,
    QueryInvalid,
    ProfileNotFound,
    InvalidProfileId,
    DatabaseLocked,
    DatabaseNotFound,
    EncryptedValueUnsupported,
    EncryptedCookieUnsupported,
    EncryptedFormatUnknown,
    EncryptedMalformed,
    NotInstalled,
    SqliteUnavailable,
    QueryFailed,
    InvalidFormat,
    TempSnapshot,
    KeychainAccess,
    KeychainItemNotFound,
    PermissionDenied,
    SecretServiceUnavailable,
    SecretServiceAccess,
    Dpapi,
    Context(ContextError),
}
impl fmt::Display for BrowserCookieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBrowser => f.write_str("browsercookies: unknown browser"),
            Self::QueryInvalid => f.write_str("browsercookies: invalid cookie query"),
            Self::ProfileNotFound => f.write_str("browsercookies: profile not found"),
            Self::InvalidProfileId => f.write_str("browsercookies: invalid profile identifier"),
            Self::DatabaseLocked => f.write_str("browsercookies: browser cookie database is locked (browser may be running)"),
            Self::DatabaseNotFound => f.write_str("browsercookies: browser cookie database not found"),
            Self::EncryptedValueUnsupported => f.write_str("browsercookies: cookie value is encrypted and decryption is not supported on this platform"),
            Self::EncryptedCookieUnsupported => f.write_str("browsercookies: cookie value is encrypted and decryption is not supported by this provider"),
            Self::EncryptedFormatUnknown => f.write_str("browsercookies: cookie value uses an unknown encryption format"),
            Self::EncryptedMalformed => f.write_str("browsercookies: encrypted cookie value is malformed"),
            Self::NotInstalled => f.write_str("browsercookies: browser is not installed"),
            Self::SqliteUnavailable => f.write_str("browsercookies: the sqlite3 command-line tool is required but was not found"),
            Self::QueryFailed => f.write_str("browsercookies: cookie database query failed"),
            Self::InvalidFormat => f.write_str("browsercookies: cookie storage file has an invalid format"),
            Self::TempSnapshot => f.write_str("browsercookies: could not create a private temporary snapshot"),
            Self::KeychainAccess => f.write_str("browsercookies: keychain access failed"),
            Self::KeychainItemNotFound => f.write_str("browsercookies: keychain item not found"),
            Self::PermissionDenied => f.write_str("browsercookies: browser storage permission denied"),
            Self::SecretServiceUnavailable => f.write_str("browsercookies: Linux Secret Service is unavailable"),
            Self::SecretServiceAccess => f.write_str("browsercookies: Linux Secret Service access failed"),
            Self::Dpapi => f.write_str("browsercookies: Windows DPAPI access failed"),
            Self::Context(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for BrowserCookieError {}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretBytes(Vec<u8>);
impl SecretBytes {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}
impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
impl fmt::Display for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
impl serde::Serialize for SecretBytes {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("<redacted>")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct BrowserProfile {
    pub id: Vec<u8>,
    pub name: Vec<u8>,
    pub path: PathBuf,
}
impl fmt::Debug for BrowserProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BrowserProfile(<redacted>)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookieQuery {
    host: String,
    name: String,
}
impl CookieQuery {
    pub fn new(host: &str, name: &str) -> Result<Self, BrowserCookieError> {
        fn valid(value: &str) -> bool {
            !value.is_empty()
                && value.len() <= 256
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        }
        if !valid(host) || !valid(name) {
            return Err(BrowserCookieError::QueryInvalid);
        }
        Ok(Self {
            host: host.into(),
            name: name.into(),
        })
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

pub struct BrowserDirEntry {
    pub name: OsString,
    pub is_dir: bool,
}
pub struct BrowserMetadata {
    pub is_dir: bool,
}
pub trait BrowserFiles: Send + Sync {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn open_file(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
        self.read_file(path)
            .map(|data| Box::new(io::Cursor::new(data)) as Box<dyn Read + Send>)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<BrowserDirEntry>>;
    fn metadata(&self, path: &Path) -> io::Result<BrowserMetadata>;
}
pub struct SystemBrowserFiles;
impl BrowserFiles for SystemBrowserFiles {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }
    fn open_file(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(std::fs::File::open(path)?))
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<BrowserDirEntry>> {
        let mut entries = std::fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                Ok(BrowserDirEntry {
                    name: entry.file_name(),
                    is_dir: entry.file_type()?.is_dir(),
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort_by(|left, right| {
            left.name
                .as_encoded_bytes()
                .cmp(right.name.as_encoded_bytes())
        });
        Ok(entries)
    }
    fn metadata(&self, path: &Path) -> io::Result<BrowserMetadata> {
        std::fs::metadata(path).map(|metadata| BrowserMetadata {
            is_dir: metadata.is_dir(),
        })
    }
}

pub trait BrowserEnvironment: Send + Sync {
    fn user_home(&self) -> io::Result<PathBuf>;
    fn config_home(&self) -> Option<PathBuf>;
}
pub struct SystemBrowserEnvironment;
impl BrowserEnvironment for SystemBrowserEnvironment {
    fn user_home(&self) -> io::Result<PathBuf> {
        let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        std::env::var_os(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory unavailable"))
    }
    fn config_home(&self) -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }
}

use crate::{
    browser_chromium::{ChromiumCookieDecoder, ChromiumKind, PlatformChromiumKeyStore},
    browser_dpapi::WindowsDpapi,
    browser_profiles::{FirefoxCookieProvider, SafariCookieProvider},
    browser_sqlite::SqliteCommand,
    host_context::ContextHostProcess,
    host_process::{HostPlatform, SystemHostProcess},
    lifecycle::Context,
};
use std::sync::Arc;

enum CookieBackend {
    Chromium(crate::browser_chromium_provider::ChromiumCookieProvider),
    Firefox(FirefoxCookieProvider),
    Safari(SafariCookieProvider),
}
pub struct BrowserCookieBackend {
    inner: CookieBackend,
}
impl BrowserCookieBackend {
    pub fn new(
        browser: &str,
        environment: Arc<dyn BrowserEnvironment>,
        files: Arc<dyn BrowserFiles>,
        process: Arc<dyn ContextHostProcess>,
        dpapi: Arc<WindowsDpapi>,
    ) -> Result<Self, BrowserCookieError> {
        let normalized = browser
            .trim()
            .chars()
            .map(|character| character.to_lowercase().next().unwrap())
            .collect::<String>();
        let browser = normalized.as_str();
        let home = environment.user_home().unwrap_or_default();
        let platform = process.platform();
        let config = environment
            .config_home()
            .unwrap_or_else(|| home.join(".config"));
        let sqlite = Arc::new(SqliteCommand::new(process.clone()));
        let inner = match browser {
            "chrome" | "edge" => {
                let kind = if browser == "chrome" {
                    ChromiumKind::Chrome
                } else {
                    ChromiumKind::Edge
                };
                let root = if home.as_os_str().is_empty() {
                    PathBuf::new()
                } else {
                    match platform {
                        HostPlatform::Linux => config.join(if browser == "chrome" {
                            "google-chrome"
                        } else {
                            "microsoft-edge"
                        }),
                        HostPlatform::Darwin => {
                            let base = home.join("Library/Application Support");
                            if browser == "chrome" {
                                base.join("Google/Chrome")
                            } else {
                                base.join("Microsoft Edge")
                            }
                        }
                        HostPlatform::Windows => {
                            home.join("AppData/Local").join(if browser == "chrome" {
                                "Google/Chrome/User Data"
                            } else {
                                "Microsoft/Edge/User Data"
                            })
                        }
                        _ => PathBuf::new(),
                    }
                };
                let keys = Arc::new(PlatformChromiumKeyStore::new(
                    kind,
                    root.clone(),
                    files.clone(),
                    process,
                    dpapi,
                ));
                CookieBackend::Chromium(
                    crate::browser_chromium_provider::ChromiumCookieProvider::new(
                        root,
                        files,
                        sqlite,
                        ChromiumCookieDecoder::new(platform, keys),
                    ),
                )
            }
            "firefox" => {
                let root = if home.as_os_str().is_empty() {
                    PathBuf::new()
                } else {
                    match platform {
                        HostPlatform::Linux => config.join("mozilla/firefox"),
                        HostPlatform::Darwin => home.join("Library/Application Support/Firefox"),
                        HostPlatform::Windows => home.join("AppData/Roaming/Mozilla/Firefox"),
                        _ => PathBuf::new(),
                    }
                };
                CookieBackend::Firefox(FirefoxCookieProvider::new(root, files, sqlite))
            }
            "safari" => {
                let paths = if home.as_os_str().is_empty() {
                    Vec::new()
                } else {
                    vec![home.join("Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies"), home.join("Library/Cookies/Cookies.binarycookies")]
                };
                CookieBackend::Safari(SafariCookieProvider::new(paths, files))
            }
            _ => return Err(BrowserCookieError::UnknownBrowser),
        };
        Ok(Self { inner })
    }
    pub fn system(browser: &str) -> Result<Self, BrowserCookieError> {
        Self::new(
            browser,
            Arc::new(SystemBrowserEnvironment),
            Arc::new(SystemBrowserFiles),
            Arc::new(SystemHostProcess),
            Arc::new(WindowsDpapi::system()),
        )
    }
    pub fn discover_profiles(
        &self,
        context: &Context,
    ) -> Result<Vec<BrowserProfile>, BrowserCookieError> {
        match &self.inner {
            CookieBackend::Firefox(provider) => provider.discover_profiles(context),
            CookieBackend::Safari(provider) => provider.discover_profiles(context),
            CookieBackend::Chromium(provider) => provider.discover_profiles(context),
        }
    }
    pub fn read(
        &self,
        context: &Context,
        query: &CookieQuery,
        profile_id: &[u8],
    ) -> Result<Vec<SecretBytes>, BrowserCookieError> {
        match &self.inner {
            CookieBackend::Firefox(provider) => provider.read(context, query, profile_id),
            CookieBackend::Safari(provider) => provider.read(context, query, profile_id),
            CookieBackend::Chromium(provider) => provider.read(context, query, profile_id),
        }
    }
    pub fn close(&self) -> Result<(), BrowserCookieError> {
        Ok(())
    }
}
