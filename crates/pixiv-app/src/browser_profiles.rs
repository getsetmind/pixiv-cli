use crate::{
    browser_cookies::{BrowserCookieError, BrowserFiles, BrowserProfile, CookieQuery, SecretBytes},
    browser_sqlite::SqliteCommand,
    lifecycle::Context,
};
use std::{
    collections::BTreeMap,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

const FIREFOX_SQL: &str =
    "SELECT value FROM moz_cookies WHERE (host = @h1 OR host = @h2) AND name = @n;";

pub struct FirefoxCookieProvider {
    root: PathBuf,
    files: Arc<dyn BrowserFiles>,
    sqlite: Arc<SqliteCommand>,
}

impl FirefoxCookieProvider {
    pub fn new(root: PathBuf, files: Arc<dyn BrowserFiles>, sqlite: Arc<SqliteCommand>) -> Self {
        Self {
            root,
            files,
            sqlite,
        }
    }

    pub fn discover_profiles(
        &self,
        context: &Context,
    ) -> Result<Vec<BrowserProfile>, BrowserCookieError> {
        if let Some(error) = context.error() {
            return Err(BrowserCookieError::Context(error));
        }
        let path = join_native(&self.root, Path::new("profiles.ini"));
        let mut file = self
            .files
            .open_file(&path)
            .map_err(|error| match error.kind() {
                io::ErrorKind::PermissionDenied => BrowserCookieError::PermissionDenied,
                io::ErrorKind::IsADirectory => BrowserCookieError::InvalidFormat,
                _ => BrowserCookieError::NotInstalled,
            })?;
        let mut data = Vec::new();
        file.read_to_end(&mut data).map_err(|error| {
            if error.kind() == io::ErrorKind::PermissionDenied {
                BrowserCookieError::PermissionDenied
            } else {
                BrowserCookieError::InvalidFormat
            }
        })?;
        let profiles = parse_profiles_ini(&data, &self.root, self.files.as_ref())?;
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
        if !safe_profile_id(profile_id) {
            return Err(BrowserCookieError::InvalidProfileId);
        }
        let profiles = self.discover_profiles(context)?;
        let profile = profiles
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .ok_or(BrowserCookieError::ProfileNotFound)?;
        let path = join_native(&profile.path, Path::new("cookies.sqlite"));
        let params = BTreeMap::from([
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
        let rows = self.sqlite.query(context, &path, FIREFOX_SQL, &params)?;
        let mut secrets = Vec::with_capacity(rows.len());
        for row in rows {
            let value = row
                .into_iter()
                .next()
                .ok_or(BrowserCookieError::QueryFailed)?;
            if std::str::from_utf8(&value).is_err() {
                return Err(BrowserCookieError::EncryptedCookieUnsupported);
            }
            secrets.push(SecretBytes::new(value));
        }
        Ok(secrets)
    }
}

struct IniSection {
    name: Vec<u8>,
    path: Vec<u8>,
    relative: bool,
}

fn parse_profiles_ini(
    data: &[u8],
    root: &Path,
    files: &dyn BrowserFiles,
) -> Result<Vec<BrowserProfile>, BrowserCookieError> {
    let mut sections: Vec<IniSection> = Vec::new();
    for raw_line in data.split(|byte| *byte == b'\n') {
        let line = trim_go_space(raw_line);
        if line.is_empty() || line.starts_with(b";") || line.starts_with(b"#") {
            continue;
        }
        if line.starts_with(b"[") && line.ends_with(b"]") {
            sections.push(IniSection {
                name: Vec::new(),
                path: Vec::new(),
                relative: true,
            });
            continue;
        }
        let Some(section) = sections.last_mut() else {
            continue;
        };
        let Some(equal) = line.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = trim_go_space(&line[..equal]);
        let value = trim_go_space(&line[equal + 1..]);
        match key {
            b"Name" => section.name = value.into(),
            b"Path" => section.path = value.into(),
            b"IsRelative" => section.relative = value == b"1",
            _ => {}
        }
    }
    let mut profiles = Vec::new();
    for section in sections {
        if section.path.is_empty() {
            continue;
        }
        let id = base_native(&section.path);
        if !safe_profile_id(&id) {
            continue;
        }
        let path = path_from_bytes(section.path)?;
        let path = if section.relative {
            join_native(root, &path)
        } else {
            path
        };
        match files.metadata(&join_native(&path, Path::new("cookies.sqlite"))) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return Err(BrowserCookieError::PermissionDenied);
            }
            Err(_) => continue,
        }
        let name = if section.name.is_empty() {
            id.clone()
        } else {
            section.name
        };
        profiles.push(BrowserProfile { id, name, path });
    }
    Ok(profiles)
}

fn safe_profile_id(id: &[u8]) -> bool {
    !id.is_empty() && !id.starts_with(b".") && base_native(id) == id
}

fn trim_go_space(mut bytes: &[u8]) -> &[u8] {
    while let Some(width) = first_space_width(bytes) {
        bytes = &bytes[width..];
    }
    while let Some(width) = last_space_width(bytes) {
        bytes = &bytes[..bytes.len() - width];
    }
    bytes
}

fn first_space_width(bytes: &[u8]) -> Option<usize> {
    for width in 1..=bytes.len().min(4) {
        if let Ok(text) = std::str::from_utf8(&bytes[..width]) {
            let mut chars = text.chars();
            let value = chars.next()?;
            if chars.next().is_none() {
                return value.is_whitespace().then_some(width);
            }
        }
    }
    None
}

fn last_space_width(bytes: &[u8]) -> Option<usize> {
    for width in 1..=bytes.len().min(4) {
        if let Ok(text) = std::str::from_utf8(&bytes[bytes.len() - width..]) {
            let mut chars = text.chars();
            let value = chars.next()?;
            if chars.next().is_none() {
                return value.is_whitespace().then_some(width);
            }
        }
    }
    None
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, BrowserCookieError> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, BrowserCookieError> {
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|_| BrowserCookieError::InvalidFormat)
}

#[cfg(unix)]
fn base_native(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        return b".".to_vec();
    }
    let mut end = path.len();
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return b"/".to_vec();
    }
    let start = path[..end]
        .iter()
        .rposition(|byte| *byte == b'/')
        .map_or(0, |index| index + 1);
    path[start..end].to_vec()
}

