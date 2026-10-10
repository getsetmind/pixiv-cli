use crate::{
    browser_cookies::{BrowserCookieError, BrowserFiles, SecretBytes},
    browser_dpapi::WindowsDpapi,
    browser_secrets::{BrowserSecretError, Keychain, SecretService},
    browser_sqlite::SqliteRows,
    host_context::ContextHostProcess,
    host_process::HostPlatform,
    lifecycle::Context,
};
use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use btls::{
    hash::MessageDigest,
    pkcs5::pbkdf2_hmac,
    symm::{Cipher, Crypter, Mode, decrypt_aead},
};
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use sha2::{Digest, Sha256};
use std::{fmt, io, path::PathBuf, sync::Arc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChromiumKind {
    Chrome,
    Edge,
}

pub trait ChromiumKeySource: Send + Sync {
    fn keys(&self, context: &Context) -> Result<Vec<SecretBytes>, BrowserCookieError>;
    fn unprotect_legacy(
        &self,
        context: &Context,
        encrypted: &[u8],
    ) -> Result<SecretBytes, BrowserCookieError>;
}

pub struct PlatformChromiumKeyStore {
    kind: ChromiumKind,
    root: PathBuf,
    files: Arc<dyn BrowserFiles>,
    process: Arc<dyn ContextHostProcess>,
    dpapi: Arc<WindowsDpapi>,
}
impl PlatformChromiumKeyStore {
    pub fn new(
        kind: ChromiumKind,
        root: PathBuf,
        files: Arc<dyn BrowserFiles>,
        process: Arc<dyn ContextHostProcess>,
        dpapi: Arc<WindowsDpapi>,
    ) -> Self {
        Self {
            kind,
            root,
            files,
            process,
            dpapi,
        }
    }

    fn local_state_key(&self) -> Result<Option<Vec<u8>>, BrowserCookieError> {
        let body = match self.files.read_file(&self.root.join("Local State")) {
            Ok(body) => body,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return Err(BrowserCookieError::PermissionDenied);
            }
            Err(_) => return Err(BrowserCookieError::EncryptedFormatUnknown),
        };
        let mut encoded = String::new();
        if !json_depth_supported(&body) {
            return Err(BrowserCookieError::EncryptedFormatUnknown);
        }
        let body = crate::auth_bundle::normalize_strings(&body);
        let mut deserializer = serde_json::Deserializer::from_str(&body);
        StateSeed {
            encoded: &mut encoded,
        }
        .deserialize(&mut deserializer)
        .map_err(|_| BrowserCookieError::EncryptedFormatUnknown)?;
        deserializer
            .end()
            .map_err(|_| BrowserCookieError::EncryptedFormatUnknown)?;
        let encoded = trim_go_space(encoded.as_bytes());
        if encoded.is_empty() {
            return Ok(None);
        }
        let compact: Vec<u8> = encoded
            .iter()
            .copied()
            .filter(|byte| !matches!(byte, b'\r' | b'\n'))
            .collect();
        let padded = GeneralPurpose::new(
            &alphabet::STANDARD,
            GeneralPurposeConfig::new()
                .with_decode_allow_trailing_bits(true)
                .with_decode_padding_mode(DecodePaddingMode::RequireCanonical),
        );
        let raw = GeneralPurpose::new(
            &alphabet::STANDARD,
            GeneralPurposeConfig::new()
                .with_decode_allow_trailing_bits(true)
                .with_decode_padding_mode(DecodePaddingMode::RequireNone),
        );
        let key = padded
            .decode(&compact)
            .or_else(|_| raw.decode(&compact))
            .map_err(|_| BrowserCookieError::EncryptedFormatUnknown)?;
        if key.is_empty() {
            return Err(BrowserCookieError::EncryptedFormatUnknown);
        }
        Ok(Some(key))
    }

    fn unix_keys(
        &self,
        context: &Context,
        platform: HostPlatform,
    ) -> Result<Vec<SecretBytes>, BrowserCookieError> {
        let password = if platform == HostPlatform::Linux {
            let application = match self.kind {
                ChromiumKind::Chrome => "chrome",
                ChromiumKind::Edge => "microsoft-edge",
            };
            SecretService::new(self.process.clone())
                .get_password(context, application)
                .map_err(map_secret_service)?
        } else {
            let (service, account) = match self.kind {
                ChromiumKind::Chrome => ("Chrome Safe Storage", "Chrome"),
                ChromiumKind::Edge => ("Microsoft Edge Safe Storage", "Microsoft Edge"),
            };
            Keychain::new(self.process.clone())
                .get_password(context, service, account)
                .map_err(map_keychain)?
        };
        let candidates = key_candidates(password.as_bytes())?;
        let Some(state_key) = self.local_state_key()? else {
            return Ok(candidates.into_iter().map(SecretBytes::new).collect());
        };
        if has_prefix(&state_key) {
            for candidate in candidates {
                if let Ok(key) = decrypt_gcm(&state_key, &candidate)
                    && !key.is_empty()
                {
                    return Ok(vec![SecretBytes::new(key)]);
                }
            }
            return Err(BrowserCookieError::EncryptedMalformed);
        }
        if state_key.starts_with(b"DPAPI") {
            return Err(BrowserCookieError::EncryptedFormatUnknown);
        }
        Ok(vec![SecretBytes::new(state_key)])
    }
}
impl ChromiumKeySource for PlatformChromiumKeyStore {
    fn keys(&self, context: &Context) -> Result<Vec<SecretBytes>, BrowserCookieError> {
        match self.process.platform() {
            HostPlatform::Linux => self.unix_keys(context, HostPlatform::Linux),
            HostPlatform::Darwin => self.unix_keys(context, HostPlatform::Darwin),
            HostPlatform::Windows => {
                let key = self
                    .local_state_key()?
                    .ok_or(BrowserCookieError::EncryptedValueUnsupported)?;
                let blob = key.strip_prefix(b"DPAPI").unwrap_or(&key);
                Ok(vec![
                    self.dpapi.unprotect(context, blob).map_err(map_dpapi)?,
                ])
            }
            _ => Err(BrowserCookieError::EncryptedValueUnsupported),
        }
    }
    fn unprotect_legacy(
        &self,
        context: &Context,
        encrypted: &[u8],
    ) -> Result<SecretBytes, BrowserCookieError> {
        if self.process.platform() != HostPlatform::Windows {
            return Err(BrowserCookieError::EncryptedValueUnsupported);
        }
        self.dpapi.unprotect(context, encrypted).map_err(map_dpapi)
    }
}

