#[path = "auth_bundle_quote.rs"]
mod quoted;
pub(crate) use quoted::quote as go_quote;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use std::{collections::HashSet, fmt};

pub const SCHEMA: &str = "pixiv-cli.auth-export";
pub const VERSION: i64 = 1;

#[derive(Clone, Serialize, Default)]
pub struct AuthExportAccount {
    pub user_id: i64,
    pub username: String,
    pub refresh_token: String,
}
#[derive(Clone, Serialize, Default)]
pub struct AuthExportBundle {
    pub schema: String,
    pub version: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub default_user_id: i64,
    pub accounts: Vec<AuthExportAccount>,
}
fn is_zero(value: &i64) -> bool {
    *value == 0
}
#[derive(Debug)]
pub struct AuthBundleError(String);
impl fmt::Display for AuthBundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AuthBundleError {}
fn error(text: impl Into<String>) -> AuthBundleError {
    AuthBundleError(text.into())
}
pub fn encode(bundle: &AuthExportBundle) -> Result<Vec<u8>, AuthBundleError> {
    if bundle.accounts.is_empty() {
        return Err(error("auth export bundle has no accounts"));
    }
    let json =
        serde_json::to_string(bundle).map_err(|_| error("auth export bundle encoding failed"))?;
    Ok(json
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
        .into_bytes())
}