#[cfg(all(not(unix), not(windows)))]
fn base_native(bytes: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    Path::new(text)
        .file_name()
        .map_or_else(|| b".".to_vec(), |name| name.as_encoded_bytes().to_vec())
}

#[cfg(unix)]
fn join_native(root: &Path, child: &Path) -> PathBuf {
    use std::{
        ffi::OsString,
        os::unix::ffi::{OsStrExt, OsStringExt},
    };
    let root = root.as_os_str().as_bytes();
    let child = child.as_os_str().as_bytes();
    let mut joined = Vec::with_capacity(root.len() + child.len() + 1);
    if !root.is_empty() {
        joined.extend_from_slice(root);
        if !child.is_empty() {
            joined.push(b'/');
        }
    }
    joined.extend_from_slice(child);
    let absolute = joined.starts_with(b"/");
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in joined.split(|byte| *byte == b'/') {
        match part {
            b"" | b"." => {}
            b".." => {
                if parts.last().is_some_and(|part| *part != b"..") {
                    parts.pop();
                } else if !absolute {
                    parts.push(part);
                }
            }
            _ => parts.push(part),
        }
    }
    let mut cleaned = Vec::new();
    if absolute {
        cleaned.push(b'/');
    }
    for part in parts {
        if !cleaned.is_empty() && cleaned.last() != Some(&b'/') {
            cleaned.push(b'/');
        }
        cleaned.extend_from_slice(part);
    }
    if cleaned.is_empty() {
        cleaned.push(b'.');
    }
    PathBuf::from(OsString::from_vec(cleaned))
}

