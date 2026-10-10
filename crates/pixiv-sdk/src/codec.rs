use crate::{Error, Reason, Result};
use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use serde::{
    Deserializer,
    de::{self, DeserializeSeed, SeqAccess, Visitor},
};
use std::fmt;
pub(crate) fn engine(url: bool) -> GeneralPurpose {
    GeneralPurpose::new(
        if url {
            &alphabet::URL_SAFE
        } else {
            &alphabet::STANDARD
        },
        GeneralPurposeConfig::new()
            .with_encode_padding(!url)
            .with_decode_padding_mode(if url {
                DecodePaddingMode::RequireNone
            } else {
                DecodePaddingMode::RequireCanonical
            })
            .with_decode_allow_trailing_bits(true),
    )
}

pub(crate) fn decode_base64(
    text: &str,
    url: bool,
) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    let bytes: Vec<_> = text
        .bytes()
        .filter(|byte| !matches!(byte, b'\r' | b'\n'))
        .collect();
    engine(url).decode(bytes)
}

pub(crate) fn base64_diagnostic(text: &str, url: bool, error: base64::DecodeError) -> String {
    base64_error_offset(text.as_bytes(), url).map_or_else(
        || error.to_string(),
        |offset| format!("illegal base64 data at input byte {offset}"),
    )
}

fn base64_error_offset(bytes: &[u8], url: bool) -> Option<usize> {
    let mut index = 0;
    loop {
        let mut symbols = 0;
        while symbols < 4 {
            let Some(&byte) = bytes.get(index) else {
                return (symbols == 1 || (!url && symbols != 0)).then_some(index - symbols);
            };
            index += 1;
            if matches!(byte, b'\r' | b'\n') {
                continue;
            }
            if byte.is_ascii_alphanumeric()
                || if url {
                    matches!(byte, b'-' | b'_')
                } else {
                    matches!(byte, b'+' | b'/')
                }
            {
                symbols += 1;
                continue;
            }
            if url || byte != b'=' || symbols < 2 {
                return Some(index - 1);
            }
            if symbols == 2 {
                while bytes
                    .get(index)
                    .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
                {
                    index += 1;
                }
                match bytes.get(index) {
                    None => return Some(bytes.len()),
                    Some(b'=') => index += 1,
                    Some(_) => return Some(index - 1),
                }
            }
            while bytes
                .get(index)
                .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
            {
                index += 1;
            }
            return (index < bytes.len()).then_some(index);
        }
    }
}

#[derive(Default)]
pub(crate) struct Payload {
    pub(crate) bytes: Vec<u8>,
    pub(crate) length: usize,
}

pub(crate) struct PayloadSeed<'a>(pub(crate) &'a mut Payload);

impl<'de> DeserializeSeed<'de> for PayloadSeed<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<(), D::Error> {
        struct PayloadVisitor<'a>(&'a mut Payload);
        impl<'de> Visitor<'de> for PayloadVisitor<'_> {
            type Value = ();

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a base64 string or byte array")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<(), E> {
                let decoded = decode_base64(value, false)
                    .map_err(|error| E::custom(base64_diagnostic(value, false, error)))?;
                self.0.bytes = vec![0; value.len() / 4 * 3];
                self.0.bytes[..decoded.len()].copy_from_slice(&decoded);
                self.0.length = decoded.len();
                Ok(())
            }

            fn visit_unit<E: de::Error>(self) -> std::result::Result<(), E> {
                *self.0 = Payload::default();
                Ok(())
            }

            fn visit_seq<S: SeqAccess<'de>>(
                self,
                mut sequence: S,
            ) -> std::result::Result<(), S::Error> {
                let mut length = 0;
                while let Some(byte) = sequence.next_element::<Option<u8>>()? {
                    if length == self.0.bytes.len() {
                        self.0.bytes.push(0);
                    }
                    if let Some(byte) = byte {
                        self.0.bytes[length] = byte;
                    }
                    length += 1;
                }
                if length == 0 {
                    self.0.bytes.clear();
                }
                // Truncation would lose Go's retained values for nulls in later duplicate arrays.
                self.0.length = length;
                Ok(())
            }
        }
        deserializer.deserialize_any(PayloadVisitor(self.0))
    }
}

pub(crate) fn go_utf8(raw: &[u8]) -> String {
    // Lossy decoding groups invalid bytes, unlike Go's JSON decoder.
    let mut text = String::new();
    let mut remaining = raw;
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                text.push_str(valid);
                break;
            }
            Err(error) => {
                text.push_str(
                    std::str::from_utf8(&remaining[..error.valid_up_to()])
                        .expect("validated UTF-8 prefix"),
                );
                text.push('\u{fffd}');
                remaining = &remaining[error.valid_up_to() + 1..];
            }
        }
    }
    text
}

pub(crate) fn normalize_json(raw: &[u8]) -> Result<String> {
    let text = go_utf8(raw);
    let bytes = text.as_bytes();
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string = false;
    let mut depth = 0_u32;
    while index < bytes.len() {
        if in_string && bytes[index] == b'\\' && index + 1 < bytes.len() {
            if let Some(code) = unicode_escape(bytes, index) {
                if (0xd800..=0xdbff).contains(&code)
                    && unicode_escape(bytes, index + 6)
                        .is_some_and(|low| (0xdc00..=0xdfff).contains(&low))
                {
                    normalized.extend_from_slice(&bytes[index..index + 12]);
                    index += 12;
                } else {
                    if (0xd800..=0xdfff).contains(&code) {
                        normalized.extend_from_slice(b"\\ufffd");
                    } else {
                        normalized.extend_from_slice(&bytes[index..index + 6]);
                    }
                    index += 6;
                }
            } else {
                normalized.extend_from_slice(&bytes[index..index + 2]);
                index += 2;
            }
        } else {
            if bytes[index] == b'"' {
                in_string = !in_string;
            }
            if !in_string {
                match bytes[index] {
                    b'{' | b'[' => {
                        depth += 1;
                        if depth > 10000 {
                            return Err(Error::with_product(
                                "",
                                Reason::InvalidArgument,
                                "DecodeJSON",
                            ));
                        }
                    }
                    b'}' | b']' => {
                        depth = depth.checked_sub(1).ok_or_else(|| {
                            Error::with_product("", Reason::InvalidArgument, "DecodeJSON")
                        })?;
                    }
                    _ => {}
                }
            }
            normalized.push(bytes[index]);
            index += 1;
        }
    }
    Ok(String::from_utf8(normalized).expect("normalization preserves UTF-8"))
}

fn unicode_escape(bytes: &[u8], index: usize) -> Option<u16> {
    let escape = bytes.get(index..index.checked_add(6)?)?;
    if !escape.starts_with(b"\\u") {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(&escape[2..]).ok()?, 16).ok()
}
