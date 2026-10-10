use super::{Client, Failure, ResolveURLRequest};
use crate::{Reason, Result, context::RequestContext};
use serde::Serialize;
use std::sync::Arc;
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    #[default]
    #[serde(rename = "")]
    Empty,
    Creator,
    CreatorPosts,
    Post,
    Tag,
}
impl ReferenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "",
            Self::Creator => "creator",
            Self::CreatorPosts => "creator_posts",
            Self::Post => "post",
            Self::Tag => "tag",
        }
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Reference {
    pub kind: ReferenceKind,
    pub creator_id: String,
    pub post_id: String,
    pub tag: String,
}
fn fail(message: &str) -> crate::Error {
    Failure::new(Reason::InvalidArgument, message).classify("ResolveURL")
}
impl Client {
    pub fn resolve_url(
        &self,
        _context: Arc<dyn RequestContext>,
        request: ResolveURLRequest,
    ) -> Result<Reference> {
        let raw = request.raw_url.trim();
        if !raw.contains("://") {
            return Err(fail("URL must be https without userinfo"));
        }
        let target = super::options::parse_url(raw).ok_or_else(|| fail("URL is not parseable"))?;
        if target.parsed.scheme() != "https" || target.authority.contains('@') {
            return Err(fail("URL must be https without userinfo"));
        }
        let host = target.parsed.host_str().unwrap_or("").to_ascii_lowercase();
        if host == "www.fanbox.cc" || host == "fanbox.cc" {
            return parse_path(&target.path);
        }
        if let Some(creator) = host.strip_suffix(".fanbox.cc") {
            if creator.is_empty() || creator.contains('.') {
                return Err(fail("unsupported fanbox subdomain"));
            }
            return Ok(Reference {
                kind: ReferenceKind::Creator,
                creator_id: creator.into(),
                ..Default::default()
            });
        }
        Err(fail("URL host is not a supported FANBOX host"))
    }
}
fn parse_path(path: &str) -> Result<Reference> {
    let path = crate::reference::decode_url_component(path, false)
        .ok_or_else(|| fail("URL is not parseable"))?;
    let path = path.trim_matches('/');
    let parts = if path.is_empty() {
        vec![]
    } else {
        path.split('/').collect::<Vec<_>>()
    };
    if parts.first() == Some(&"creators") {
        if parts.len() == 2 && !parts[1].is_empty() {
            return Ok(Reference {
                kind: ReferenceKind::Creator,
                creator_id: parts[1].into(),
                ..Default::default()
            });
        }
        return Err(fail("malformed creators redirect"));
    }
    if let Some(creator) = parts.first().and_then(|v| v.strip_prefix('@')) {
        if creator.is_empty() {
            return Err(fail("creator id is empty"));
        }
        let mut out = Reference {
            creator_id: creator.into(),
            ..Default::default()
        };
        match parts.len() {
            1 => out.kind = ReferenceKind::Creator,
            2 => {
                if parts[1] != "posts" {
                    return Err(fail("unsupported creator path"));
                }
                out.kind = ReferenceKind::CreatorPosts;
            }
            3 => {
                if parts[1] != "posts" {
                    return Err(fail("unsupported creator path"));
                }
                if parts[2] == "tag" {
                    return Err(fail("tag path is missing a tag"));
                }
                out.kind = ReferenceKind::Post;
                out.post_id = parts[2].into();
            }
            4 => {
                if parts[1] != "posts" || parts[2] != "tag" {
                    return Err(fail("unsupported tag path"));
                }
                out.kind = ReferenceKind::Tag;
                out.tag = parts[3].into();
            }
            _ => return Err(fail("unsupported creator path depth")),
        }
        return Ok(out);
    }
    Err(fail("URL does not name a supported FANBOX resource"))
}