#[cfg(all(not(unix), not(windows)))]
fn join_native(root: &Path, child: &Path) -> PathBuf {
    let mut joined = root.to_path_buf();
    joined.push(child);
    let mut cleaned = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
                if cleaned.file_name().is_some_and(|name| name != "..") =>
            {
                cleaned.pop();
            }
            _ => cleaned.push(component.as_os_str()),
        }
    }
    if cleaned.as_os_str().is_empty() {
        cleaned.push(".");
    }
    cleaned
}

#[cfg(windows)]
fn windows_separator(byte: u8) -> bool {
    byte == b'\\' || byte == b'/'
}

#[cfg(windows)]
fn windows_prefix(path: &[u8], prefix: &[u8]) -> bool {
    path.len() >= prefix.len()
        && prefix.iter().zip(path).all(|(expected, actual)| {
            if windows_separator(*expected) {
                windows_separator(*actual)
            } else {
                expected.eq_ignore_ascii_case(actual)
            }
        })
        && (path.len() == prefix.len() || windows_separator(path[prefix.len()]))
}

#[cfg(windows)]
fn windows_unc_length(path: &[u8], prefix: usize) -> usize {
    path.iter()
        .enumerate()
        .skip(prefix)
        .filter(|(_, byte)| windows_separator(**byte))
        .nth(1)
        .map_or(path.len(), |(index, _)| index)
}

#[cfg(windows)]
fn windows_volume_length(path: &[u8]) -> usize {
    if path.len() >= 2 && path[1] == b':' {
        return 2;
    }
    if path.is_empty() || !windows_separator(path[0]) {
        return 0;
    }
    let length = if windows_prefix(path, br"\\.")
        || windows_prefix(path, br"\\?")
        || windows_prefix(path, br"\??")
    {
        if path.len() == 3 {
            return 3;
        }
        if windows_prefix(&path[4..], b"UNC") {
            windows_unc_length(path, 8)
        } else {
            path.iter()
                .enumerate()
                .skip(4)
                .find(|(_, byte)| windows_separator(**byte))
                .map_or(path.len(), |(index, _)| index)
        }
    } else if path.len() >= 2 && windows_separator(path[1]) {
        windows_unc_length(path, 2)
    } else {
        return 0;
    };
    if path[..length]
        .split(|byte| windows_separator(*byte))
        .any(|part| part == b"..")
    {
        0
    } else {
        length
    }
}

#[cfg(windows)]
fn base_native(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        return b".".to_vec();
    }
    let mut end = path.len();
    while end > 0 && windows_separator(path[end - 1]) {
        end -= 1;
    }
    let path = &path[..end];
    let path = &path[windows_volume_length(path)..];
    let start = path
        .iter()
        .rposition(|byte| windows_separator(*byte))
        .map_or(0, |index| index + 1);
    if path[start..].is_empty() {
        b"\\".to_vec()
    } else {
        path[start..].to_vec()
    }
}

