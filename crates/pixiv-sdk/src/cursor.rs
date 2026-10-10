use crate::{
    Error, Reason, Result,
    codec::{Payload, PayloadSeed, base64_diagnostic, decode_base64, engine, normalize_json},
    error::Cause,
};
use base64::Engine;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, IgnoredAny, MapAccess, Visitor},
};
use std::fmt;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CursorOptions {
    pub identity: String,
    pub ephemeral: bool,
    pub instance: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Cursor,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            next: Cursor::default(),
        }
    }
}

fn cursor_error(operation: &'static str, detail: &'static str) -> Error {
    Error::with_product("", Reason::InvalidCursor, operation).with_detail(detail)
}

impl Cursor {
    pub fn new(
        product: &str,
        operation: &str,
        binding: i64,
        query: &str,
        payload: &[u8],
        options: CursorOptions,
    ) -> Result<Self> {
        let detail = if product.is_empty() {
            Some("product is required")
        } else if operation.is_empty() {
            Some("operation is required")
        } else if binding <= 0 {
            Some("binding version must be positive")
        } else if query.is_empty() {
            Some("query hash is required")
        } else if payload.is_empty() {
            Some("cursor payload is required")
        } else if options
            .instance
            .as_ref()
            .is_some_and(|instance| instance.is_empty())
        {
            Some("cursor instance is required")
        } else {
            None
        };
        if let Some(detail) = detail {
            return Err(
                Error::with_product("", Reason::InvalidArgument, "NewCursor").with_detail(detail),
            );
        }
        #[derive(Serialize)]
        struct Encoded<'a> {
            v: u8,
            p: &'a str,
            o: &'a str,
            b: i64,
            q: &'a str,
            #[serde(skip_serializing_if = "String::is_empty")]
            id: String,
            #[serde(skip_serializing_if = "std::ops::Not::not")]
            e: bool,
            #[serde(skip_serializing_if = "String::is_empty")]
            i: String,
            pl: String,
        }
        let raw = serde_json::to_string(&Encoded {
            v: 1,
            p: product,
            o: operation,
            b: binding,
            q: query,
            id: options.identity,
            e: options.ephemeral || options.instance.is_some(),
            i: options.instance.unwrap_or_default(),
            pl: engine(false).encode(payload),
        })
        .map_err(|_| Error::with_product("", Reason::InvalidArgument, "NewCursor"))?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
        Ok(Self {
            text: engine(true).encode(raw),
        })
    }

    pub fn parse(text: &str) -> Result<Self> {
        if text.is_empty() {
            return Err(cursor_error("Cursor.UnmarshalText", "empty cursor text"));
        }
        let cursor = Self {
            text: text.to_owned(),
        };
        cursor.decode("Cursor.UnmarshalText")?;
        Ok(cursor)
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
    pub fn is_zero(&self) -> bool {
        self.text.is_empty()
    }

    pub fn marshal_text(&self) -> Result<&str> {
        if self.is_zero() {
            return Err(
                Error::with_product("", Reason::InvalidArgument, "Cursor.MarshalText")
                    .with_detail("zero cursor has no encoding"),
            );
        }
        Ok(&self.text)
    }

    pub fn marshal_json(&self) -> Result<String> {
        if self.is_zero() {
            return Err(
                Error::with_product("", Reason::InvalidArgument, "Cursor.MarshalJSON")
                    .with_detail("zero cursor has no encoding"),
            );
        }
        serde_json::to_string(&self.text)
            .map_err(|_| Error::with_product("", Reason::InvalidArgument, "Cursor.MarshalJSON"))
    }

    pub fn unmarshal_text(&mut self, text: &str) -> Result<()> {
        *self = Self::parse(text)?;
        Ok(())
    }

    pub fn unmarshal_json(&mut self, data: &[u8]) -> Result<()> {
        let json = normalize_json(data)
            .map_err(|_| cursor_error("Cursor.UnmarshalJSON", "malformed cursor JSON"))?;
        let text: Option<String> = serde_json::from_str(&json)
            .map_err(|_| cursor_error("Cursor.UnmarshalJSON", "malformed cursor JSON"))?;
        self.unmarshal_text(text.as_deref().unwrap_or_default())
    }

    pub fn validate(
        &self,
        product: &str,
        operation: &str,
        binding: i64,
        query: &str,
    ) -> Result<()> {
        if self.is_zero() {
            return Err(cursor_error("ValidateCursor", "zero cursor"));
        }
        let envelope = self.decode("ValidateCursor")?;
        if envelope.product != product
            || envelope.operation != operation
            || envelope.binding != binding
            || envelope.query != query
        {
            return Err(cursor_error("ValidateCursor", "cursor binding mismatch"));
        }
        Ok(())
    }

    pub fn validate_instance(&self, instance: &str) -> Result<()> {
        if self.is_zero() || instance.is_empty() {
            return Err(cursor_error(
                "ValidateCursorInstance",
                "cursor instance binding is unavailable",
            ));
        }
        let envelope = self.decode("ValidateCursorInstance")?;
        if !envelope.ephemeral || envelope.instance.is_empty() || envelope.instance != instance {
            return Err(cursor_error(
                "ValidateCursorInstance",
                "cursor instance binding mismatch",
            ));
        }
        Ok(())
    }

    pub fn payload(&self) -> Result<Vec<u8>> {
        if self.is_zero() {
            return Err(cursor_error("CursorPayload", "zero cursor"));
        }
        let payload = self.decode("CursorPayload")?.payload;
        Ok(payload.bytes[..payload.length].to_vec())
    }

    pub fn identity(&self) -> Option<String> {
        let identity = self.decode("CursorIdentity").ok()?.identity;
        (!identity.is_empty()).then_some(identity)
    }
    pub fn is_ephemeral(&self) -> bool {
        self.decode("CursorEphemeral")
            .map(|value| value.ephemeral)
            .unwrap_or_default()
    }

    fn decode(&self, operation: &'static str) -> Result<Envelope> {
        let cause = |message: String| {
            Error::with_product("", Reason::InvalidCursor, operation)
                .with_cause(Cause::Redacted(message))
        };
        let raw = decode_base64(&self.text, true)
            .map_err(|error| cause(base64_diagnostic(&self.text, true, error)))?;
        let normalized = normalize_json(&raw).map_err(|error| cause(error.to_string()))?;
        let envelope: Envelope = serde_json::from_str(&normalized)
            .map_err(|error| cause(cursor_json_diagnostic(&raw, error)))?;
        if envelope.version != 1 {
            return Err(cause(format!(
                "unsupported cursor format version {}",
                envelope.version
            )));
        }
        Ok(envelope)
    }
}

impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Serialize for Cursor {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if self.is_zero() {
            return Err(serde::ser::Error::custom(
                Error::with_product("", Reason::InvalidArgument, "Cursor.MarshalJSON")
                    .with_detail("zero cursor has no encoding"),
            ));
        }
        serializer.serialize_str(&self.text)
    }
}

impl<'de> Deserialize<'de> for Cursor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

#[derive(Default)]
struct Envelope {
    version: i64,
    product: String,
    operation: String,
    binding: i64,
    query: String,
    identity: String,
    ephemeral: bool,
    instance: String,
    payload: Payload,
}

impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct EnvelopeVisitor;
        impl<'de> Visitor<'de> for EnvelopeVisitor {
            type Value = Envelope;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a cursor envelope")
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Envelope, E> {
                Ok(Envelope::default())
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Envelope, M::Error> {
                let mut envelope = Envelope::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.to_ascii_lowercase().as_str() {
                        "v" => {
                            if let Some(value) = map.next_value::<Option<i64>>()? {
                                envelope.version = value;
                            }
                        }
                        "p" => {
                            if let Some(value) = map.next_value::<Option<String>>()? {
                                envelope.product = value;
                            }
                        }
                        "o" => {
                            if let Some(value) = map.next_value::<Option<String>>()? {
                                envelope.operation = value;
                            }
                        }
                        "b" => {
                            if let Some(value) = map.next_value::<Option<i64>>()? {
                                envelope.binding = value;
                            }
                        }
                        "q" => {
                            if let Some(value) = map.next_value::<Option<String>>()? {
                                envelope.query = value;
                            }
                        }
                        "id" => {
                            if let Some(value) = map.next_value::<Option<String>>()? {
                                envelope.identity = value;
                            }
                        }
                        "e" => {
                            if let Some(value) = map.next_value::<Option<bool>>()? {
                                envelope.ephemeral = value;
                            }
                        }
                        "i" => {
                            if let Some(value) = map.next_value::<Option<String>>()? {
                                envelope.instance = value;
                            }
                        }
                        "pl" => map.next_value_seed(PayloadSeed(&mut envelope.payload))?,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(envelope)
            }
        }
        deserializer.deserialize_any(EnvelopeVisitor)
    }
}

fn cursor_json_diagnostic(raw: &[u8], error: serde_json::Error) -> String {
    if error.is_eof() {
        return "unexpected end of JSON input".into();
    }
    if let Some(&byte) = raw
        .iter()
        .find(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        && !matches!(
            byte,
            b'{' | b'[' | b'"' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n'
        )
    {
        let character = match byte {
            b'\'' => "\"'\"".into(),
            b'\\' => "'\\\\'".into(),
            0x20..=0x7e => format!("'{}'", char::from(byte)),
            _ => format!("'\\x{byte:02x}'"),
        };
        return format!("invalid character {character} looking for beginning of value");
    }
    error.to_string()
}