#[derive(Debug)]
pub struct DecodedCookies {
    pub values: Vec<SecretBytes>,
    pub error: Option<BrowserCookieError>,
}

impl DecodedCookies {
    pub fn into_result(self) -> Result<Vec<SecretBytes>, BrowserCookieError> {
        match self.error {
            Some(error) => Err(error),
            None => Ok(self.values),
        }
    }
}

pub struct ChromiumCookieDecoder {
    platform: HostPlatform,
    keys: Arc<dyn ChromiumKeySource>,
}
impl ChromiumCookieDecoder {
    pub fn new(platform: HostPlatform, keys: Arc<dyn ChromiumKeySource>) -> Self {
        Self { platform, keys }
    }

    pub fn decode_value(
        &self,
        context: &Context,
        encrypted: &[u8],
    ) -> Result<SecretBytes, BrowserCookieError> {
        if let Some(reason) = context.error() {
            return Err(BrowserCookieError::Context(reason));
        }
        if !has_prefix(encrypted) {
            if self.platform == HostPlatform::Windows {
                if encrypted.is_empty() {
                    return Err(BrowserCookieError::EncryptedFormatUnknown);
                }
                return self.keys.unprotect_legacy(context, encrypted);
            }
            if encrypted.len() < 32 || !encrypted.len().is_multiple_of(16) {
                return Err(BrowserCookieError::EncryptedFormatUnknown);
            }
            for key in self.keys.keys(context)? {
                if let Ok(plain) = decrypt_legacy(encrypted, key.as_bytes())
                    && has_prefix(&plain)
                {
                    return Ok(SecretBytes::new(plain[32..].to_vec()));
                }
            }
            return Err(BrowserCookieError::EncryptedMalformed);
        }
        for key in self.keys.keys(context)? {
            if let Ok(plain) = decrypt_gcm(encrypted, key.as_bytes())
                .or_else(|_| decrypt_cbc(encrypted, key.as_bytes()))
            {
                return Ok(SecretBytes::new(plain));
            }
        }
        Err(BrowserCookieError::EncryptedMalformed)
    }

    pub fn decode_rows(&self, context: &Context, rows: SqliteRows) -> DecodedCookies {
        let mut outcome = DecodedCookies {
            values: Vec::with_capacity(rows.len()),
            error: None,
        };
        for row in rows {
            match self.decode_row(context, &row) {
                Ok(value) => outcome.values.push(value),
                Err(error) => {
                    outcome.error = Some(error);
                    break;
                }
            }
        }
        outcome
    }

    fn decode_row(
        &self,
        context: &Context,
        row: &[Vec<u8>],
    ) -> Result<SecretBytes, BrowserCookieError> {
        if row.len() < 3 {
            return Err(BrowserCookieError::QueryFailed);
        }
        let encoded = trim_go_space(&row[2]);
        if encoded.is_empty() {
            return Ok(SecretBytes::new(row[1].clone()));
        }
        let encrypted = decode_hex(encoded).ok_or(BrowserCookieError::EncryptedFormatUnknown)?;
        let mut plain = self.decode_value(context, &encrypted)?.into_bytes();
        if plain.len() >= 32 {
            let digest = Sha256::digest(&row[0]);
            if plain[..32] == digest[..] {
                plain.drain(..32);
            }
        }
        Ok(SecretBytes::new(plain))
    }
}