#[cfg(windows)]
fn join_native(root: &Path, child: &Path) -> PathBuf {
    let mut joined = Vec::new();
    for element in [
        root.as_os_str().as_encoded_bytes(),
        child.as_os_str().as_encoded_bytes(),
    ] {
        let mut element = element;
        if let Some(last) = joined.last().copied() {
            if windows_separator(last) {
                while element.first().is_some_and(|byte| windows_separator(*byte)) {
                    element = &element[1..];
                }
                if joined.len() == 1
                    && element.starts_with(b"??")
                    && (element.len() == 2 || windows_separator(element[2]))
                {
                    joined.extend_from_slice(b".\\");
                }
            } else if last != b':' {
                joined.push(b'\\');
            }
        }
        joined.extend_from_slice(element);
    }
    let volume = windows_volume_length(&joined);
    let path = &joined[volume..];
    let mut cleaned = joined[..volume].to_vec();
    if path.is_empty() {
        if !(volume > 1 && windows_separator(joined[0]) && windows_separator(joined[1])) {
            cleaned.push(b'.');
        }
    } else {
        let rooted = windows_separator(path[0]);
        let mut parts: Vec<&[u8]> = Vec::new();
        for part in path.split(|byte| windows_separator(*byte)) {
            match part {
                b"" | b"." => {}
                b".." => {
                    if parts.last().is_some_and(|part| *part != b"..") {
                        parts.pop();
                    } else if !rooted {
                        parts.push(part);
                    }
                }
                _ => parts.push(part),
            }
        }
        let mut tail = Vec::new();
        if rooted {
            tail.push(b'\\');
        }
        for part in parts {
            if !tail.is_empty() && tail.last() != Some(&b'\\') {
                tail.push(b'\\');
            }
            tail.extend_from_slice(part);
        }
        if tail.is_empty() {
            tail.push(b'.');
        }
        if volume == 0 && tail != path {
            if tail
                .split(|byte| windows_separator(*byte))
                .next()
                .unwrap()
                .contains(&b':')
            {
                tail.splice(..0, b".\\".iter().copied());
            } else if tail.len() >= 3 && windows_separator(tail[0]) && tail[1..3] == *b"??" {
                tail.splice(..0, b"\\.".iter().copied());
            }
        }
        cleaned.extend_from_slice(&tail);
    }
    for byte in &mut cleaned {
        if *byte == b'/' {
            *byte = b'\\';
        }
    }
    // Native encoded components remain whole; only ASCII separators and dot components are changed.
    PathBuf::from(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(cleaned) })
}

pub struct SafariCookieProvider {
    paths: Vec<PathBuf>,
    files: Arc<dyn BrowserFiles>,
}

impl SafariCookieProvider {
    pub fn new(paths: Vec<PathBuf>, files: Arc<dyn BrowserFiles>) -> Self {
        Self { paths, files }
    }

    fn find_cookie_file(&self) -> Result<Option<&Path>, BrowserCookieError> {
        for path in &self.paths {
            match self.files.metadata(path) {
                Ok(metadata) if !metadata.is_dir => return Ok(Some(path)),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    return Err(BrowserCookieError::PermissionDenied);
                }
                Err(_) => {}
            }
        }
        Ok(None)
    }

    pub fn discover_profiles(
        &self,
        context: &Context,
    ) -> Result<Vec<BrowserProfile>, BrowserCookieError> {
        if let Some(error) = context.error() {
            return Err(BrowserCookieError::Context(error));
        }
        let path = self
            .find_cookie_file()?
            .ok_or(BrowserCookieError::NotInstalled)?;
        Ok(vec![BrowserProfile {
            id: b"Default".to_vec(),
            name: b"Default".to_vec(),
            path: path.into(),
        }])
    }

    pub fn read(
        &self,
        _context: &Context,
        query: &CookieQuery,
        profile_id: &[u8],
    ) -> Result<Vec<SecretBytes>, BrowserCookieError> {
        if profile_id != b"Default" {
            return Err(BrowserCookieError::ProfileNotFound);
        }
        let path = self
            .find_cookie_file()?
            .or_else(|| self.paths.first().map(PathBuf::as_path))
            .ok_or(BrowserCookieError::DatabaseNotFound)?;
        let data = self.files.read_file(path).map_err(|error| {
            if error.kind() == io::ErrorKind::PermissionDenied {
                BrowserCookieError::PermissionDenied
            } else {
                BrowserCookieError::DatabaseNotFound
            }
        })?;
        let cookies = parse_binary_cookies(&data)?;
        let mut secrets = Vec::new();
        for cookie in cookies {
            if cookie.name != query.name().as_bytes()
                || trim_one_dot(cookie.domain) != trim_one_dot(query.host().as_bytes())
            {
                continue;
            }
            if std::str::from_utf8(cookie.value).is_err() {
                return Err(BrowserCookieError::EncryptedCookieUnsupported);
            }
            secrets.push(SecretBytes::new(cookie.value.into()));
        }
        Ok(secrets)
    }
}

