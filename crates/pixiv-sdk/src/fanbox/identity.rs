use super::{Failure, Session, User, read_body, safe_external_error};
use crate::{Reason, context::RequestContext, error::Cause};
use html5ever::{
    tendril::StrTendril,
    tokenizer::{
        BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
        states::RawKind,
    },
};
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{
    cell::{Cell, RefCell},
    fmt,
    sync::Arc,
};

impl Session {
    pub(super) async fn current_user(
        &self,
        context: Arc<dyn RequestContext>,
    ) -> std::result::Result<User, Failure> {
        let mut response = self.request(context.clone()).await?;
        let Some(body) = response.body.as_mut() else {
            return Err(Failure::message("FANBOX response has no body"));
        };
        let document = read_body(body.as_mut()).await;
        let close = body.close().await;
        match (document, close) {
            (Err(read), close) => {
                let read = safe_external_error(
                    context.as_ref(),
                    "read FANBOX identity page failed",
                    read.as_ref(),
                );
                let cause = match close {
                    Ok(()) => read,
                    Err(close) => Cause::Joined(vec![
                        read,
                        safe_external_error(
                            context.as_ref(),
                            "close FANBOX identity page failed",
                            close.as_ref(),
                        ),
                    ]),
                };
                Err(Failure {
                    code: Reason::UpstreamError,
                    cause,
                })
            }
            (Ok(_), Err(close)) => Err(Failure {
                code: Reason::UpstreamError,
                cause: safe_external_error(
                    context.as_ref(),
                    "close FANBOX identity page failed",
                    close.as_ref(),
                ),
            }),
            (Ok(document), Ok(())) => parse_identity(&document),
        }
    }
}
#[derive(Default)]
struct UserWire {
    user_id: Option<Box<RawValue>>,
    name: String,
    creator_id: Option<Box<RawValue>>,
    creator_status: Option<Box<RawValue>>,
    is_creator: Option<bool>,
}
struct Entries(Vec<(String, Box<RawValue>)>);
impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = Entries;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object or null")
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(Entries(vec![]))
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut entries = vec![];
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(Entries(entries))
            }
        }
        deserializer.deserialize_any(ObjectVisitor)
    }
}
pub(super) fn json_name_eq(name: &str, expected: &str) -> bool {
    // These are the only non-ASCII SimpleFold aliases of ASCII JSON field names.
    let mut name = name.chars();
    for expected in expected.bytes() {
        let Some(actual) = name.next() else {
            return false;
        };
        let folded = match actual {
            '\u{017f}' => b'S',
            '\u{212a}' => b'K',
            value if value.is_ascii() => (value as u8).to_ascii_uppercase(),
            _ => return false,
        };
        if folded != expected.to_ascii_uppercase() {
            return false;
        }
    }
    name.next().is_none()
}
fn merge_envelope(raw: &str, user: &mut UserWire) -> std::result::Result<(), serde_json::Error> {
    for (key, context) in serde_json::from_str::<Entries>(raw)?.0 {
        if !json_name_eq(&key, "context") {
            continue;
        }
        for (key, value) in serde_json::from_str::<Entries>(context.get())?.0 {
            if !json_name_eq(&key, "user") {
                continue;
            }
            for (key, value) in serde_json::from_str::<Entries>(value.get())?.0 {
                if json_name_eq(&key, "userId") {
                    user.user_id = Some(value);
                } else if json_name_eq(&key, "name") {
                    if let Some(name) = serde_json::from_str::<Option<String>>(value.get())? {
                        user.name = name;
                    }
                } else if json_name_eq(&key, "creatorId") {
                    user.creator_id = Some(value);
                } else if json_name_eq(&key, "creatorStatus") {
                    user.creator_status = Some(value);
                } else if json_name_eq(&key, "isCreator") {
                    user.is_creator = serde_json::from_str(value.get())?;
                }
            }
        }
    }
    Ok(())
}
fn optional_text(value: Option<&RawValue>) -> Option<String> {
    let Some(value) = value else {
        return Some(String::new());
    };
    if value.get() == "null" {
        return Some(String::new());
    }
    if let Ok(value) = serde_json::from_str::<String>(value.get()) {
        return Some(value.trim().to_owned());
    }
    let first = value.get().as_bytes().first()?;
    (first.is_ascii_digit() || *first == b'-').then(|| value.get().to_owned())
}
fn parse_identity(document: &[u8]) -> std::result::Result<User, Failure> {
    let metadata = metadata_content(document)
        .ok_or_else(|| Failure::message("FANBOX metadata tag was not found"))?;
    // Go retains literal attribute NULs, which JSON rejects; html5ever replaces them.
    if metadata.raw_nul {
        return Err(Failure::message("FANBOX metadata is not valid JSON"));
    }
    let metadata = crate::codec::normalize_json(metadata.value.as_bytes())
        .map_err(|_| Failure::message("FANBOX metadata is not valid JSON"))?;
    let mut user = UserWire::default();
    merge_envelope(&metadata, &mut user)
        .map_err(|_| Failure::message("FANBOX metadata is not valid JSON"))?;
    let user_id = optional_text(user.user_id.as_deref())
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| Failure::message("FANBOX metadata has no valid user id"))?;
    let display_name = user.name.trim().to_owned();
    if display_name.is_empty() {
        return Err(Failure::message("FANBOX metadata has no display name"));
    }
    let creator_id = optional_text(user.creator_id.as_deref())
        .ok_or_else(|| Failure::message("FANBOX metadata has an invalid creator id"))?;
    let (creator_status, is_creator) = match user
        .creator_status
        .as_deref()
        .filter(|raw| raw.get() != "null")
    {
        None => (
            String::new(),
            user.is_creator.unwrap_or(!creator_id.is_empty()),
        ),
        Some(raw) => {
            if let Ok(value) = serde_json::from_str::<String>(raw.get()) {
                (
                    value.trim().to_owned(),
                    user.is_creator.unwrap_or(!creator_id.is_empty()),
                )
            } else {
                let value = serde_json::from_str::<bool>(raw.get()).map_err(|_| {
                    Failure::message("FANBOX metadata has an invalid creator status")
                })?;
                let value = user.is_creator.unwrap_or(value);
                (value.to_string(), value)
            }
        }
    };
    Ok(User {
        user_id,
        display_name,
        creator_id,
        creator_status,
        is_creator,
    })
}
struct RawTag {
    source: String,
    attributes: Vec<(String, std::ops::Range<usize>, std::ops::Range<usize>)>,
}
fn html_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}
fn metadata_tag_source(span: &str) -> Option<RawTag> {
    for (start, _) in span.match_indices('<') {
        let source = &span[start..];
        let bytes = source.as_bytes();
        if !bytes
            .get(1..5)
            .is_some_and(|name| name.eq_ignore_ascii_case(b"meta"))
            || !bytes
                .get(5)
                .is_some_and(|byte| html_space(*byte) || matches!(byte, b'/' | b'>'))
        {
            continue;
        }
        let mut at = 5;
        let mut attributes = vec![];
        while at < bytes.len() {
            while bytes.get(at).is_some_and(|byte| html_space(*byte)) {
                at += 1;
            }
            match bytes.get(at) {
                Some(b'>') => {
                    if at + 1 == bytes.len() {
                        return Some(RawTag {
                            source: source.into(),
                            attributes,
                        });
                    }
                    break;
                }
                Some(b'/') => {
                    at += 1;
                    continue;
                }
                None => break,
                _ => {}
            }
            let attr_start = at;
            at += 1;
            while bytes
                .get(at)
                .is_some_and(|byte| !html_space(*byte) && !matches!(byte, b'=' | b'/' | b'>'))
            {
                at += 1;
            }
            let name = source[attr_start..at]
                .replace('\0', "\u{fffd}")
                .to_ascii_lowercase();
            let name_range = attr_start..at;
            let mut value_range = at..at;
            while bytes.get(at).is_some_and(|byte| html_space(*byte)) {
                at += 1;
            }
            if bytes.get(at) == Some(&b'=') {
                at += 1;
                while bytes.get(at).is_some_and(|byte| html_space(*byte)) {
                    at += 1;
                }
                if let Some(quote) = bytes
                    .get(at)
                    .filter(|byte| matches!(byte, b'\'' | b'"'))
                    .copied()
                {
                    at += 1;
                    let value_start = at;
                    while bytes.get(at).is_some_and(|byte| *byte != quote) {
                        at += 1;
                    }
                    value_range = value_start..at;
                    if at < bytes.len() {
                        at += 1;
                    }
                } else {
                    let value_start = at;
                    while bytes
                        .get(at)
                        .is_some_and(|byte| !html_space(*byte) && *byte != b'>')
                    {
                        at += 1;
                    }
                    value_range = value_start..at;
                }
            }
            attributes.push((name, name_range, value_range));
        }
    }
    None
}
struct AttributeSink(RefCell<Vec<(String, String)>>);
impl TokenSink for AttributeSink {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        if let Token::TagToken(tag) = token {
            *self.0.borrow_mut() = tag
                .attrs
                .into_iter()
                .map(|attr| (attr.name.local.to_string(), attr.value.to_string()))
                .collect();
        }
        TokenSinkResult::Continue
    }
}
fn preserved_attributes(raw: &RawTag) -> Vec<(String, String)> {
    // html5ever drops duplicate attributes; Go's identity tokenizer retains their order.
    let mut rewritten = String::new();
    let mut at = 0;
    let mut names = std::collections::BTreeMap::new();
    for (index, (name, range, _)) in raw.attributes.iter().enumerate() {
        let replacement = format!("fanbox-attribute-{index}");
        rewritten.push_str(&raw.source[at..range.start]);
        rewritten.push_str(&replacement);
        names.insert(replacement, name.clone());
        at = range.end;
    }
    rewritten.push_str(&raw.source[at..]);
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(&rewritten));
    let tokenizer = Tokenizer::new(
        AttributeSink(RefCell::new(vec![])),
        TokenizerOpts::default(),
    );
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer
        .sink
        .0
        .into_inner()
        .into_iter()
        .map(|(name, value)| (names.remove(&name).unwrap_or(name), value))
        .collect()
}
struct MetadataContent {
    value: String,
    raw_nul: bool,
}
struct MetadataSink<'a> {
    input: &'a BufferQueue,
    document: &'a str,
    boundary: Cell<usize>,
    metadata: RefCell<Option<MetadataContent>>,
}
impl MetadataSink<'_> {
    fn consumed(&self) -> usize {
        let remaining = (*self.input).clone();
        let mut bytes = 0;
        while let Some(buffer) = remaining.pop_front() {
            bytes += buffer.len();
        }
        self.document.len().saturating_sub(bytes)
    }
}
impl TokenSink for MetadataSink<'_> {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let tag = match token {
            Token::TagToken(tag) => tag,
            Token::CommentToken(_) | Token::DoctypeToken(_) => {
                self.boundary.set(self.consumed());
                return TokenSinkResult::Continue;
            }
            _ => return TokenSinkResult::Continue,
        };
        let end = self.consumed();
        let source = self.document.get(self.boundary.replace(end)..end);
        let is_end = tag.kind == TagKind::EndTag;
        if !is_end && tag.name.as_ref() == "meta" && self.metadata.borrow().is_none() {
            let raw = source.and_then(metadata_tag_source);
            let raw_nul = raw.as_ref().is_some_and(|raw| {
                raw.attributes
                    .iter()
                    .rev()
                    .find(|(name, _, _)| name == "content")
                    .is_some_and(|(_, _, value)| raw.source[value.clone()].contains('\0'))
            });
            let attributes = if tag.had_duplicate_attributes {
                raw.as_ref().map(preserved_attributes).unwrap_or_default()
            } else {
                tag.attrs
                    .into_iter()
                    .map(|attr| (attr.name.local.to_string(), attr.value.to_string()))
                    .collect()
            };
            let mut name = String::new();
            let mut content = String::new();
            for (key, value) in attributes {
                if key.eq_ignore_ascii_case("name") {
                    name = value;
                } else if key.eq_ignore_ascii_case("content") {
                    content = value;
                }
            }
            if name.trim().eq_ignore_ascii_case("metadata") && !content.trim().is_empty() {
                *self.metadata.borrow_mut() = Some(MetadataContent {
                    value: content,
                    raw_nul,
                });
            }
        }
        // Go activates raw text for these names even on self-closing start tags.
        if !is_end {
            match tag.name.as_ref() {
                "script" => return TokenSinkResult::RawData(RawKind::ScriptData),
                "style" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript" => {
                    return TokenSinkResult::RawData(RawKind::Rawtext);
                }
                "title" | "textarea" => return TokenSinkResult::RawData(RawKind::Rcdata),
                "plaintext" => return TokenSinkResult::Plaintext,
                _ => {}
            }
        }
        TokenSinkResult::Continue
    }
}
fn metadata_content(document: &[u8]) -> Option<MetadataContent> {
    let document = crate::codec::go_utf8(document);
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(&document));
    let tokenizer = Tokenizer::new(
        MetadataSink {
            input: &input,
            document: &document,
            boundary: Cell::new(0),
            metadata: RefCell::new(None),
        },
        TokenizerOpts::default(),
    );
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.metadata.into_inner()
}
