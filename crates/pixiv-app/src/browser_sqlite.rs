use crate::{
    browser_cookies::BrowserCookieError,
    host_context::ContextHostProcess,
    host_process::{HostProcessError, ProcessStdio, SystemHostProcess},
    lifecycle::Context,
};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::Path,
    sync::Arc,
};

pub type SqliteRows = Vec<Vec<Vec<u8>>>;

pub struct SqliteCommand {
    process: Arc<dyn ContextHostProcess>,
}

impl Default for SqliteCommand {
    fn default() -> Self {
        Self::new(Arc::new(SystemHostProcess))
    }
}

impl SqliteCommand {
    pub fn new(process: Arc<dyn ContextHostProcess>) -> Self {
        Self { process }
    }

    pub fn query(
        &self,
        context: &Context,
        database: &Path,
        sql: &str,
        params: &BTreeMap<String, String>,
    ) -> Result<SqliteRows, BrowserCookieError> {
        if database
            .as_os_str()
            .to_str()
            .is_some_and(|path| path.trim().is_empty())
            || sql.trim().is_empty()
        {
            return Err(BrowserCookieError::QueryFailed);
        }
        let mut args: Vec<OsString> = ["-readonly", "-noheader", "-csv", "-newline", "\n"]
            .map(OsString::from)
            .into();
        for (name, value) in params {
            if !safe_param(name) || !safe_param(value) {
                return Err(BrowserCookieError::QueryFailed);
            }
            args.push("-cmd".into());
            args.push(format!(".parameter set {name} {value}").into());
        }
        args.push(database.as_os_str().to_owned());
        args.push(sql.into());
        match self
            .process
            .run_context(context, OsStr::new("sqlite3"), &args, ProcessStdio::Capture)
        {
            Ok(output) => parse_csv(&output.stdout),
            Err(error) => Err(classify_error(context, &error)),
        }
    }
}

fn safe_param(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_@:$".contains(&byte))
}

fn process_stderr(error: &HostProcessError) -> &[u8] {
    match error {
        HostProcessError::Exit { output, .. } => &output.stderr,
        HostProcessError::Captured { stderr, .. } => stderr,
        _ => &[],
    }
}

fn process_cause(error: &HostProcessError) -> &HostProcessError {
    match error {
        HostProcessError::Captured { source, .. } => process_cause(source),
        _ => error,
    }
}

fn classify_error(context: &Context, error: &HostProcessError) -> BrowserCookieError {
    if let Some(reason) = context.error() {
        return BrowserCookieError::Context(reason);
    }
    let cause = process_cause(error);
    if cause.is_not_found() {
        return BrowserCookieError::SqliteUnavailable;
    }
    let stderr = String::from_utf8_lossy(process_stderr(error)).to_lowercase();
    if stderr.contains("permission denied") || stderr.contains("not authorized") {
        return BrowserCookieError::PermissionDenied;
    }
    if let HostProcessError::Exit { output, .. } = cause
        && (output.status.code() == Some(5) || stderr.contains("locked"))
    {
        return BrowserCookieError::DatabaseLocked;
    }
    BrowserCookieError::QueryFailed
}

fn normalize_lines(bytes: &[u8]) -> Vec<u8> {
    let mut normalized = Vec::with_capacity(bytes.len());
    for (index, &byte) in bytes.iter().enumerate() {
        if byte == b'\r' && (index + 1 == bytes.len() || bytes[index + 1] == b'\n') {
            continue;
        }
        normalized.push(byte);
    }
    normalized
}

fn parse_csv(bytes: &[u8]) -> Result<SqliteRows, BrowserCookieError> {
    let bytes = normalize_lines(bytes);
    let mut cursor = 0;
    let mut rows = Vec::new();
    while cursor < bytes.len() {
        if bytes[cursor] == b'\n' {
            cursor += 1;
            continue;
        }
        let mut row = Vec::new();
        loop {
            let field = if bytes.get(cursor) == Some(&b'"') {
                cursor += 1;
                let mut field = Vec::new();
                loop {
                    let Some(&byte) = bytes.get(cursor) else {
                        return Err(BrowserCookieError::QueryFailed);
                    };
                    cursor += 1;
                    if byte == b'"' {
                        if bytes.get(cursor) == Some(&b'"') {
                            cursor += 1;
                            field.push(b'"');
                        } else {
                            break;
                        }
                    } else {
                        field.push(byte);
                    }
                }
                field
            } else {
                let start = cursor;
                while let Some(&byte) = bytes.get(cursor) {
                    if byte == b',' || byte == b'\n' {
                        break;
                    }
                    if byte == b'"' {
                        return Err(BrowserCookieError::QueryFailed);
                    }
                    cursor += 1;
                }
                bytes[start..cursor].to_vec()
            };
            row.push(field);
            match bytes.get(cursor) {
                Some(b',') => cursor += 1,
                Some(b'\n') => {
                    cursor += 1;
                    break;
                }
                None => break,
                _ => return Err(BrowserCookieError::QueryFailed),
            }
        }
        rows.push(row);
    }
    Ok(rows)
}