fn map_secret_service(error: BrowserSecretError) -> BrowserCookieError {
    match error {
        BrowserSecretError::NotAvailableOnBuild => BrowserCookieError::SecretServiceUnavailable,
        BrowserSecretError::Context(reason) => BrowserCookieError::Context(reason),
        _ => BrowserCookieError::SecretServiceAccess,
    }
}
fn map_keychain(error: BrowserSecretError) -> BrowserCookieError {
    match error {
        BrowserSecretError::ItemNotFound => BrowserCookieError::KeychainItemNotFound,
        BrowserSecretError::NotAvailableOnBuild => BrowserCookieError::EncryptedValueUnsupported,
        _ => BrowserCookieError::KeychainAccess,
    }
}
fn map_dpapi(error: BrowserSecretError) -> BrowserCookieError {
    match error {
        BrowserSecretError::Context(reason) => BrowserCookieError::Context(reason),
        _ => BrowserCookieError::Dpapi,
    }
}
fn has_prefix(bytes: &[u8]) -> bool {
    bytes.starts_with(b"v10") || bytes.starts_with(b"v11")
}
fn key_candidates(password: &[u8]) -> Result<Vec<Vec<u8>>, BrowserCookieError> {
    let mut legacy = password.to_vec();
    for _ in 0..1000 {
        let mut digest = Sha256::new();
        digest.update(&legacy);
        digest.update(b"peanuts");
        legacy = digest.finalize().to_vec();
    }
    let mut candidates = vec![legacy];
    for (salt, iterations) in [(b"saltysalt".as_slice(), 1003), (b"peanuts".as_slice(), 1)] {
        let mut key = vec![0; 16];
        pbkdf2_hmac(password, salt, iterations, MessageDigest::sha1(), &mut key)
            .map_err(|_| BrowserCookieError::EncryptedMalformed)?;
        if !candidates.contains(&key) {
            candidates.push(key);
        }
    }
    Ok(candidates)
}
fn gcm_cipher(length: usize) -> Result<Cipher, BrowserCookieError> {
    match length {
        16 => Ok(Cipher::aes_128_gcm()),
        24 => Ok(Cipher::aes_192_gcm()),
        32 => Ok(Cipher::aes_256_gcm()),
        _ => Err(BrowserCookieError::EncryptedMalformed),
    }
}
fn cbc_cipher(length: usize) -> Result<Cipher, BrowserCookieError> {
    match length {
        16 => Ok(Cipher::aes_128_cbc()),
        24 => Ok(Cipher::aes_192_cbc()),
        32 => Ok(Cipher::aes_256_cbc()),
        _ => Err(BrowserCookieError::EncryptedMalformed),
    }
}
fn decrypt_gcm(encrypted: &[u8], key: &[u8]) -> Result<Vec<u8>, BrowserCookieError> {
    if encrypted.len() < 31 || !has_prefix(encrypted) {
        return Err(BrowserCookieError::EncryptedMalformed);
    }
    let tag = encrypted.len() - 16;
    decrypt_aead(
        gcm_cipher(key.len())?,
        key,
        Some(&encrypted[3..15]),
        &[],
        &encrypted[15..tag],
        &encrypted[tag..],
    )
    .map_err(|_| BrowserCookieError::EncryptedMalformed)
}
fn decrypt_cbc(encrypted: &[u8], key: &[u8]) -> Result<Vec<u8>, BrowserCookieError> {
    if encrypted.len() < 19 || !has_prefix(encrypted) {
        return Err(BrowserCookieError::EncryptedMalformed);
    }
    let key = &key[..key.len().min(32)];
    let mut plain = decrypt_blocks(&encrypted[3..], key, &[b' '; 16])?;
    let padding = usize::from(*plain.last().ok_or(BrowserCookieError::EncryptedMalformed)?);
    if !(1..=16).contains(&padding)
        || padding > plain.len()
        || plain[plain.len() - padding..]
            .iter()
            .any(|value| usize::from(*value) != padding)
    {
        return Err(BrowserCookieError::EncryptedMalformed);
    }
    plain.truncate(plain.len() - padding);
    Ok(plain)
}
fn decrypt_legacy(encrypted: &[u8], key: &[u8]) -> Result<Vec<u8>, BrowserCookieError> {
    if key.len() < 16 {
        return Err(BrowserCookieError::EncryptedMalformed);
    }
    let plain = decrypt_blocks(encrypted, &key[..16], &[0; 16])?;
    if plain.len() < 32 {
        return Err(BrowserCookieError::EncryptedMalformed);
    }
    Ok(plain)
}
fn decrypt_blocks(
    encrypted: &[u8],
    key: &[u8],
    iv: &[u8; 16],
) -> Result<Vec<u8>, BrowserCookieError> {
    if encrypted.is_empty() || !encrypted.len().is_multiple_of(16) {
        return Err(BrowserCookieError::EncryptedMalformed);
    }
    let mut crypter = Crypter::new(cbc_cipher(key.len())?, Mode::Decrypt, key, Some(iv))
        .map_err(|_| BrowserCookieError::EncryptedMalformed)?;
    crypter.pad(false);
    let mut plain = vec![0; encrypted.len() + 16];
    let count = crypter
        .update(encrypted, &mut plain)
        .map_err(|_| BrowserCookieError::EncryptedMalformed)?;
    let rest = crypter
        .finalize(&mut plain[count..])
        .map_err(|_| BrowserCookieError::EncryptedMalformed)?;
    plain.truncate(count + rest);
    Ok(plain)
}
fn decode_hex(encoded: &[u8]) -> Option<Vec<u8>> {
    if !encoded.len().is_multiple_of(2) {
        return None;
    }
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    encoded
        .chunks_exact(2)
        .map(|pair| Some((digit(pair[0])? << 4) | digit(pair[1])?))
        .collect()
}
fn go_space(character: char) -> bool {
    matches!(character, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{0085}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}
fn trim_go_space(mut bytes: &[u8]) -> &[u8] {
    loop {
        let length = match bytes.first().copied() {
            Some(0..=127) => 1,
            Some(0xc2..=0xdf) => 2,
            Some(0xe0..=0xef) => 3,
            Some(0xf0..=0xf4) => 4,
            _ => break,
        };
        let Some(first) = bytes
            .get(..length)
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|s| s.chars().next())
        else {
            break;
        };
        if !go_space(first) {
            break;
        }
        bytes = &bytes[length..];
    }
    loop {
        if bytes.is_empty() {
            break;
        }
        let start = (bytes.len().saturating_sub(4)..bytes.len())
            .rev()
            .find(|index| bytes[*index] & 0xc0 != 0x80);
        let Some(start) = start else {
            break;
        };
        let Some(last) = std::str::from_utf8(&bytes[start..])
            .ok()
            .and_then(|s| s.chars().next())
        else {
            break;
        };
        if !go_space(last) {
            break;
        }
        bytes = &bytes[..start];
    }
    bytes
}

