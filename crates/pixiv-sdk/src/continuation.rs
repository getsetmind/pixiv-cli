use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn query_escape(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push('%');
                encoded.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
                encoded.push(char::from(b"0123456789ABCDEF"[(byte & 15) as usize]));
            }
        }
    }
    encoded
}
pub(crate) fn query_digest(query: &BTreeMap<String, String>) -> String {
    let mut canonical = String::new();
    for (key, value) in query {
        canonical.push_str(&query_escape(key));
        canonical.push('=');
        canonical.push_str(&query_escape(value));
        canonical.push('&');
    }
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}
pub(crate) fn next_offset(raw: &str, endpoint: &str, allowed_keys: &[&str]) -> Option<i64> {
    next_value(raw, endpoint, allowed_keys, "offset")
}
pub(crate) fn next_value(
    raw: &str,
    endpoint: &str,
    allowed_keys: &[&str],
    key: &str,
) -> Option<i64> {
    next_keyed_value(raw, endpoint, allowed_keys, &[key]).map(|(_, value)| value)
}
pub(crate) fn next_keyed_value(
    raw: &str,
    endpoint: &str,
    allowed_keys: &[&str],
    keys: &[&str],
) -> Option<(String, i64)> {
    let (scheme, remainder) = raw.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") || raw.bytes().any(|byte| byte < 32 || byte == 127) {
        return None;
    }
    let (authority, path) = remainder.split_once('/')?;
    if !matches!(authority, "app-api.pixiv.net" | "app-api.pixiv.net:") {
        return None;
    }
    let (path, fragment) = path
        .split_once('#')
        .map_or((path, ""), |(path, fragment)| (path, fragment));
    if !fragment.is_empty() {
        return None;
    }
    let (path, query) = path.split_once('?')?;
    if path != endpoint || query.contains(';') {
        return None;
    }
    let bytes = query.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (bytes
                .get(index + 1)
                .is_none_or(|byte| !byte.is_ascii_hexdigit())
                || bytes
                    .get(index + 2)
                    .is_none_or(|byte| !byte.is_ascii_hexdigit()))
        {
            return None;
        }
    }
    let mut entries = BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(bytes) {
        if !allowed_keys.contains(&key.as_ref())
            || entries
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return None;
        }
    }
    let mut selected = keys.iter().filter(|key| entries.contains_key(**key));
    let key = *selected.next()?;
    if selected.next().is_some() {
        return None;
    }
    let value = entries.get(key)?.parse::<i64>().ok()?;
    (value > 0).then_some((key.into(), value))
}
