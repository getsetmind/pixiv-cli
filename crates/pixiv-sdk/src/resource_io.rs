use crate::{Error, Reason, Result, resource::ResourceRef};
use std::{collections::BTreeMap, fmt};
use tokio::io::AsyncRead;

pub const RESOURCE_METHOD_GET: &str = "GET";
pub const RESOURCE_METHOD_HEAD: &str = "HEAD";

#[derive(Clone, Default, Eq, PartialEq)]
pub struct OpenResourceRequest {
    pub reference: ResourceRef,
    pub method: String,
    pub range: String,
    pub if_none_match: String,
    pub if_modified_since: String,
    pub if_range: String,
}
impl fmt::Debug for OpenResourceRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenResourceRequest")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}
impl OpenResourceRequest {
    pub fn validate(&self) -> Result<()> {
        let invalid = |detail| {
            Error::with_product("", Reason::InvalidArgument, "OpenResourceRequest.Validate")
                .with_detail(detail)
        };
        if !matches!(
            self.method.as_str(),
            "" | RESOURCE_METHOD_GET | RESOURCE_METHOD_HEAD
        ) {
            return Err(invalid("unsupported resource method".to_owned()));
        }
        for (name, value) in [
            ("range", &self.range),
            ("if-none-match", &self.if_none_match),
            ("if-modified-since", &self.if_modified_since),
            ("if-range", &self.if_range),
        ] {
            if value.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
                return Err(invalid(format!(
                    "{name} header contains control characters"
                )));
            }
        }
        Ok(())
    }
}

pub type ResourceHeaders = BTreeMap<String, Vec<String>>;

pub struct ResourceResponse<R: AsyncRead + Unpin + Send> {
    pub status_code: i64,
    pub body: R,
    headers: ResourceHeaders,
}
impl<R: AsyncRead + Unpin + Send> fmt::Debug for ResourceResponse<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResourceResponse")
            .field("status_code", &self.status_code)
            .finish_non_exhaustive()
    }
}
impl<R: AsyncRead + Unpin + Send> ResourceResponse<R> {
    pub fn new(status_code: i64, source: &ResourceHeaders, body: R) -> Self {
        let mut headers = ResourceHeaders::new();
        for name in [
            "Content-Type",
            "Content-Length",
            "Content-Range",
            "Accept-Ranges",
            "Etag",
            "Last-Modified",
            "Cache-Control",
        ] {
            if let Some(values) = source.get(name).filter(|values| !values.is_empty()) {
                headers.insert(name.to_owned(), values.clone());
            }
        }
        Self {
            status_code,
            body,
            headers,
        }
    }
    pub fn header(&self) -> ResourceHeaders {
        self.headers.clone()
    }
    fn first(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|values| values.first())
            .map(String::as_str)
            .unwrap_or_default()
    }
    pub fn content_type(&self) -> &str {
        self.first("Content-Type")
    }
    pub fn content_length(&self) -> i64 {
        self.first("Content-Length")
            .trim()
            .parse()
            .unwrap_or_default()
    }
    pub fn content_range(&self) -> &str {
        self.first("Content-Range")
    }
    pub fn accept_ranges(&self) -> &str {
        self.first("Accept-Ranges")
    }
    pub fn etag(&self) -> &str {
        self.first("Etag")
    }
    pub fn last_modified(&self) -> &str {
        self.first("Last-Modified")
    }
    pub fn cache_control(&self) -> &str {
        self.first("Cache-Control")
    }
}