enum Node {
    Null,
    String(String),
    Number(String),
    Bool,
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}
impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Nodes;
        impl<'de> Visitor<'de> for Nodes {
            type Value = Node;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
                Ok(Node::Null)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Node, E> {
                Ok(Node::Bool)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Node, E> {
                Ok(Node::String(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Node, E> {
                Ok(Node::String(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Node, E> {
                Ok(Node::Number(v.to_string()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Node, E> {
                Ok(Node::Number(v.to_string()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Node, E> {
                Ok(Node::Number(format!("{v:e}")))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Node, A::Error> {
                let mut v = Vec::new();
                while let Some(x) = a.next_element()? {
                    v.push(x)
                }
                Ok(Node::Array(v))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Node, A::Error> {
                let mut v = Vec::new();
                let mut seen = HashSet::new();
                while let Some(k) = a.next_key::<String>()? {
                    if !seen.insert(k.clone()) {
                        return Err(de::Error::custom(format!(
                            "auth export bundle JSON has duplicate object key {}",
                            go_quote(&k)
                        )));
                    }
                    v.push((k, a.next_value()?));
                }
                Ok(Node::Object(v))
            }
        }
        let raw = <&serde_json::value::RawValue>::deserialize(deserializer)?;
        let text = raw.get();
        if matches!(text.as_bytes().first(), Some(b'-' | b'0'..=b'9')) {
            let number = text.parse::<f64>().map_err(de::Error::custom)?;
            if !number.is_finite() {
                return Err(de::Error::custom("strconv.ParseFloat: value out of range"));
            }
            return Ok(Node::Number(text.into()));
        }
        serde_json::Deserializer::from_str(text)
            .deserialize_any(Nodes)
            .map_err(de::Error::custom)
    }
}

fn folded(key: &str, name: &str) -> bool {
    key.chars()
        .map(|c| match c {
            '\u{017f}' => 'S',
            '\u{212a}' => 'K',
            c => c.to_ascii_uppercase(),
        })
        .eq(name.chars().map(|c| c.to_ascii_uppercase()))
}
fn kind(node: &Node) -> &str {
    match node {
        Node::Null => "null",
        Node::String(_) => "string",
        Node::Number(_) => "number",
        Node::Bool => "bool",
        Node::Array(_) => "array",
        Node::Object(_) => "object",
    }
}
fn invalid_type(node: &Node, path: &str, ty: &str) -> String {
    format!(
        "json: cannot unmarshal {} into Go struct field {path} of type {ty}",
        kind(node)
    )
}
fn scalar(node: Node, path: &str, target: &mut String) -> Result<(), String> {
    match node {
        Node::Null => Ok(()),
        Node::String(v) => {
            *target = v;
            Ok(())
        }
        n => Err(invalid_type(&n, path, "string")),
    }
}
fn integer(node: Node, path: &str, ty: &str, target: &mut i64) -> Result<(), String> {
    match node {
        Node::Null => Ok(()),
        Node::Number(v) => match v.parse() {
            Ok(v) => {
                *target = v;
                Ok(())
            }
            Err(_) => Err(format!(
                "json: cannot unmarshal number {v} into Go struct field {path} of type {ty}"
            )),
        },
        n => Err(invalid_type(&n, path, ty)),
    }
}
fn account(node: Node, index: usize) -> Result<(AuthExportAccount, Option<String>), String> {
    let mut result = AuthExportAccount::default();
    let mut first = None;
    match node {
        Node::Null => {}
        Node::Object(fields) => {
            for (key, value) in fields {
                let r = if folded(&key, "user_id") {
                    integer(
                        value,
                        &format!("authExportBundle.accounts.{index}.user_id"),
                        "int64",
                        &mut result.user_id,
                    )
                } else if folded(&key, "username") {
                    scalar(
                        value,
                        &format!("authExportBundle.accounts.{index}.username"),
                        &mut result.username,
                    )
                } else if folded(&key, "refresh_token") {
                    scalar(
                        value,
                        &format!("authExportBundle.accounts.{index}.refresh_token"),
                        &mut result.refresh_token,
                    )
                } else {
                    Err(format!("json: unknown field {}", go_quote(&key)))
                };
                if first.is_none() {
                    first = r.err();
                }
            }
        }
        other => {
            return Err(invalid_type(
                &other,
                "authExportBundle.accounts",
                "auth.authExportSecretAccount",
            ));
        }
    }
    Ok((result, first))
}

struct Scan;
impl<'de> Deserialize<'de> for Scan {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Tokens;
        impl<'de> Visitor<'de> for Tokens {
            type Value = Scan;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Scan, E> {
                Ok(Scan)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Scan, E> {
                Ok(Scan)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Scan, E> {
                Ok(Scan)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Scan, E> {
                Ok(Scan)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Scan, E> {
                Ok(Scan)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Scan, E> {
                Ok(Scan)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Scan, A::Error> {
                while a.next_element::<Scan>()?.is_some() {}
                Ok(Scan)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Scan, A::Error> {
                let mut seen = HashSet::new();
                while let Some(key) = a.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(de::Error::custom(format!(
                            "auth export bundle JSON has duplicate object key {}",
                            go_quote(&key)
                        )));
                    }
                    a.next_value::<Scan>()?;
                }
                Ok(Scan)
            }
        }
        deserializer.deserialize_any(Tokens)
    }
}

pub fn is_json(body: &[u8]) -> bool {
    serde_json::from_str::<&serde_json::value::RawValue>(&normalize_strings(body)).is_ok()
}

pub fn decode(body: &[u8]) -> Result<AuthExportBundle, AuthBundleError> {
    let normalized = normalize_strings(body);
    let body = normalized.as_bytes();
    let mut scanner = serde_json::Deserializer::from_slice(body).into_iter::<Scan>();
    scanner
        .next()
        .ok_or_else(|| error("invalid auth export bundle: EOF"))?
        .map_err(|e| scan_error(e, body))?;
    let mut trailing = false;
    for token in scanner {
        token.map_err(|e| scan_error(e, body))?;
        trailing = true;
    }
    let mut parser = serde_json::Deserializer::from_slice(body).into_iter::<Node>();
    let first = parser
        .next()
        .ok_or_else(|| error("invalid auth export bundle: EOF"))?
        .map_err(|e| scan_error(e, body))?;
    let mut bundle = AuthExportBundle::default();
    let mut saved = None;
    match first {
        Node::Null => {}
        Node::Object(fields) => {
            for (key, value) in fields {
                let r = if folded(&key, "schema") {
                    scalar(value, "authExportBundle.schema", &mut bundle.schema)
                } else if folded(&key, "version") {
                    integer(
                        value,
                        "authExportBundle.version",
                        "int",
                        &mut bundle.version,
                    )
                } else if folded(&key, "default_user_id") {
                    integer(
                        value,
                        "authExportBundle.default_user_id",
                        "int64",
                        &mut bundle.default_user_id,
                    )
                } else if folded(&key, "accounts") {
                    match value {
                        Node::Null => {
                            bundle.accounts.clear();
                            Ok(())
                        }
                        Node::Array(values) => {
                            bundle.accounts.clear();
                            let mut failure = None;
                            for (index, value) in values.into_iter().enumerate() {
                                match account(value, index) {
                                    Ok((a, e)) => {
                                        bundle.accounts.push(a);
                                        if failure.is_none() {
                                            failure = e
                                        }
                                    }
                                    Err(e) => {
                                        if failure.is_none() {
                                            failure = Some(e)
                                        }
                                    }
                                }
                            }
                            failure.map_or(Ok(()), Err)
                        }
                        n => Err(invalid_type(
                            &n,
                            "authExportBundle.accounts",
                            "[]auth.authExportSecretAccount",
                        )),
                    }
                } else {
                    Err(format!("json: unknown field {}", go_quote(&key)))
                };
                if saved.is_none() {
                    saved = r.err();
                }
            }
        }
        node => {
            saved = Some(format!(
                "json: cannot unmarshal {} into Go value of type auth.authExportBundle",
                kind(&node)
            ))
        }
    }
    if let Some(e) = saved {
        return Err(error(format!("invalid auth export bundle: {e}")));
    }
    if trailing {
        return Err(error("auth export bundle has trailing JSON"));
    }
    if bundle.schema != SCHEMA || bundle.version != VERSION {
        return Err(error("unsupported auth export bundle schema or version"));
    }
    if bundle.accounts.is_empty() {
        return Err(error("auth export bundle has no accounts"));
    }
    let mut seen = HashSet::new();
    for a in &bundle.accounts {
        if a.user_id <= 0 || a.refresh_token.is_empty() {
            return Err(error("auth export bundle contains an invalid account"));
        }
        if !seen.insert(a.user_id) {
            return Err(error(format!(
                "auth export bundle contains duplicate account {}",
                a.user_id
            )));
        }
    }
    if bundle.default_user_id != 0 && !seen.contains(&bundle.default_user_id) {
        return Err(error(
            "auth export bundle default does not name an included account",
        ));
    }
    Ok(bundle)
}
fn scan_error(e: serde_json::Error, body: &[u8]) -> AuthBundleError {
    if e.to_string().starts_with("number out of range") {
        let preceding = body
            .split(|b| *b == b'\n')
            .take(e.line().saturating_sub(1))
            .map(|line| line.len() + 1)
            .sum::<usize>();
        let end = (preceding + e.column()).min(body.len());
        let mut start = end;
        while start > 0
            && matches!(
                body[start - 1],
                b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'
            )
        {
            start -= 1;
        }
        let number = String::from_utf8_lossy(&body[start..end]);
        return error(format!(
            "invalid auth export bundle: json: cannot unmarshal number {number} into Go value of type float64"
        ));
    }
    let text = e.to_string();
    let message = if let Some(index) = text.find(" at line ") {
        &text[..index]
    } else {
        &text
    };
    error(format!("invalid auth export bundle: {message}"))
}

pub(crate) fn go_utf8(raw: &[u8]) -> String {
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
fn normalize_strings(raw: &[u8]) -> String {
    let text = go_utf8(raw);
    let bytes = text.as_bytes();
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string = false;
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
            normalized.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(normalized).expect("normalization preserves UTF-8")
}
fn unicode_escape(bytes: &[u8], index: usize) -> Option<u16> {
    let escape = bytes.get(index..index.checked_add(6)?)?;
    if !escape.starts_with(b"\\u") {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(&escape[2..]).ok()?, 16).ok()
}
