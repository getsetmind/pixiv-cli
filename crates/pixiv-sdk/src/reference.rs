use crate::{Error, Reason, Result};
use url::Url;

pub type ReferenceKind = String;
pub const REFERENCE_KIND_ARTWORK: &str = "artwork";
pub const REFERENCE_KIND_NOVEL: &str = "novel";
pub const REFERENCE_KIND_USER: &str = "user";
pub const REFERENCE_KIND_USER_BOOKMARKS: &str = "user_bookmarks";
pub const REFERENCE_KIND_ARTWORK_SERIES: &str = "artwork_series";
pub const REFERENCE_KIND_NOVEL_SERIES: &str = "novel_series";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Reference {
    pub kind: ReferenceKind,
    pub id: i64,
    pub owner_user_id: i64,
}
impl Reference {
    pub fn canonical_url(&self) -> Result<String> {
        if self.id <= 0 {
            return Err(reference_error("reference has no positive ID"));
        }
        let path = match self.kind.as_str() {
            REFERENCE_KIND_ARTWORK => format!("artworks/{}", self.id),
            REFERENCE_KIND_NOVEL => format!("novel/show.php?id={}", self.id),
            REFERENCE_KIND_USER => format!("users/{}", self.id),
            REFERENCE_KIND_USER_BOOKMARKS => format!("users/{}/bookmarks/artworks", self.id),
            REFERENCE_KIND_ARTWORK_SERIES => {
                if self.owner_user_id <= 0 {
                    return Err(reference_error(
                        "artwork series reference requires an owner user ID",
                    ));
                }
                format!("user/{}/series/{}", self.owner_user_id, self.id)
            }
            REFERENCE_KIND_NOVEL_SERIES => format!("novel/series/{}", self.id),
            _ => return Err(reference_error("unknown reference kind")),
        };
        Ok(format!("https://www.pixiv.net/{path}"))
    }
}

pub fn parse_url(input: &str) -> Result<Reference> {
    let input = input.trim();
    if input.is_empty() {
        return Err(reference_error("empty URL"));
    }
    if input.parse::<i64>().is_ok() {
        return Err(reference_error(
            "bare integer cannot be resolved to a resource type",
        ));
    }
    let unparseable = || reference_error("URL is not parseable");
    if input.bytes().any(|byte| byte < 32 || byte == 127) {
        return Err(unparseable());
    }
    let fragment = input
        .split_once('#')
        .map(|(_, fragment)| fragment)
        .unwrap_or_default();
    decode_url_component(fragment, false).ok_or_else(unparseable)?;
    let (scheme, remainder) = input.split_once("://").unwrap_or(("", input));
    let boundary = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..boundary];
    let rest = &remainder[boundary..];
    if !scheme.is_empty() {
        Url::parse(input).map_err(|_| unparseable())?;
        if authority.contains('%') {
            return Err(unparseable());
        }
    }
    let path = rest.split(['?', '#']).next().unwrap_or_default();
    let path = decode_url_component(path, false).ok_or_else(unparseable)?;
    let host = authority.strip_suffix(':').unwrap_or(authority);
    if !scheme.eq_ignore_ascii_case("https")
        || !matches!(
            host.to_ascii_lowercase().as_str(),
            "pixiv.net" | "www.pixiv.net"
        )
        || authority.contains('@')
    {
        return Err(reference_error(
            "URL must be https on pixiv.net without userinfo or port",
        ));
    }
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    let query = rest
        .split('#')
        .next()
        .unwrap_or_default()
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    if let Some(reference) = parse_path(&parts, query) {
        return Ok(reference);
    }
    if parts.first().is_some_and(|part| is_locale(part))
        && let Some(reference) = parse_path(&parts[1..], query)
    {
        return Ok(reference);
    }
    Err(reference_error(
        "URL does not name a supported Pixiv resource",
    ))
}

fn parse_path(parts: &[&str], query: &str) -> Option<Reference> {
    let (kind, id, owner) = match parts {
        ["artworks", id] => (REFERENCE_KIND_ARTWORK, positive_id(id)?, 0),
        ["novel", "show.php"] => {
            let mut ids = vec![];
            for entry in query.split('&').filter(|entry| !entry.contains(';')) {
                let (key, value) = entry.split_once('=').unwrap_or((entry, ""));
                let Some(key) = decode_url_component(key, true) else {
                    continue;
                };
                let Some(value) = decode_url_component(value, true) else {
                    continue;
                };
                if key == "id" {
                    ids.push(value);
                }
            }
            if ids.len() != 1 {
                return None;
            }
            (REFERENCE_KIND_NOVEL, positive_id(&ids[0])?, 0)
        }
        ["users", id] | ["users", id, "artworks"] => (REFERENCE_KIND_USER, positive_id(id)?, 0),
        ["users", id, "bookmarks", "artworks"] => {
            (REFERENCE_KIND_USER_BOOKMARKS, positive_id(id)?, 0)
        }
        ["user", owner, "series", id] => (
            REFERENCE_KIND_ARTWORK_SERIES,
            positive_id(id)?,
            positive_id(owner)?,
        ),
        ["novel", "series", id] => (REFERENCE_KIND_NOVEL_SERIES, positive_id(id)?, 0),
        _ => return None,
    };
    Some(Reference {
        kind: kind.into(),
        id,
        owner_user_id: owner,
    })
}
fn positive_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|id| *id > 0)
}
fn is_locale(value: &str) -> bool {
    let parts: Vec<_> = value.split('-').collect();
    parts.len() <= 2
        && parts.iter().all(|part| {
            (2..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphabetic())
        })
}
fn decode_url_component(value: &str, query: bool) -> Option<String> {
    let mut result = vec![];
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let high = (*bytes.get(index + 1)? as char).to_digit(16)?;
                let low = (*bytes.get(index + 2)? as char).to_digit(16)?;
                result.push((high * 16 + low) as u8);
                index += 3;
            }
            b'+' if query => {
                result.push(b' ');
                index += 1;
            }
            byte => {
                result.push(byte);
                index += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&result).into_owned())
}
fn reference_error(detail: &str) -> Error {
    Error::new(Reason::InvalidArgument, "ParseURL").with_detail(detail)
}

pub fn artwork_id(input: &str) -> Result<i64> {
    if let Ok(id) = input.parse::<i64>() {
        return positive(id);
    }
    let url = Url::parse(input).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("www.pixiv.net" | "pixiv.net"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(invalid());
    }
    let segments: Vec<_> = url.path_segments().ok_or_else(invalid)?.collect();
    let id = match segments.as_slice() {
        ["artworks", id] | ["en", "artworks", id] => id.parse::<i64>().map_err(|_| invalid())?,
        ["member_illust.php"] => url
            .query_pairs()
            .find(|(key, _)| key == "illust_id")
            .ok_or_else(invalid)?
            .1
            .parse::<i64>()
            .map_err(|_| invalid())?,
        _ => return Err(invalid()),
    };
    positive(id)
}

fn positive(id: i64) -> Result<i64> {
    if id > 0 { Ok(id) } else { Err(invalid()) }
}

fn invalid() -> Error {
    Error::new(Reason::InvalidArgument, "artwork_reference")
}
