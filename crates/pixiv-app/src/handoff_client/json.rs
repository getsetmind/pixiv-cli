use super::OwnedBody;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use serde_json::value::RawValue;
use std::fmt;

pub(super) struct Decoder<'a> {
    body: &'a OwnedBody,
    buffer: Vec<u8>,
    capacity: usize,
}
impl<'a> Decoder<'a> {
    pub(super) fn new(body: &'a OwnedBody) -> Self {
        Self {
            body,
            buffer: Vec::new(),
            capacity: 0,
        }
    }
    pub(super) async fn next(&mut self) -> Result<Option<String>, ()> {
        let mut terminal = None;
        loop {
            let normalized = crate::auth_bundle::normalize_strings(&self.buffer);
            let mut parser = serde_json::Deserializer::from_str(&normalized);
            match <&RawValue>::deserialize(&mut parser) {
                Ok(value) => {
                    crate::handoff_state::validate_json_depth(value.get()).map_err(|_| ())?;
                    let normalized_end = parser.into_iter::<&RawValue>().byte_offset();
                    let structural = value.get().starts_with(['{', '[']);
                    if structural || normalized_end < normalized.len() || terminal == Some(false) {
                        let end = raw_end(&self.buffer, normalized_end);
                        let value = value.get().to_owned();
                        self.buffer.drain(..end);
                        return Ok(Some(value));
                    }
                }
                Err(error) if !error.is_eof() => return Err(()),
                _ => {
                    crate::handoff_state::validate_json_depth(&normalized).map_err(|_| ())?;
                }
            }
            if let Some(failed) = terminal {
                if !failed && self.buffer.iter().all(|b| b" \r\n\t".contains(b)) {
                    self.buffer.clear();
                    return Ok(None);
                }
                return Err(());
            }
            if self.capacity - self.buffer.len() < 512 {
                self.capacity = self
                    .capacity
                    .checked_mul(2)
                    .and_then(|n| n.checked_add(512))
                    .ok_or(())?;
            }
            let mut bytes = vec![0_u8; self.capacity - self.buffer.len()];
            let read = self.body.body.read(&mut bytes).await;
            if read.count > bytes.len() {
                return Err(());
            }
            self.buffer.extend_from_slice(&bytes[..read.count]);
            terminal = if read.error.is_some() {
                Some(true)
            } else if read.count == 0 {
                Some(false)
            } else {
                None
            };
        }
    }
}

fn raw_end(buffer: &[u8], fallback: usize) -> usize {
    let start = buffer
        .iter()
        .position(|b| !b" \r\n\t".contains(b))
        .unwrap_or(buffer.len());
    let Some(first) = buffer.get(start) else {
        return buffer.len();
    };
    if !b"{[\"".contains(first) {
        return fallback;
    }
    let mut depth = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, &byte) in buffer.iter().enumerate().skip(start) {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
                if *first == b'"' {
                    return index + 1;
                }
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return index + 1;
                    }
                }
                _ => {}
            }
        }
    }
    fallback
}

struct Fields(Vec<(String, String)>);
impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;
        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = Fields;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("relay result object")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Fields, E> {
                Ok(Fields(Vec::new()))
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Fields, M::Error> {
                let mut fields = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    let raw = map.next_value::<&RawValue>()?;
                    fields.push((key, raw.get().to_owned()));
                }
                Ok(Fields(fields))
            }
        }
        deserializer.deserialize_any(FieldsVisitor)
    }
}

pub(super) fn authorization(raw: &str) -> Result<String, ()> {
    let fields: Fields = serde_json::from_str(raw).map_err(|_| ())?;
    let mut address = String::new();
    let mut bad_type = false;
    for (name, value) in fields.0 {
        if !crate::auth_bundle::folded(&name, "authorization_url") || value == "null" {
            continue;
        }
        match serde_json::from_str::<String>(&value) {
            Ok(value) => address = value,
            Err(_) => bad_type = true,
        }
    }
    if bad_type { Err(()) } else { Ok(address) }
}
pub(super) fn completion(raw: &str) -> Result<bool, ()> {
    let fields: Fields = serde_json::from_str(raw).map_err(|_| ())?;
    let mut success = false;
    let mut bad_type = false;
    for (name, value) in fields.0 {
        if !crate::auth_bundle::folded(&name, "success") || value == "null" {
            continue;
        }
        match value.as_str() {
            "true" => success = true,
            "false" => success = false,
            _ => bad_type = true,
        }
    }
    if bad_type { Err(()) } else { Ok(success) }
}
