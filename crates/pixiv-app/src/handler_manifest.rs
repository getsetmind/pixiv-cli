use crate::{
    auth_bundle::{escape_json_html, folded, normalize_strings},
    callback_handler::handler_manifest_path,
    config::{ConfigError, private_file},
    handoff_state::validate_json_depth,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

pub const HANDLER_MANIFEST_FILENAME: &str = "handler-manifest.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HandlerManifest {
    pub version: i64,
    pub executable_path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub home_directory: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub previous_handler: String,
    #[serde(skip_serializing_if = "empty_slice")]
    pub linux_mime_snapshots: Option<Vec<HandlerFileSnapshot>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HandlerFileSnapshot {
    pub path: String,
    pub exists: bool,
    pub mode: u32,
    #[serde(
        skip_serializing_if = "empty_slice",
        serialize_with = "serialize_content"
    )]
    pub content: Option<Vec<u8>>,
}

fn empty_slice<T>(value: &Option<Vec<T>>) -> bool {
    value.as_ref().is_none_or(Vec::is_empty)
}
fn serialize_content<S: Serializer>(
    value: &Option<Vec<u8>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&STANDARD.encode(value.as_deref().unwrap_or_default()))
}

#[derive(Debug)]
pub enum HandlerManifestError {
    Invalid,
    Io(io::Error),
    Storage(ConfigError),
}
impl fmt::Display for HandlerManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => f.write_str("Pixiv URL handler manifest is invalid"),
            Self::Io(error) => error.fmt(f),
            Self::Storage(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for HandlerManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid => None,
            Self::Io(error) => Some(error),
            Self::Storage(error) => Some(error),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HandlerManifestStore {
    path: PathBuf,
}
impl HandlerManifestStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn default_path() -> Result<Self, HandlerManifestError> {
        handler_manifest_path()
            .map(Self::new)
            .map_err(HandlerManifestError::Io)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn load(&self) -> Result<Option<HandlerManifest>, HandlerManifestError> {
        load_handler_manifest_at(&self.path)
    }
    pub fn save(&self, manifest: &HandlerManifest) -> Result<(), HandlerManifestError> {
        save_handler_manifest_at(&self.path, manifest)
    }
    pub fn remove(&self) -> Result<(), HandlerManifestError> {
        remove_handler_manifest_at(&self.path)
    }
}

pub fn load_handler_manifest_at(
    path: &Path,
) -> Result<Option<HandlerManifest>, HandlerManifestError> {
    match fs::read(path) {
        Ok(body) => decode(&body).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(HandlerManifestError::Io(error)),
    }
}
pub fn save_handler_manifest_at(
    path: &Path,
    manifest: &HandlerManifest,
) -> Result<(), HandlerManifestError> {
    private_file::write(path, &encode(manifest)?).map_err(HandlerManifestError::Storage)
}
pub fn remove_handler_manifest_at(path: &Path) -> Result<(), HandlerManifestError> {
    private_file::remove_if_exists(path).map_err(HandlerManifestError::Io)
}

pub fn encode(manifest: &HandlerManifest) -> Result<Vec<u8>, HandlerManifestError> {
    let mut manifest = manifest.clone();
    if manifest.version == 0 {
        manifest.version = 1;
    }
    encode_unpromoted(&manifest)
}
pub(crate) fn encode_unpromoted(
    manifest: &HandlerManifest,
) -> Result<Vec<u8>, HandlerManifestError> {
    serde_json::to_string(manifest)
        .map(|body| escape_json_html(body).into_bytes())
        .map_err(|_| HandlerManifestError::Invalid)
}
pub fn decode(body: &[u8]) -> Result<HandlerManifest, HandlerManifestError> {
    let normalized = normalize_strings(body);
    validate_json_depth(&normalized).map_err(|_| HandlerManifestError::Invalid)?;
    let manifest: HandlerManifest =
        serde_json::from_str(&normalized).map_err(|_| HandlerManifestError::Invalid)?;
    if manifest.version != 1 || manifest.executable_path.is_empty() {
        return Err(HandlerManifestError::Invalid);
    }
    Ok(manifest)
}

type Raw = Box<serde_json::value::RawValue>;
struct Fields(Vec<(String, Raw)>);
impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Fields;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("object or null")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Fields, E> {
                Ok(Fields(Vec::new()))
            }
            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Fields, M::Error> {
                let mut fields = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    fields.push((key, map.next_value()?));
                }
                Ok(Fields(fields))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
fn scalar<T: for<'de> Deserialize<'de>>(
    raw: &Raw,
    target: &mut T,
) -> Result<(), serde_json::Error> {
    if let Some(value) = serde_json::from_str::<Option<T>>(raw.get())? {
        *target = value;
    }
    Ok(())
}
#[derive(Default)]
struct SnapshotDecoder {
    value: HandlerFileSnapshot,
    bytes: Vec<u8>,
    content_length: Option<usize>,
}
impl SnapshotDecoder {
    fn finish(mut self) -> HandlerFileSnapshot {
        self.value.content = self.content_length.map(|length| {
            self.bytes.truncate(length);
            self.bytes
        });
        self.value
    }
    fn content(&mut self, raw: &Raw) -> Result<(), String> {
        if raw.get() == "null" {
            self.bytes.clear();
            self.content_length = None;
        } else if raw.get().starts_with('"') {
            let text: String =
                serde_json::from_str(raw.get()).map_err(|error| error.to_string())?;
            let compact: String = text.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
            let decoder = base64::engine::GeneralPurpose::new(
                &base64::alphabet::STANDARD,
                base64::engine::GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
            );
            self.bytes = decoder.decode(compact).map_err(|error| error.to_string())?;
            self.content_length = Some(self.bytes.len());
        } else {
            let bytes: Vec<Option<u8>> =
                serde_json::from_str(raw.get()).map_err(|error| error.to_string())?;
            if bytes.is_empty() {
                self.bytes.clear();
            }
            if bytes.len() > self.bytes.len() {
                self.bytes.resize(bytes.len(), 0);
            }
            for (target, byte) in self.bytes.iter_mut().zip(&bytes) {
                if let Some(byte) = byte {
                    *target = *byte;
                }
            }
            self.content_length = Some(bytes.len());
        }
        Ok(())
    }
    fn update(&mut self, raw: &Raw) -> Result<(), String> {
        for (key, raw) in serde_json::from_str::<Fields>(raw.get())
            .map_err(|error| error.to_string())?
            .0
        {
            if folded(&key, "path") {
                scalar(&raw, &mut self.value.path).map_err(|error| error.to_string())?;
            } else if folded(&key, "exists") {
                scalar(&raw, &mut self.value.exists).map_err(|error| error.to_string())?;
            } else if folded(&key, "mode") {
                scalar(&raw, &mut self.value.mode).map_err(|error| error.to_string())?;
            } else if folded(&key, "content") {
                self.content(&raw)?;
            }
        }
        Ok(())
    }
}
impl<'de> Deserialize<'de> for HandlerFileSnapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Raw::deserialize(deserializer)?;
        let mut result = SnapshotDecoder::default();
        result.update(&raw).map_err(de::Error::custom)?;
        Ok(result.finish())
    }
}
impl<'de> Deserialize<'de> for HandlerManifest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = Fields::deserialize(deserializer)?;
        let mut manifest = Self::default();
        let mut snapshots = Vec::new();
        let mut snapshot_length = None;
        for (key, raw) in fields.0 {
            if folded(&key, "version") {
                scalar(&raw, &mut manifest.version).map_err(de::Error::custom)?;
            } else if folded(&key, "executable_path") {
                scalar(&raw, &mut manifest.executable_path).map_err(de::Error::custom)?;
            } else if folded(&key, "home_directory") {
                scalar(&raw, &mut manifest.home_directory).map_err(de::Error::custom)?;
            } else if folded(&key, "previous_handler") {
                scalar(&raw, &mut manifest.previous_handler).map_err(de::Error::custom)?;
            } else if folded(&key, "linux_mime_snapshots") {
                let values: Option<Vec<Raw>> =
                    serde_json::from_str(raw.get()).map_err(de::Error::custom)?;
                match values {
                    None => {
                        snapshots.clear();
                        snapshot_length = None;
                    }
                    Some(values) => {
                        if values.is_empty() {
                            snapshots.clear();
                        }
                        if values.len() > snapshots.len() {
                            snapshots.resize_with(values.len(), SnapshotDecoder::default);
                        }
                        for (raw, existing) in values.iter().zip(&mut snapshots) {
                            existing.update(raw).map_err(de::Error::custom)?;
                        }
                        snapshot_length = Some(values.len());
                    }
                }
            }
        }
        manifest.linux_mime_snapshots = snapshot_length.map(|length| {
            snapshots
                .into_iter()
                .take(length)
                .map(SnapshotDecoder::finish)
                .collect()
        });
        Ok(manifest)
    }
}
