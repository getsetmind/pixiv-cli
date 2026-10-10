use crate::context::RequestContext;
use std::{collections::BTreeMap, error::Error, fmt, future::Future, pin::Pin, sync::Arc};

pub type ExternalError = Box<dyn Error + Send + Sync + 'static>;
pub type BodyFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type TransportFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type Headers = BTreeMap<String, Vec<String>>;

pub struct RawRequest {
    pub method: String,
    pub url: String,
    pub logical_host: Option<String>,
    pub headers: Headers,
    pub body: Option<Vec<u8>>,
    pub content_length: i64,
    pub context: Arc<dyn RequestContext>,
}
impl fmt::Debug for RawRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawRequest")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}
pub struct RawResponse {
    pub status: u16,
    pub headers: Headers,
    pub content_length: i64,
    pub body: Option<Box<dyn RawBody>>,
}
impl fmt::Debug for RawResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawResponse")
            .field("status", &self.status)
            .field("content_length", &self.content_length)
            .finish_non_exhaustive()
    }
}
pub struct RawRead {
    pub count: usize,
    pub eof: bool,
    pub error: Option<ExternalError>,
}
impl fmt::Debug for RawRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawRead")
            .field("count", &self.count)
            .field("eof", &self.eof)
            .field("has_error", &self.error.is_some())
            .finish()
    }
}
/// Exclusive Rust body ownership does not reproduce Go's concurrent Read/Close interface.
pub trait RawBody: Send {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead>;
    fn close(&mut self) -> BodyFuture<'_, std::result::Result<(), ExternalError>>;
}
/// Automatic redirects would bypass the SDK's per-hop credential and host policy.
pub trait RawTransport: Send + Sync {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, std::result::Result<Option<RawResponse>, ExternalError>>;
    fn close_idle_connections(&self) {}
}

pub(crate) struct EmptyBody;
impl RawBody for EmptyBody {
    fn read<'a>(&'a mut self, _output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async {
            RawRead {
                count: 0,
                eof: true,
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, std::result::Result<(), ExternalError>> {
        Box::pin(async { Ok(()) })
    }
}
pub(crate) fn header<'a>(headers: &'a Headers, name: &str) -> &'a str {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, values)| values.first())
        .map(String::as_str)
        .unwrap_or("")
}
