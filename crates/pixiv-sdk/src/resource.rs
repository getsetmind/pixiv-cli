use crate::{Error, Reason, Result};
use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
};
use std::fmt;

fn reference_error(operation: &'static str) -> Error {
    Error::with_product("", Reason::InvalidArgument, operation)
}

fn engine(url: bool) -> GeneralPurpose {
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

fn decode_base64(text: &str, url: bool) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    let bytes: Vec<_> = text
        .bytes()
        .filter(|byte| !matches!(byte, b'\r' | b'\n'))
        .collect();
    engine(url).decode(bytes)
}

#[derive(Clone, Default, Eq, Hash, PartialEq)]
pub struct ResourceRef {
    text: String,
}

impl ResourceRef {
    pub fn new(product: &str, payload: &[u8]) -> Result<Self> {
        if product.is_empty() || payload.is_empty() {
            return Err(reference_error("NewResourceRef"));
        }
        #[derive(Serialize)]
        struct Encoded<'a> {
            v: u8,
            p: &'a str,
            d: String,
        }
        let json = serde_json::to_string(&Encoded {
            v: 1,
            p: product,
            d: engine(false).encode(payload),
        })
        .map_err(|_| reference_error("NewResourceRef"))?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
        Ok(Self {
            text: engine(true).encode(json),
        })
    }

    pub fn parse(text: &str) -> Result<Self> {
        if text.is_empty() {
            return Err(reference_error("ResourceRef.UnmarshalText"));
        }
        let reference = Self {
            text: text.to_owned(),
        };
        reference.decode()?;
        Ok(reference)
    }

    pub fn is_zero(&self) -> bool {
        self.text.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn marshal_text(&self) -> Result<&str> {
        if self.is_zero() {
            return Err(reference_error("ResourceRef.MarshalText"));
        }
        Ok(&self.text)
    }

    pub fn unmarshal_text(&mut self, text: &str) -> Result<()> {
        *self = Self::parse(text)?;
        Ok(())
    }

    pub fn unmarshal_json(&mut self, data: &[u8]) -> Result<()> {
        let normalized =
            normalize_json(data).map_err(|_| reference_error("ResourceRef.UnmarshalJSON"))?;
        let text: Option<String> = serde_json::from_str(&normalized)
            .map_err(|_| reference_error("ResourceRef.UnmarshalJSON"))?;
        self.unmarshal_text(text.as_deref().unwrap_or_default())
    }

    pub fn marshal_json(&self) -> Result<String> {
        self.marshal_text()?;
        serde_json::to_string(self).map_err(|_| reference_error("ResourceRef.MarshalJSON"))
    }

    pub fn product(&self) -> Result<String> {
        Ok(self.decode()?.product)
    }

    pub fn payload(&self) -> Result<Vec<u8>> {
        let payload = self.decode()?.payload;
        Ok(payload.bytes[..payload.length].to_vec())
    }

    fn decode(&self) -> Result<Envelope> {
        let raw =
            decode_base64(&self.text, true).map_err(|_| reference_error("decodeResourceRef"))?;
        let envelope: Envelope = serde_json::from_str(&normalize_json(&raw)?)
            .map_err(|_| reference_error("decodeResourceRef"))?;
        if envelope.version != 1 || envelope.product.is_empty() || envelope.payload.length == 0 {
            return Err(reference_error("decodeResourceRef"));
        }
        Ok(envelope)
    }
}

impl fmt::Debug for ResourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ResourceRef").field(&self.text).finish()
    }
}

impl fmt::Display for ResourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Serialize for ResourceRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.marshal_text().map_err(serde::ser::Error::custom)?)
    }
}

impl<'de> Deserialize<'de> for ResourceRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

#[derive(Default)]
struct Envelope {
    version: i64,
    product: String,
    payload: Payload,
}

impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct EnvelopeVisitor;
        impl<'de> Visitor<'de> for EnvelopeVisitor {
            type Value = Envelope;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a resource reference envelope")
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Envelope, M::Error> {
                let mut envelope = Envelope::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.to_ascii_lowercase().as_str() {
                        "v" => {
                            if let Some(version) = map.next_value::<Option<i64>>()? {
                                envelope.version = version;
                            }
                        }
                        "p" => {
                            if let Some(product) = map.next_value::<Option<String>>()? {
                                envelope.product = product;
                            }
                        }
                        "d" => map.next_value_seed(PayloadSeed(&mut envelope.payload))?,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(envelope)
            }
        }
        deserializer.deserialize_map(EnvelopeVisitor)
    }
}

#[derive(Default)]
struct Payload {
    bytes: Vec<u8>,
    length: usize,
}

struct PayloadSeed<'a>(&'a mut Payload);

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
                let decoded = decode_base64(value, false).map_err(E::custom)?;
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

fn normalize_json(raw: &[u8]) -> Result<String> {
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
                            return Err(reference_error("decodeResourceRef"));
                        }
                    }
                    b'}' | b']' => {
                        depth = depth
                            .checked_sub(1)
                            .ok_or_else(|| reference_error("decodeResourceRef"))?;
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