fn json_depth_supported(body: &[u8]) -> bool {
    let mut in_string = false;
    let mut depth = 0_u32;
    let mut index = 0;
    while index < body.len() {
        let byte = body[index];
        if in_string && byte == b'\\' {
            index += 2;
            continue;
        }
        if byte == b'"' {
            in_string = !in_string;
        } else if !in_string {
            match byte {
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 10000 {
                        return false;
                    }
                }
                b'}' | b']' => {
                    let Some(next) = depth.checked_sub(1) else {
                        return false;
                    };
                    depth = next;
                }
                _ => {}
            }
        }
        index += 1;
    }
    true
}
fn json_field_eq(actual: &str, expected: &str) -> bool {
    let mut actual = actual.chars();
    for expected in expected.bytes() {
        let Some(character) = actual.next() else {
            return false;
        };
        let folded = match character {
            '\u{017f}' => b'S',
            '\u{212a}' => b'K',
            value if value.is_ascii() => (value as u8).to_ascii_uppercase(),
            _ => return false,
        };
        if folded != expected.to_ascii_uppercase() {
            return false;
        }
    }
    actual.next().is_none()
}

struct StateSeed<'a> {
    encoded: &'a mut String,
}
struct CryptSeed<'a> {
    encoded: &'a mut String,
}
struct KeySeed<'a> {
    encoded: &'a mut String,
}
impl<'de> DeserializeSeed<'de> for StateSeed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for StateSeed<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a Local State object or null")
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        while let Some(field) = map.next_key::<String>()? {
            if json_field_eq(&field, "os_crypt") {
                map.next_value_seed(CryptSeed {
                    encoded: self.encoded,
                })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}
impl<'de> DeserializeSeed<'de> for CryptSeed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for CryptSeed<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an os_crypt object or null")
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        while let Some(field) = map.next_key::<String>()? {
            if json_field_eq(&field, "encrypted_key") {
                map.next_value_seed(KeySeed {
                    encoded: self.encoded,
                })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}
impl<'de> DeserializeSeed<'de> for KeySeed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for KeySeed<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an encrypted_key string or null")
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<(), E> {
        value.clone_into(self.encoded);
        Ok(())
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<(), E> {
        *self.encoded = value;
        Ok(())
    }
}
