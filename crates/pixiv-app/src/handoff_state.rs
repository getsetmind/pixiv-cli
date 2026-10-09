use crate::config::{ConfigError, private_file};
use crate::handoff_protocol::{RemoteLoginStart, canonical_relay_origin};
use crate::private_lock;
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, PartialEq, Eq, Serialize, Default)]
pub struct ActiveRemoteLogin {
    pub version: i64,
    pub origin: String,
    pub session_id: String,
    pub proof: String,
}

impl fmt::Debug for ActiveRemoteLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActiveRemoteLogin")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum HandoffStateError {
    NoActiveRemoteLogin,
    Unreadable,
    Invalid,
    InvalidHandoff,
    ClearFailed,
    Storage(ConfigError),
    Cancelled,
}

impl fmt::Display for HandoffStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoActiveRemoteLogin => "no active remote login handoff",
            Self::Unreadable => "could not read active remote login handoff",
            Self::Invalid => "active remote login handoff is invalid",
            Self::InvalidHandoff => "invalid remote login handoff",
            Self::ClearFailed => "could not clear active remote login handoff",
            Self::Storage(error) => return fmt::Display::fmt(error, f),
            Self::Cancelled => "context canceled",
        })
    }
}
impl std::error::Error for HandoffStateError {}
impl From<ConfigError> for HandoffStateError {
    fn from(error: ConfigError) -> Self {
        Self::Storage(error)
    }
}

#[derive(Clone, Debug)]
pub struct HandoffState {
    path: PathBuf,
}
impl HandoffState {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn save(&self, session: &ActiveRemoteLogin) -> Result<(), HandoffStateError> {
        private_lock::with_private_lock(&self.path, None, || {
            save_active_remote_login_at(&self.path, session)
        })
    }
    pub fn load(&self) -> Result<ActiveRemoteLogin, HandoffStateError> {
        private_lock::with_private_lock(&self.path, None, || {
            load_active_remote_login_at(&self.path)
        })
    }
    pub fn clear_if_matches(&self, expected: &ActiveRemoteLogin) -> Result<(), HandoffStateError> {
        private_lock::with_private_lock(&self.path, None, || {
            let active = match load_active_remote_login_at(&self.path) {
                Ok(active) => active,
                Err(HandoffStateError::NoActiveRemoteLogin) => return Ok(()),
                Err(error) => return Err(error),
            };
            if active != *expected {
                return Ok(());
            }
            match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(ConfigError::Io(error).into()),
            }
        })
    }
    pub fn clear_remote_login_handoff(
        &self,
        start: &RemoteLoginStart,
    ) -> Result<(), HandoffStateError> {
        let origin =
            canonical_relay_origin(&start.origin).map_err(|_| HandoffStateError::InvalidHandoff)?;
        if go_blank(&start.session_id) || go_blank(&start.proof) {
            return Err(HandoffStateError::InvalidHandoff);
        }
        self.clear_if_matches(&ActiveRemoteLogin {
            version: 1,
            origin,
            session_id: start.session_id.clone(),
            proof: start.proof.clone(),
        })
        .map_err(|_| HandoffStateError::ClearFailed)
    }
}

pub fn save_active_remote_login_at(
    path: &Path,
    session: &ActiveRemoteLogin,
) -> Result<(), HandoffStateError> {
    let body = crate::auth_bundle::escape_json_html(
        serde_json::to_string(session).map_err(|_| HandoffStateError::Invalid)?,
    );
    private_file::write(path, body.as_bytes()).map_err(Into::into)
}

pub fn load_active_remote_login_at(path: &Path) -> Result<ActiveRemoteLogin, HandoffStateError> {
    let body = fs::read(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            HandoffStateError::NoActiveRemoteLogin
        } else {
            HandoffStateError::Unreadable
        }
    })?;
    let normalized = crate::auth_bundle::normalize_strings(&body);
    validate_json_depth(&normalized)?;
    let mut session: ActiveRemoteLogin =
        serde_json::from_str(&normalized).map_err(|_| HandoffStateError::Invalid)?;
    if session.version != 1 || go_blank(&session.session_id) || go_blank(&session.proof) {
        return Err(HandoffStateError::Invalid);
    }
    session.origin =
        canonical_relay_origin(&session.origin).map_err(|_| HandoffStateError::Invalid)?;
    Ok(session)
}

pub(crate) fn validate_json_depth(body: &str) -> Result<(), HandoffStateError> {
    let mut depth = 0_u32;
    let mut in_string = false;
    let mut escaped = false;
    for byte in body.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 10000 {
                        return Err(HandoffStateError::Invalid);
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}

fn go_blank(value: &str) -> bool {
    value.chars().all(|c| matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{0085}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'))
}

fn field_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c == '\u{017f}' {
                's'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

impl<'de> Deserialize<'de> for ActiveRemoteLogin {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = ActiveRemoteLogin;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("active remote login object")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(ActiveRemoteLogin::default())
            }
            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut active = ActiveRemoteLogin::default();
                while let Some(name) = map.next_key::<String>()? {
                    match field_name(&name).as_str() {
                        "version" => {
                            if let Some(value) = map.next_value::<Option<i64>>()? {
                                active.version = value;
                            }
                        }
                        "origin" => {
                            if let Some(value) = map.next_value::<Option<String>>()? {
                                active.origin = value;
                            }
                        }
                        "session_id" => {
                            if let Some(value) = map.next_value::<Option<String>>()? {
                                active.session_id = value;
                            }
                        }
                        "proof" => {
                            if let Some(value) = map.next_value::<Option<String>>()? {
                                active.proof = value;
                            }
                        }
                        _ => {
                            let _ = map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(active)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
