use crate::codec::{Payload, PayloadSeed, decode_base64, engine, normalize_json};
use crate::{Error, Reason, Result};
use base64::Engine;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, IgnoredAny, MapAccess, Visitor},
};
use std::fmt;

#[derive(Clone, Default, PartialEq)]
pub struct Resource {
    pub reference: ResourceRef,
    pub url: String,
    pub request_headers: std::collections::BTreeMap<String, String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub requires_credentials: bool,
}
impl fmt::Debug for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resource")
            .field("reference", &self.reference)
            .field("requires_credentials", &self.requires_credentials)
            .finish_non_exhaustive()
    }
}
fn reference_error(operation: &'static str) -> Error {
    Error::with_product("", Reason::InvalidArgument, operation)
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
        let envelope: Envelope = serde_json::from_str(
            &normalize_json(&raw).map_err(|_| reference_error("decodeResourceRef"))?,
        )
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