fn trim_one_dot(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b".").unwrap_or(bytes)
}

struct SafariCookie<'a> {
    domain: &'a [u8],
    name: &'a [u8],
    value: &'a [u8],
}

fn u32_be(bytes: &[u8]) -> usize {
    u32::from_be_bytes(bytes.try_into().unwrap()) as usize
}

fn parse_binary_cookies(data: &[u8]) -> Result<Vec<SafariCookie<'_>>, BrowserCookieError> {
    if data.len() < 8 || &data[..4] != b"cook" {
        return Err(BrowserCookieError::InvalidFormat);
    }
    let pages = u32_be(&data[4..8]);
    if pages == 0 {
        return Ok(Vec::new());
    }
    if pages > (data.len() - 8) / 4 {
        return Err(BrowserCookieError::InvalidFormat);
    }
    let mut cookies = Vec::new();
    let mut offset = 8;
    for _ in 0..pages {
        // Go reads the next interleaved size unchecked; validating it would erase its frozen panic.
        let page_size = u32_be(&data[offset..offset + 4]);
        offset += 4;
        if page_size < 16 || offset + page_size > data.len() {
            return Err(BrowserCookieError::InvalidFormat);
        }
        let page = &data[offset..offset + page_size];
        offset += page_size;
        cookies.extend(parse_safari_page(page)?);
    }
    Ok(cookies)
}

fn parse_safari_page(page: &[u8]) -> Result<Vec<SafariCookie<'_>>, BrowserCookieError> {
    if page.len() < 16 {
        return Err(BrowserCookieError::InvalidFormat);
    }
    let header_size = u32_be(&page[..4]);
    let count = u32_be(&page[4..8]);
    let page_start = u32_be(&page[8..12]);
    if header_size < 16 || page_start < 16 || header_size + 4 * count > page.len() {
        return Err(BrowserCookieError::InvalidFormat);
    }
    let mut cookies = Vec::new();
    for index in 0..count {
        let table_offset = header_size + 4 * index;
        let offset = u32_be(&page[table_offset..table_offset + 4]) + page_start;
        if offset >= page.len() {
            return Err(BrowserCookieError::InvalidFormat);
        }
        cookies.push(parse_safari_cookie(&page[offset..])?);
    }
    Ok(cookies)
}

fn parse_safari_cookie(record: &[u8]) -> Result<SafariCookie<'_>, BrowserCookieError> {
    if record.len() < 4 {
        return Err(BrowserCookieError::InvalidFormat);
    }
    let size = u16::from_be_bytes(record[..2].try_into().unwrap()) as usize;
    if size < 4 || size > record.len() {
        return Err(BrowserCookieError::InvalidFormat);
    }
    let record = &record[..size];
    let mut position = 4;
    let domain = read_safari_field(record, &mut position)?;
    let name = read_safari_field(record, &mut position)?;
    let _path = read_safari_field(record, &mut position)?;
    let value = read_safari_field(record, &mut position)?;
    if position + 12 >= record.len() {
        return Err(BrowserCookieError::InvalidFormat);
    }
    Ok(SafariCookie {
        domain,
        name,
        value,
    })
}

fn read_safari_field<'a>(
    record: &'a [u8],
    position: &mut usize,
) -> Result<&'a [u8], BrowserCookieError> {
    let length = *record
        .get(*position)
        .ok_or(BrowserCookieError::InvalidFormat)? as usize;
    *position += 1;
    if *position + length > record.len() {
        return Err(BrowserCookieError::InvalidFormat);
    }
    let value = &record[*position..*position + length];
    *position += length;
    Ok(value)
}
